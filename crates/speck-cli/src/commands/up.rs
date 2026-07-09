use std::io::Read as _;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use indicatif::ProgressBar;
use sha2::{Digest as _, Sha256};
use speck_core::VmState;
use speck_dockerd::SpeckDockerd;
use speck_net::config::NetworkConfig;
use speck_vz::config::GuestConfig;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;
use tokio::signal::unix::{SignalKind, signal};

use anstream::{eprint, eprintln, print, println};

use crate::UpArgs;
use crate::config::{EffectiveConfig, EffectiveVmConfig};
use crate::docker_client::DockerClient;
use crate::shell::{self, EnvShell};
use crate::theme::{NEON_CYAN, RESET};

const ROOTFS_VERSION: &str = "0.2.0";
const ROOTFS_BACKEND: &str = "moby";
const KATA_VERSION: &str = "3.32.0";
const KATA_KERNEL_FILE: &str = "vmlinux-6.18.35-197";
const KATA_INITRD_FILE: &str = "kata-alpine-3.22.initrd";
const INITRD_VERSION: &str = "0.1.0";
const LAUNCHD_LABEL: &str = "io.speck.vm";

/// Returns true when running interactively (stderr is a TTY and not launched as
/// a launchd daemon). Used to decide whether to show curl progress bars.
fn is_interactive() -> bool {
    use std::io::IsTerminal as _;
    std::env::var("SPECK_DAEMONIZED").as_deref() != Ok("1") && std::io::stderr().is_terminal()
}

pub fn daemonize(speck_home: &Path, binary: &Path) -> anyhow::Result<()> {
    let _ = speck_home;
    let home_dir = std::env::var("HOME").context("HOME not set")?;
    let plist_path = PathBuf::from(&home_dir)
        .join("Library/LaunchAgents")
        .join(format!("{LAUNCHD_LABEL}.plist"));
    std::fs::create_dir_all(plist_path.parent().context("plist path has no parent")?)?;

    let uid_out = std::process::Command::new("id")
        .arg("-u")
        .output()
        .context("failed to run id -u")?;
    let uid_str = std::str::from_utf8(&uid_out.stdout)
        .context("non-UTF8 uid")?
        .trim()
        .to_owned();

    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LAUNCHD_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{}</string>
        <string>up</string>
        <string>--foreground</string>
    </array>
    <key>EnvironmentVariables</key>
    <dict>
        <key>SPECK_DAEMONIZED</key>
        <string>1</string>
        <key>PATH</key>
        <string>/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin</string>
    </dict>
    <key>KeepAlive</key>
    <true/>
    <key>RunAtLoad</key>
    <true/>
</dict>
</plist>
"#,
        binary.display()
    );

    std::fs::write(&plist_path, &plist)
        .with_context(|| format!("failed to write plist {}", plist_path.display()))?;

    let status = std::process::Command::new("launchctl")
        .args([
            "bootstrap",
            &format!("gui/{uid_str}"),
            &plist_path.to_string_lossy(),
        ])
        .status()
        .context("launchctl bootstrap exec failed")?;

    anyhow::ensure!(
        status.success(),
        "launchctl bootstrap failed - check that launchd is running (macOS only)"
    );
    println!("{NEON_CYAN}Speck daemon registered — VM is starting in the background.{RESET}");
    println!("  Follow boot:  spk logs --follow");
    println!("  Check status: spk status");
    println!("  For scripts:  spk up --wait  (blocks until ready)");
    println!("  To stop:      spk down");
    Ok(())
}

/// Ensure all VM assets (kernel, initrd, rootfs, data disk) are present in
/// `speck_home`, downloading them from GitHub Releases if not.
///
/// This makes `spk up` self-bootstrapping: first run works with no separate
/// init step, exactly like `colima start`.
async fn ensure_assets(
    speck_home: &Path,
    requested_disk_gb: u64,
    force_refresh: bool,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(speck_home.join("kernel"))?;
    std::fs::create_dir_all(speck_home.join("initrd"))?;

    let kernel = speck_home.join("kernel/vmlinux");
    let initrd = speck_home.join(format!("initrd/{KATA_INITRD_FILE}"));
    let rootfs = speck_home.join("rootfs.img");
    let data = speck_home.join("data.img");

    if force_refresh || !kernel.exists() || !initrd.exists() {
        fetch_kata_assets(speck_home)
            .await
            .context("failed to download kernel + initrd")?;
    }

    let custom_initrd = speck_home.join("initrd/initrd.cpio.gz");
    if force_refresh || !custom_initrd.exists() {
        fetch_initrd(speck_home)
            .await
            .context("failed to download initrd")?;
    }

    if force_refresh || !rootfs.exists() {
        fetch_rootfs(speck_home)
            .await
            .context("failed to download rootfs")?;
    }

    reconcile_data_disk(&data, requested_disk_gb).context("failed to reconcile data disk")?;

    Ok(())
}

fn check_asset_version_file(
    path: &Path,
    asset_name: &str,
    required_version: &str,
) -> anyhow::Result<()> {
    if !path.exists() {
        return Ok(());
    }

    let on_disk_version = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?
        .trim()
        .to_owned();

    anyhow::ensure!(
        on_disk_version == required_version,
        "stale {asset_name} asset version at {}: found {on_disk_version}, required {required_version}; run `spk up --pull` to refresh VM assets",
        path.display()
    );

    Ok(())
}

fn check_asset_versions(speck_home: &Path) -> anyhow::Result<()> {
    check_asset_version_file(&speck_home.join("rootfs.version"), "rootfs", ROOTFS_VERSION)?;
    check_asset_version_file(
        &speck_home.join("initrd/initrd.version"),
        "initrd",
        INITRD_VERSION,
    )?;

    Ok(())
}

fn console_log_hint(speck_home: &Path) -> String {
    format!(
        "check {} for guest boot messages",
        speck_home.join("console.log").display()
    )
}

/// Download the Kata Containers kernel + initrd from GitHub Releases and
/// place them under `speck_home/kernel/` and `speck_home/initrd/`.
async fn fetch_kata_assets(speck_home: &Path) -> anyhow::Result<()> {
    let kernel_dir = speck_home.join("kernel");
    let initrd_dir = speck_home.join("initrd");
    let kernel_out = kernel_dir.join(KATA_KERNEL_FILE);
    let initrd_out = initrd_dir.join(KATA_INITRD_FILE);
    let symlink = kernel_dir.join("vmlinux");

    println!("  Downloading Kata kernel {KATA_VERSION} (arm64)...");

    let url = format!(
        "https://github.com/kata-containers/kata-containers/releases/download\
         /{KATA_VERSION}/kata-static-{KATA_VERSION}-arm64.tar.zst"
    );

    let tmp_dir =
        std::path::PathBuf::from(format!("/tmp/speck-kata-assets-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir)?;

    // curl → zstdcat → tar, extracting only the two files we need.
    // The shell command contains only fixed URLs/filenames; destination paths
    // are handled below with Rust filesystem calls to avoid shell injection.
    // curl_flags is one of two hardcoded strings, so no shell injection risk.
    let curl_flags = if is_interactive() {
        "-fL --progress-bar"
    } else {
        "-fsSL"
    };
    let status = tokio::process::Command::new("bash")
        .args([
            "-c",
            &format!(
                r#"curl {curl_flags} {url} | zstdcat -c | tar -C /tmp/speck-kata-assets-{pid} \
                    --strip-components=5 -xf - \
                    ./opt/kata/share/kata-containers/{KATA_KERNEL_FILE} \
                    ./opt/kata/share/kata-containers/{KATA_INITRD_FILE}"#,
                curl_flags = curl_flags,
                url = url,
                pid = std::process::id(),
            ),
        ])
        .status()
        .await
        .context("failed to run curl/zstdcat — is zstd installed? (brew install zstd)")?;

    anyhow::ensure!(status.success(), "kernel download failed");

    std::fs::rename(tmp_dir.join(KATA_KERNEL_FILE), &kernel_out)?;
    std::fs::rename(tmp_dir.join(KATA_INITRD_FILE), &initrd_out)?;
    let _ = std::fs::remove_dir_all(&tmp_dir);

    // Stable symlink: kernel/vmlinux → vmlinux-6.18.35-197
    let _ = std::fs::remove_file(&symlink);
    std::os::unix::fs::symlink(KATA_KERNEL_FILE, &symlink)?;

    println!("  Kernel ready: {}", symlink.display());
    println!("  Initrd ready: {}", initrd_out.display());
    Ok(())
}

/// Download `speck-initrd-{VERSION}-arm64.cpio.gz` from GitHub Releases and
/// place it at `speck_home/initrd/initrd.cpio.gz`.
async fn fetch_initrd(speck_home: &Path) -> anyhow::Result<()> {
    let dest = speck_home.join("initrd/initrd.cpio.gz");
    let tmp = speck_home.join("initrd/initrd.cpio.gz.tmp");
    let _ = std::fs::remove_file(&tmp);

    let base =
        format!("https://github.com/wasilak/speck/releases/download/initrd-{INITRD_VERSION}");
    let name = format!("speck-initrd-{INITRD_VERSION}-arm64.cpio.gz");

    println!("  Downloading initrd {INITRD_VERSION}...");

    let mut curl_cmd = tokio::process::Command::new("curl");
    if is_interactive() {
        curl_cmd.args(["-fL", "--progress-bar"]);
    } else {
        curl_cmd.arg("-fsSL");
    }
    let status = curl_cmd
        .arg(&format!("{base}/{name}"))
        .arg("-o")
        .arg(&tmp)
        .status()
        .await
        .context("curl failed")?;

    if !status.success() {
        let _ = std::fs::remove_file(&tmp);
        anyhow::bail!("initrd download failed");
    }

    std::fs::rename(&tmp, &dest)?;
    std::fs::write(
        speck_home.join("initrd/initrd.version"),
        format!("{INITRD_VERSION}\n"),
    )?;
    println!("  Initrd ready: {}", dest.display());
    Ok(())
}

/// Download `speck-rootfs-{VERSION}-{BACKEND}-arm64.img.gz` from GitHub
/// Releases, verify its SHA-256 checksum, and decompress to `rootfs.img`.
async fn fetch_rootfs(speck_home: &Path) -> anyhow::Result<()> {
    let dest = speck_home.join("rootfs.img");
    let tmp = speck_home.join("rootfs.img.tmp");
    let gz_tmp = speck_home.join("rootfs.img.gz.tmp");
    let _ = std::fs::remove_file(&tmp);
    let _ = std::fs::remove_file(&gz_tmp);

    let base =
        format!("https://github.com/wasilak/speck/releases/download/rootfs-{ROOTFS_VERSION}");
    let gz_name = format!("speck-rootfs-{ROOTFS_VERSION}-{ROOTFS_BACKEND}-arm64.img.gz");
    let sum_name = format!("speck-rootfs-{ROOTFS_VERSION}-{ROOTFS_BACKEND}-arm64.img.sha256");

    println!("  Downloading rootfs {ROOTFS_VERSION} ({ROOTFS_BACKEND})...");

    let mut curl_cmd = tokio::process::Command::new("curl");
    if is_interactive() {
        curl_cmd.args(["-fL", "--progress-bar"]);
    } else {
        curl_cmd.arg("-fsSL");
    }
    let curl_status = curl_cmd
        .arg(&format!("{base}/{gz_name}"))
        .arg("-o")
        .arg(&gz_tmp)
        .status()
        .await
        .context("curl failed")?;
    if !curl_status.success() {
        let _ = std::fs::remove_file(&gz_tmp);
        anyhow::bail!("rootfs download failed");
    }

    let tmp_file = match std::fs::File::create(&tmp) {
        Ok(f) => f,
        Err(e) => {
            let _ = std::fs::remove_file(&gz_tmp);
            return Err(e).with_context(|| format!("failed to create {}", tmp.display()));
        }
    };
    let gunzip_status = tokio::process::Command::new("gunzip")
        .arg("-c")
        .arg(&gz_tmp)
        .stdout(Stdio::from(tmp_file))
        .status()
        .await
        .context("gunzip failed")?;
    if !gunzip_status.success() {
        let _ = std::fs::remove_file(&gz_tmp);
        let _ = std::fs::remove_file(&tmp);
        anyhow::bail!("rootfs decompression failed");
    }
    let _ = std::fs::remove_file(&gz_tmp);

    // Verify SHA-256.
    let sum_bytes = tokio::process::Command::new("curl")
        .args(["-fsSL", &format!("{base}/{sum_name}")])
        .output()
        .await
        .context("failed to download checksum")?;
    anyhow::ensure!(sum_bytes.status.success(), "checksum download failed");

    let expected = std::str::from_utf8(&sum_bytes.stdout)
        .ok()
        .and_then(|s| s.split_whitespace().next())
        .context("invalid checksum file")?
        .to_string();

    let actual = {
        let mut f = std::fs::File::open(&tmp)
            .with_context(|| format!("failed to open {} for checksum", tmp.display()))?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = f.read(&mut buf).context("read error during checksum")?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        format!("{:x}", hasher.finalize())
    };

    anyhow::ensure!(
        expected == actual,
        "rootfs SHA-256 mismatch (expected {expected}, got {actual})"
    );

    // On checksum mismatch, remove the corrupted tmp file.
    // On success, atomically rename to final path.
    std::fs::rename(&tmp, &dest)?;
    std::fs::write(
        speck_home.join("rootfs.version"),
        format!("{ROOTFS_VERSION}\n"),
    )?;
    println!("  Rootfs ready: {}", dest.display());
    Ok(())
}

pub fn requested_disk_bytes(requested_gb: u64) -> anyhow::Result<u64> {
    requested_gb
        .checked_mul(1024)
        .and_then(|bytes| bytes.checked_mul(1024))
        .and_then(|bytes| bytes.checked_mul(1024))
        .context("requested disk size overflows u64 bytes")
}

pub fn reconcile_data_disk(path: &Path, requested_gb: u64) -> anyhow::Result<()> {
    let requested_bytes = requested_disk_bytes(requested_gb)?;

    if !path.exists() {
        let tmp = path.with_extension("img.tmp");
        let _ = std::fs::remove_file(&tmp);
        println!("  Creating data disk ({requested_gb} GiB)...");
        let file = std::fs::File::create(&tmp)
            .with_context(|| format!("failed to create {}", tmp.display()))?;
        file.set_len(requested_bytes)
            .with_context(|| format!("failed to size {}", tmp.display()))?;
        std::fs::rename(&tmp, path)
            .with_context(|| format!("failed to install {}", path.display()))?;
        println!("  Data disk ready: {}", path.display());
        return Ok(());
    }

    let current_bytes = std::fs::metadata(path)
        .with_context(|| format!("failed to inspect {}", path.display()))?
        .len();
    if requested_bytes < current_bytes {
        anyhow::bail!(
            "disk shrink not supported: requested {requested_gb} GiB but current disk is {} GiB",
            current_bytes / 1024 / 1024 / 1024
        );
    }
    if requested_bytes > current_bytes {
        println!("  Growing data disk to {requested_gb} GiB...");
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .with_context(|| format!("failed to open {}", path.display()))?;
        file.set_len(requested_bytes)
            .with_context(|| format!("failed to grow {}", path.display()))?;
    }

    Ok(())
}

pub fn read_vm_resource_snapshot(speck_home: &Path) -> anyhow::Result<Option<EffectiveVmConfig>> {
    let path = speck_home.join("run/vm-config.json");
    if !path.exists() {
        return Ok(None);
    }

    let contents = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let snapshot = serde_json::from_str(&contents)
        .with_context(|| format!("failed to decode {}", path.display()))?;
    Ok(Some(snapshot))
}

pub fn write_vm_resource_snapshot(speck_home: &Path, vm: &EffectiveVmConfig) -> anyhow::Result<()> {
    let run_dir = speck_home.join("run");
    std::fs::create_dir_all(&run_dir)
        .with_context(|| format!("failed to create {}", run_dir.display()))?;
    let path = run_dir.join("vm-config.json");
    let tmp = run_dir.join("vm-config.json.tmp");
    let contents =
        serde_json::to_vec_pretty(vm).context("failed to encode VM resource snapshot")?;
    std::fs::write(&tmp, contents).with_context(|| format!("failed to write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path)
        .with_context(|| format!("failed to install {}", path.display()))?;
    Ok(())
}

pub fn runtime_holders_exist(paths: &[&Path]) -> bool {
    let my_pid = std::process::id();

    for path in paths {
        let Ok(out) = std::process::Command::new("lsof")
            .args(["-t", &path.to_string_lossy()])
            .output()
        else {
            continue;
        };
        for line in out.stdout.split(|&b| b == b'\n') {
            let Ok(s) = std::str::from_utf8(line) else {
                continue;
            };
            let Ok(pid) = s.trim().parse::<u32>() else {
                continue;
            };
            if pid != my_pid {
                return true;
            }
        }
    }

    false
}

pub fn ensure_no_active_vm_resource_mismatch(
    speck_home: &Path,
    rootfs_disk_path: &Path,
    data_disk_path: &Path,
    requested: &EffectiveVmConfig,
) -> anyhow::Result<()> {
    if !runtime_holders_exist(&[rootfs_disk_path, data_disk_path]) {
        return Ok(());
    }

    let Some(previous) = read_vm_resource_snapshot(speck_home)? else {
        return Ok(());
    };
    if previous.cpus != requested.cpus || previous.memory_mb != requested.memory_mb {
        anyhow::bail!(
            "VM resource change requires restart: old cpus={} memory_mb={}, new cpus={} memory_mb={}. Stop the VM and run spk up again.",
            previous.cpus,
            previous.memory_mb,
            requested.cpus,
            requested.memory_mb
        );
    }

    Ok(())
}

/// Kill any processes that hold an exclusive lock on the given disk image paths.
fn kill_stale_vm_holders(paths: &[&std::path::Path]) {
    let my_pid = std::process::id();
    let mut killed_any = false;

    for path in paths {
        let Ok(out) = std::process::Command::new("lsof")
            .args(["-t", &path.to_string_lossy()])
            .output()
        else {
            continue;
        };
        for line in out.stdout.split(|&b| b == b'\n') {
            let Ok(s) = std::str::from_utf8(line) else {
                continue;
            };
            let Ok(pid) = s.trim().parse::<u32>() else {
                continue;
            };
            if pid == my_pid {
                continue;
            }
            let _ = std::process::Command::new("kill")
                .args(["-9", &pid.to_string()])
                .status();
            killed_any = true;
        }
    }

    if killed_any {
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

pub async fn run_up(
    args: UpArgs,
    speck_home: &Path,
    effective: EffectiveConfig,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(speck_home).with_context(|| {
        format!(
            "failed to create speck_home directory: {}",
            speck_home.display()
        )
    })?;

    // Auto-download assets only when not all provided via CLI overrides.
    let has_all_overrides = args.kernel.is_some()
        && args.initrd.is_some()
        && args.rootfs.is_some()
        && args.data_disk.is_some();
    if !has_all_overrides {
        if !args.pull {
            check_asset_versions(speck_home)?;
        }
        ensure_assets(speck_home, effective.vm.disk_gb, args.pull).await?;
    }

    let kernel_path = args
        .kernel
        .clone()
        .unwrap_or_else(|| speck_home.join("kernel/vmlinux"));
    let initrd_path: std::path::PathBuf = args
        .initrd
        .clone()
        .or_else(|| {
            let p = speck_home.join("initrd/initrd.cpio.gz");
            if p.exists() { Some(p) } else { None }
        })
        .or_else(|| {
            let p = speck_home.join(format!("initrd/{KATA_INITRD_FILE}"));
            if p.exists() { Some(p) } else { None }
        })
        .unwrap_or_default();
    let rootfs_disk_path = args
        .rootfs
        .clone()
        .unwrap_or_else(|| speck_home.join("rootfs.img"));
    let data_disk_path = args
        .data_disk
        .clone()
        .unwrap_or_else(|| speck_home.join("data.img"));

    ensure_no_active_vm_resource_mismatch(
        speck_home,
        &rootfs_disk_path,
        &data_disk_path,
        &effective.vm,
    )?;

    kill_stale_vm_holders(&[&rootfs_disk_path, &data_disk_path]);

    // Validate and prepare CA certificates if any are configured (CERT-01).
    let ca_cert_paths =
        crate::config::validate_and_prepare_ca_certs(speck_home, &effective.extra_certs)
            .context("CA certificate validation failed")?;

    let mut builder = GuestConfig::builder()
        .kernel_path(kernel_path)
        .initrd_path(initrd_path)
        .rootfs_disk_path(rootfs_disk_path.clone())
        .data_disk_path(data_disk_path.clone())
        .cpu_count(effective.vm.cpus)
        .memory_size_bytes(effective.vm.memory_mb * 1024 * 1024)
        .containerd_vsock_port(9001)
        .buildkitd_vsock_port(9002)
        .ready_vsock_port(9000)
        .docker_vsock_port(9003)
        .log_relay_vsock_port(9005)
        .cmdline("console=hvc0 panic=-1 container_backend=dockerd docker_vsock_port=9003 ready_vsock_port=9000 dns_vsock_port=53 speck_guest_ip=172.16.0.2 speck_gateway=172.16.0.1 log_relay_vsock_port=9005")
        .speck_home(speck_home)
        .network(NetworkConfig::default())
        .dns_vsock_port(53)
        .add_identity_mount("/Users");

    if !ca_cert_paths.is_empty() {
        builder = builder.ca_certs_paths(&ca_cert_paths);
    }

    for optional_root in ["/Volumes", "/private/tmp"] {
        if std::path::Path::new(optional_root).exists() {
            builder = builder.add_identity_mount(optional_root);
        }
    }

    let config = builder.build();

    let guest = speck_vz::Guest::new(config);
    let spinner = ProgressBar::new_spinner();
    spinner.set_message("Starting VM...");
    spinner.enable_steady_tick(std::time::Duration::from_millis(100));

    let console_log_hint = console_log_hint(speck_home);
    let spinner_for_start = spinner.clone();
    let guest = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        guest.start().context("failed to start VM")?;
        spinner_for_start.set_message("Waiting for guest...");
        guest
            .wait_for_ready()
            .map_err(|e| anyhow::anyhow!("{} — {}", e, console_log_hint))?;
        Ok(guest)
    })
    .await
    .context("VM startup task failed")??;
    let guest = Arc::new(guest);

    write_vm_resource_snapshot(speck_home, &effective.vm)?;

    let (port_map_tx, port_map_rx) = tokio::sync::mpsc::channel::<speck_net::PortMapConfig>(64);
    guest.set_port_map_channel(port_map_tx)?;
    let netstack_fd = guest.netstack_fd()?;
    let dns_vsock_fd = match guest.connect_dns_vsock(53) {
        Ok(fd) => {
            tracing::info!(fd, "DNS vsock connected");
            Some(fd)
        }
        Err(e) => {
            tracing::warn!(error = %e, "DNS vsock connect failed — VPN-proof DNS disabled");
            None
        }
    };
    let (_resolver_tx, resolver_rx) = speck_net::spawn_resolver_watcher();
    let net_config = speck_net::config::NetworkConfig::default();
    let netstack_handles = speck_net::SpeckNet::new(net_config, Some(53)).spawn(
        netstack_fd,
        dns_vsock_fd,
        vec![],
        Some(port_map_rx),
        resolver_rx,
    );
    // Supervise netstack tasks so failures surface via structured logs instead of
    // being silently dropped when the JoinHandles are discarded (WR-05).
    {
        use futures::stream::{FuturesUnordered, StreamExt as _};
        let mut set: FuturesUnordered<
            tokio::task::JoinHandle<std::result::Result<(), speck_net::Error>>,
        > = netstack_handles.into_iter().collect();
        tokio::spawn(async move {
            while let Some(join_result) = set.next().await {
                match join_result {
                    Ok(Ok(())) => tracing::warn!(
                        "netstack task exited unexpectedly — VPN-proof networking may be degraded"
                    ),
                    Ok(Err(e)) => tracing::error!(
                        error = %e,
                        "netstack task error — VPN-proof networking may be degraded"
                    ),
                    Err(e) => tracing::error!(error = %e, "netstack task panicked"),
                }
            }
        });
    }

    spinner.set_message("Starting Docker API server...");
    let sock_path = speck_home.join("speck.sock");
    let vm_state: Arc<RwLock<VmState>> = Arc::new(RwLock::new(VmState::Running));
    let _dockerd = SpeckDockerd::start(guest.clone(), sock_path.clone(), vm_state.clone())
        .context("failed to start Docker API server")?;
    tracing::info!("Docker API server started at {}", sock_path.display());

    spinner.finish_with_message(format!("{NEON_CYAN}Speck is running{RESET}"));

    println!();
    print!("{}", shell::render_env(speck_home, EnvShell::Posix));
    print!("{}", shell::render_speck_home(speck_home, EnvShell::Posix));
    println!();

    std::fs::create_dir_all(speck_home.join("run"))?;

    let pid_path = speck_home.join("run/speck.pid");
    std::fs::write(&pid_path, format!("{}\n", std::process::id()))
        .context("failed to write pid file")?;

    let ctrl_sock_path = speck_home.join("run/control.sock");
    let _ = std::fs::remove_file(&ctrl_sock_path);
    let listener = UnixListener::bind(&ctrl_sock_path).context("failed to bind control socket")?;
    std::fs::set_permissions(&ctrl_sock_path, std::fs::Permissions::from_mode(0o600))?;
    let vm_state_ctrl = vm_state.clone();
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((mut stream, _)) => {
                    let state = vm_state_ctrl.clone();
                    tokio::spawn(async move {
                        let mut buf = [0u8; 32];
                        let n = match stream.read(&mut buf).await {
                            Ok(n) => n,
                            Err(e) => {
                                tracing::warn!("control socket read error: {e}");
                                return;
                            }
                        };
                        let cmd = std::str::from_utf8(&buf[..n]).unwrap_or("").trim();
                        match cmd {
                            "PREPARE_RESTART" => {
                                let _restart_start;
                                {
                                    let mut v = state.write().expect("VmState RwLock poisoned");
                                    *v = VmState::Restarting;
                                    _restart_start = Instant::now();
                                }
                                tracing::info!(
                                    "VmState set to Restarting — 503 middleware active, 60s lease"
                                );
                                let _ = stream.write_all(b"OK\n").await;
                                let state_clone = state.clone();
                                tokio::spawn(async move {
                                    tokio::time::sleep(Duration::from_secs(60)).await;
                                    let mut v =
                                        state_clone.write().expect("VmState RwLock poisoned");
                                    if *v == VmState::Restarting {
                                        *v = VmState::Running;
                                        tracing::warn!(
                                            "Restarting lease expired (60s) — reset to Running"
                                        );
                                    }
                                });
                            }
                            _ => {
                                let _ = stream.write_all(b"PONG\n").await;
                            }
                        }
                    });
                }
                Err(e) => {
                    tracing::warn!("control socket accept error: {e}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            }
        }
    });

    let mut sigterm =
        signal(SignalKind::terminate()).context("failed to install SIGTERM handler")?;

    tokio::select! {
        result = tokio::signal::ctrl_c() => {
            result.context("SIGINT listener failed")?;
            tracing::info!("SIGINT received - shutting down");
        }
        _ = sigterm.recv() => {
            tracing::info!("SIGTERM received - shutting down");
        }
    }

    shutdown_gracefully(&guest, &ctrl_sock_path, speck_home).await?;

    Ok(())
}

async fn shutdown_gracefully(
    guest: &Arc<speck_vz::Guest>,
    sock_path: &Path,
    speck_home: &Path,
) -> anyhow::Result<()> {
    let client = DockerClient::new(speck_home.join("speck.sock"));
    if let Ok(containers) = client.get("/containers/json").await
        && let Some(containers) = containers.as_array()
    {
        for container in containers {
            if let Some(id) = container.get("Id").and_then(|value| value.as_str()) {
                match client.post_empty(&format!("/containers/{id}/stop")).await {
                    Ok(_) => tracing::info!(container_id = id, "stopped container"),
                    Err(error) => {
                        tracing::warn!(container_id = id, error = %error, "failed to stop container")
                    }
                }
            }
        }
    }

    if let Err(error) = guest.stop() {
        tracing::warn!(error = %error, "VM stop error (continuing cleanup)");
    }
    let _ = std::fs::remove_file(sock_path);
    let _ = std::fs::remove_file(speck_home.join("run/speck.pid"));
    let _ = std::fs::remove_file(speck_home.join("run/control.sock"));
    Ok(())
}

/// Poll `sock_path` with `GET /_ping` until the Docker API responds with HTTP 200
/// or `timeout_secs` elapses. Progress dots are printed to stderr every 500 ms.
/// Returns an error (non-zero exit) on timeout.
pub async fn wait_for_socket(sock_path: &std::path::Path, timeout_secs: u64) -> anyhow::Result<()> {
    use std::time::{Duration, Instant};

    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let client = DockerClient::new(sock_path);

    eprintln!("Waiting for Speck to be ready (timeout: {timeout_secs}s)...");

    loop {
        if Instant::now() >= deadline {
            anyhow::bail!(
                "timed out after {timeout_secs}s waiting for {} — run `spk logs` for details",
                sock_path.display()
            );
        }

        if client.ping().await {
            eprintln!("Speck is ready.");
            return Ok(());
        }

        eprint!(".");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_up_uses_spawn_blocking_for_vm_start_and_ready_wait() {
        let source = include_str!("up.rs");

        let spawn_blocking = source
            .find("tokio::task::spawn_blocking")
            .expect("run_up should wrap blocking VM startup in tokio::task::spawn_blocking");
        let start = source
            .find("guest.start()")
            .expect("run_up should still start the guest");
        let wait = source
            .find(".wait_for_ready()")
            .expect("run_up should still wait for guest readiness");
        let speck_net = source
            .find("SpeckNet::new")
            .expect("run_up should still start SpeckNet after readiness");

        assert!(
            spawn_blocking < start,
            "guest.start() must be inside the blocking startup section"
        );
        assert!(
            start < wait,
            "guest.start() should happen before guest.wait_for_ready()"
        );
        assert!(
            wait < speck_net,
            "SpeckNet startup must remain after guest readiness"
        );
    }

    fn temp_disk_path(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("speck-up-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("data.img")
    }

    #[test]
    fn data_disk_reconcile_creates_requested_size() {
        let path = temp_disk_path("create");

        reconcile_data_disk(&path, 20).unwrap();

        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            20 * 1024 * 1024 * 1024
        );
    }

    #[test]
    fn data_disk_reconcile_grows_existing_image() {
        let path = temp_disk_path("grow");
        std::fs::File::create(&path)
            .unwrap()
            .set_len(10 * 1024 * 1024 * 1024)
            .unwrap();

        reconcile_data_disk(&path, 20).unwrap();

        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            20 * 1024 * 1024 * 1024
        );
    }

    #[test]
    fn data_disk_reconcile_rejects_shrink() {
        let path = temp_disk_path("shrink");
        std::fs::File::create(&path)
            .unwrap()
            .set_len(20 * 1024 * 1024 * 1024)
            .unwrap();

        let err = reconcile_data_disk(&path, 10).unwrap_err().to_string();

        assert!(err.starts_with("disk shrink not supported:"));
        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            20 * 1024 * 1024 * 1024
        );
    }

    #[test]
    fn check_asset_versions_allows_matching_version_files() {
        let dir =
            std::env::temp_dir().join(format!("speck-asset-version-match-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("initrd")).unwrap();
        std::fs::write(dir.join("rootfs.version"), format!("{ROOTFS_VERSION}\n")).unwrap();
        std::fs::write(
            dir.join("initrd/initrd.version"),
            format!("{INITRD_VERSION}\n"),
        )
        .unwrap();

        check_asset_versions(&dir).unwrap();
    }

    #[test]
    fn check_asset_versions_rejects_stale_rootfs_with_pull_hint() {
        let dir = std::env::temp_dir().join(format!(
            "speck-asset-version-rootfs-stale-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("initrd")).unwrap();
        std::fs::write(dir.join("rootfs.version"), "0.0.1\n").unwrap();
        std::fs::write(
            dir.join("initrd/initrd.version"),
            format!("{INITRD_VERSION}\n"),
        )
        .unwrap();

        let err = check_asset_versions(&dir).unwrap_err().to_string();

        assert!(
            err.contains("rootfs.version"),
            "error names stale file: {err}"
        );
        assert!(
            err.contains("0.0.1"),
            "error includes on-disk version: {err}"
        );
        assert!(
            err.contains(ROOTFS_VERSION),
            "error includes required version: {err}"
        );
        assert!(
            err.contains("spk up --pull"),
            "error includes pull hint: {err}"
        );
    }

    #[test]
    fn check_asset_versions_rejects_stale_initrd_with_pull_hint() {
        let dir = std::env::temp_dir().join(format!(
            "speck-asset-version-initrd-stale-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("initrd")).unwrap();
        std::fs::write(dir.join("rootfs.version"), format!("{ROOTFS_VERSION}\n")).unwrap();
        std::fs::write(dir.join("initrd/initrd.version"), "0.0.1\n").unwrap();

        let err = check_asset_versions(&dir).unwrap_err().to_string();

        assert!(
            err.contains("initrd/initrd.version"),
            "error names stale file: {err}"
        );
        assert!(
            err.contains("0.0.1"),
            "error includes on-disk version: {err}"
        );
        assert!(
            err.contains(INITRD_VERSION),
            "error includes required version: {err}"
        );
        assert!(
            err.contains("spk up --pull"),
            "error includes pull hint: {err}"
        );
    }

    #[test]
    fn run_up_checks_versions_unless_pull_or_full_overrides() {
        let source = include_str!("up.rs");
        let run_up_start = source.find("pub async fn run_up").unwrap();
        let tests_start = source.find("#[cfg(test)]").unwrap();
        let run_up = &source[run_up_start..tests_start];

        assert!(
            run_up.contains("check_asset_versions(speck_home)?"),
            "run_up must check asset versions before ensure_assets"
        );
        assert!(
            run_up.contains("!args.pull"),
            "--pull must bypass stale version checks"
        );
        assert!(
            run_up.contains("!has_all_overrides"),
            "full explicit asset overrides must bypass version checks"
        );
        assert!(
            run_up.contains("ensure_assets(speck_home, effective.vm.disk_gb, args.pull).await"),
            "--pull must force asset refresh through ensure_assets"
        );
    }

    #[test]
    fn run_up_no_longer_uses_fixed_512_mib_data_disk_creation() {
        let source = include_str!("up.rs");
        let run_up_start = source.find("pub async fn run_up").unwrap();
        let tests_start = source.find("#[cfg(test)]").unwrap();
        let run_up = &source[run_up_start..tests_start];

        assert!(
            run_up.contains("ensure_assets(speck_home, effective.vm.disk_gb, args.pull).await")
        );
        assert!(!run_up.contains("create_data_disk"));
        assert!(!run_up.contains("count=512"));
    }

    #[test]
    fn run_up_applies_effective_cpu_and_memory_to_guest_config() {
        let source = include_str!("up.rs");
        let run_up_start = source.find("pub async fn run_up").unwrap();
        let tests_start = source.find("#[cfg(test)]").unwrap();
        let run_up = &source[run_up_start..tests_start];
        let cpu = run_up
            .find(".cpu_count(effective.vm.cpus)")
            .expect("run_up must pass effective vm.cpus into GuestConfig::builder()");
        let memory = run_up
            .find(".memory_size_bytes(effective.vm.memory_mb * 1024 * 1024)")
            .expect("run_up must pass effective vm.memory_mb into GuestConfig::builder()");
        let build = run_up
            .find(".build()")
            .expect("run_up must build GuestConfig");

        assert!(cpu < build);
        assert!(memory < build);
    }

    #[test]
    fn resource_change_requires_restart_when_runtime_active() {
        let dir =
            std::env::temp_dir().join(format!("speck-up-resource-mismatch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let rootfs = dir.join("rootfs.img");
        let data = dir.join("data.img");
        std::fs::write(&rootfs, b"rootfs").unwrap();
        std::fs::write(&data, b"data").unwrap();
        write_vm_resource_snapshot(
            &dir,
            &crate::config::EffectiveVmConfig {
                cpus: 4,
                memory_mb: 2048,
                disk_gb: 20,
            },
        )
        .unwrap();

        let mut holder = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("exec 3<{}; sleep 5", data.display()))
            .spawn()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(250));

        let err = ensure_no_active_vm_resource_mismatch(
            &dir,
            &rootfs,
            &data,
            &crate::config::EffectiveVmConfig {
                cpus: 2,
                memory_mb: 2048,
                disk_gb: 20,
            },
        )
        .unwrap_err()
        .to_string();

        let _ = holder.kill();
        let _ = holder.wait();
        assert!(err.starts_with("VM resource change requires restart:"));
        assert!(err.contains("old cpus=4"));
        assert!(err.contains("new cpus=2"));
    }

    #[test]
    fn vm_resource_snapshot_round_trips_effective_vm_config() {
        let dir = std::env::temp_dir().join(format!("speck-up-snapshot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let vm = crate::config::EffectiveVmConfig {
            cpus: 3,
            memory_mb: 3072,
            disk_gb: 40,
        };

        write_vm_resource_snapshot(&dir, &vm).unwrap();

        assert_eq!(read_vm_resource_snapshot(&dir).unwrap(), Some(vm));
    }

    #[test]
    fn daemonize_writes_launchd_label_in_plist() {
        let source = include_str!("up.rs");

        assert!(
            source.contains("const LAUNCHD_LABEL: &str = \"io.speck.vm\";"),
            "daemonize must define the launchd label constant"
        );
        assert!(
            source.contains("<key>Label</key>")
                && source.contains("<string>{LAUNCHD_LABEL}</string>"),
            "daemonize plist must embed LAUNCHD_LABEL"
        );
    }

    #[test]
    fn daemonize_sets_speck_daemonized_env_var() {
        let source = include_str!("up.rs");

        assert!(
            source.contains("SPECK_DAEMONIZED") && source.contains("<string>1</string>"),
            "daemonize plist must export SPECK_DAEMONIZED=1"
        );
    }

    #[test]
    fn daemonize_includes_homebrew_in_path() {
        assert!(
            include_str!("up.rs").contains("/opt/homebrew/bin"),
            "daemonize plist PATH must include Homebrew binaries"
        );
    }

    #[test]
    fn daemonize_keepalive_true_runataload_true() {
        let source = include_str!("up.rs");

        assert!(source.contains("KeepAlive") && source.contains("<true/>"));
        assert!(
            source.contains("RunAtLoad") && source.contains("<true/>"),
            "RunAtLoad must be true so the daemon starts immediately on bootstrap"
        );
    }

    #[test]
    fn run_up_installs_sigterm_handler() {
        let source = include_str!("up.rs");
        let sigterm = source
            .find("SignalKind::terminate()")
            .expect("run_up must install a SIGTERM handler");
        let shutdown = source
            .find("shutdown_gracefully")
            .expect("run_up must call shutdown_gracefully");

        assert!(sigterm < shutdown);
    }

    #[test]
    fn run_up_binds_control_socket_with_0600() {
        let source = include_str!("up.rs");

        assert!(
            source.contains("run/control.sock") && source.contains("0o600"),
            "run_up must bind control.sock and lock it down to 0600"
        );
    }

    #[test]
    fn run_up_removes_stale_socket_before_bind() {
        let source = include_str!("up.rs");
        let run_up_start = source.find("pub async fn run_up").unwrap();
        let tests_start = source.find("#[cfg(test)]").unwrap();
        let run_up = &source[run_up_start..tests_start];
        let remove = run_up
            .find("remove_file(&ctrl_sock_path)")
            .expect("run_up must remove stale control.sock before bind");
        let bind = run_up
            .find("UnixListener::bind(&ctrl_sock_path)")
            .expect("run_up must bind the control socket");

        assert!(remove < bind);
    }

    #[test]
    fn shutdown_gracefully_stops_containers_before_vm() {
        let source = include_str!("up.rs");
        let containers = source
            .find("/containers/json")
            .expect("shutdown_gracefully must list running containers before stop");
        let guest_stop = source
            .find("guest.stop()")
            .expect("shutdown_gracefully must stop the VM");

        assert!(containers < guest_stop);
    }

    #[test]
    fn run_up_uses_shared_shell_renderer_for_exports() {
        let source = include_str!("up.rs");
        let run_up_start = source.find("pub async fn run_up").unwrap();
        let tests_start = source.find("#[cfg(test)]").unwrap();
        let run_up = &source[run_up_start..tests_start];

        assert!(
            run_up.contains("shell::render_env"),
            "run_up production code must call shell::render_env instead of hardcoding export strings"
        );
        assert!(
            run_up.contains("shell::render_speck_home"),
            "run_up production code must call shell::render_speck_home for the SPECK_HOME export"
        );
    }

    #[test]
    fn run_up_production_no_literal_docker_host_export() {
        let source = include_str!("up.rs");
        let run_up_start = source.find("pub async fn run_up").unwrap();
        let tests_start = source.find("#[cfg(test)]").unwrap();
        let run_up = &source[run_up_start..tests_start];

        assert!(
            !run_up.contains("export DOCKER_HOST="),
            "run_up production code must not hardcode `export DOCKER_HOST=`; use shell::render_env instead"
        );
        assert!(
            !run_up.contains("export SPECK_HOME="),
            "run_up production code must not hardcode `export SPECK_HOME=`; use shell::render_speck_home instead"
        );
    }

    #[test]
    fn run_up_includes_console_log_hint_on_ready_timeout() {
        let source = include_str!("up.rs");
        let run_up_start = source.find("pub async fn run_up").unwrap();
        let tests_start = source.find("#[cfg(test)]").unwrap();
        let run_up = &source[run_up_start..tests_start];

        assert!(
            run_up.contains("console_log_hint(speck_home)"),
            "run_up must build the GuestReadyTimeout hint from speck_home"
        );
        assert!(
            run_up.contains("console_log_hint"),
            "run_up must bind a console_log_hint and attach it to the wait_for_ready error"
        );
    }

    #[test]
    fn console_log_hint_includes_exact_speck_home_path() {
        let dir = std::env::temp_dir().join(format!("speck-console-hint-{}", std::process::id()));
        let hint = console_log_hint(&dir);
        let expected_path = dir.join("console.log");

        assert!(
            hint.contains(&expected_path.display().to_string()),
            "hint must include exact console log path {}; got {hint}",
            expected_path.display()
        );
    }

    #[test]
    fn wait_for_socket_is_exported() {
        let source = include_str!("up.rs");
        assert!(
            source.contains("pub async fn wait_for_socket("),
            "wait_for_socket must be pub async so main.rs can call it after daemonize"
        );
    }

    #[test]
    fn wait_for_socket_uses_ping_method() {
        let source = include_str!("up.rs");
        assert!(
            source.contains("client.ping()"),
            "wait_for_socket must use DockerClient::ping() to check readiness"
        );
    }

    #[test]
    fn wait_for_socket_prints_timeout_message() {
        let source = include_str!("up.rs");
        assert!(
            source.contains("spk logs"),
            "wait_for_socket timeout error must direct the user to spk logs"
        );
    }

    #[tokio::test]
    async fn wait_for_socket_times_out_when_socket_absent() {
        let dir = std::env::temp_dir().join(format!("speck-wait-timeout-{}", std::process::id()));
        let sock = dir.join("speck.sock");

        let err = wait_for_socket(&sock, 1).await.unwrap_err().to_string();

        assert!(
            err.contains("timed out after 1s"),
            "expected timeout, got: {err}"
        );
        assert!(
            err.contains("spk logs"),
            "timeout error must reference spk logs"
        );
    }

    #[tokio::test]
    async fn wait_for_socket_succeeds_when_socket_ready() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        use tokio::net::UnixListener;

        let dir = std::env::temp_dir().join(format!("speck-wait-ready-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sock_path = dir.join("speck.sock");
        let _ = std::fs::remove_file(&sock_path);

        let listener = UnixListener::bind(&sock_path).unwrap();
        let sock_path_clone = sock_path.clone();

        // Serve a minimal HTTP/1.1 200 OK: read the request first so hyper doesn't
        // see an empty read before it finishes sending the request headers.
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = vec![0u8; 4096];
                let _ = stream.read(&mut buf).await;
                let _ = stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK",
                    )
                    .await;
            }
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let result = wait_for_socket(&sock_path_clone, 10).await;
        let _ = std::fs::remove_file(&sock_path_clone);

        assert!(
            result.is_ok(),
            "wait_for_socket should succeed when socket serves HTTP 200"
        );
    }

    #[test]
    fn control_socket_handles_prepare_restart() {
        let source = include_str!("up.rs");
        let run_up_start = source.find("pub async fn run_up").unwrap();
        let tests_start = source.find("#[cfg(test)]").unwrap();
        let run_up = &source[run_up_start..tests_start];

        assert!(
            run_up.contains("PREPARE_RESTART"),
            "control socket handler must read and dispatch PREPARE_RESTART command"
        );
        assert!(
            run_up.contains("VmState::Restarting"),
            "control socket handler must set VmState::Restarting on PREPARE_RESTART"
        );
    }

    #[test]
    fn control_socket_does_not_break_on_accept_error() {
        let source = include_str!("up.rs");
        let tests_start = source.find("#[cfg(test)]").unwrap_or(source.len());
        let production = &source[..tests_start];

        // Find the accept loop: look for listener.accept() in production code
        let accept_start = production
            .rfind("listener.accept()")
            .expect("production code must have a control socket accept loop");
        let accept_section = &production[accept_start..];

        // The accept loop must NOT break on error
        // Find the accept error handler by looking for the unique "accept error" message
        let accept_err_msg = accept_section
            .find("accept error")
            .expect("accept loop must have an error handler with accept error message");
        // Walk back from the message to find the Err(e) line
        let err_stanza = &accept_section[..accept_err_msg];
        let err_start = err_stanza
            .rfind("Err(e)")
            .expect("accept loop must have an Err(e) handler");
        let err_handler = &accept_section[err_start..][..200];

        assert!(
            !err_handler.contains("break;"),
            "control socket accept loop must NOT break on error: found break in {err_handler:?}"
        );
        assert!(
            err_handler.contains("continue;"),
            "control socket accept loop must continue on error: missing continue in {err_handler:?}"
        );
        assert!(
            err_handler.contains("sleep"),
            "control socket accept loop must have backoff sleep on error: missing sleep in {err_handler:?}"
        );
    }

    #[test]
    fn control_socket_read_errors_are_logged() {
        let source = include_str!("up.rs");
        let tests_start = source.find("#[cfg(test)]").unwrap_or(source.len());
        let production = &source[..tests_start];

        let read_start = production
            .rfind("stream.read(&mut buf)")
            .expect("production code must read from control socket");
        let read_section = &production[read_start..][..15];

        assert!(
            !read_section.contains("unwrap_or(0)"),
            "control socket read must not silently swallow errors with unwrap_or(0)"
        );
        assert!(
            production.contains("tracing::warn!(\"control socket read error"),
            "control socket read errors must be logged via tracing::warn!"
        );
    }

    #[test]
    fn shutdown_gracefully_removes_control_sock() {
        let source = include_str!("up.rs");
        let tests_start = source.find("#[cfg(test)]").unwrap_or(source.len());
        let production = &source[..tests_start];

        let shutdown_start = production
            .rfind("async fn shutdown_gracefully")
            .expect("production code must define shutdown_gracefully");
        let shutdown_section = &production[shutdown_start..];

        assert!(
            shutdown_section.contains("remove_file(sock_path)"),
            "shutdown_gracefully must remove the Docker API sock_path"
        );
        assert!(
            shutdown_section.contains("run/speck.pid"),
            "shutdown_gracefully must remove the PID file"
        );
        assert!(
            shutdown_section.contains("run/control.sock"),
            "shutdown_gracefully must remove the control socket at run/control.sock"
        );
    }

    #[test]
    fn restarting_state_has_timeout_lease() {
        let source = include_str!("up.rs");
        let tests_start = source.find("#[cfg(test)]").unwrap_or(source.len());
        let production = &source[..tests_start];

        let prepare_start = production
            .rfind("PREPARE_RESTART")
            .expect("production code must handle PREPARE_RESTART command");
        let prepare_section = &production[prepare_start..][..800];

        assert!(
            prepare_section.contains("Instant::now()") || prepare_section.contains("Instant::now"),
            "PREPARE_RESTART handler must record lease start time with Instant::now"
        );
        assert!(
            prepare_section.contains("tokio::spawn"),
            "PREPARE_RESTART handler must spawn a background lease timeout task"
        );
        assert!(
            prepare_section.contains("Duration::from_secs(60)")
                || prepare_section.contains("Duration::from_secs"),
            "lease timeout must be Duration::from_secs(60)"
        );
        assert!(
            prepare_section.contains("Restarting"),
            "lease timeout reset check must reference Restarting state"
        );
    }
}
