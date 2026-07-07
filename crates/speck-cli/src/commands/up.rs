use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use anyhow::Context as _;
use indicatif::ProgressBar;
use speck_dockerd::SpeckDockerd;
use speck_net::config::NetworkConfig;
use speck_vz::config::GuestConfig;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixListener;
use tokio::signal::unix::{SignalKind, signal};

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
    <false/>
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
    Ok(())
}

/// Ensure all VM assets (kernel, initrd, rootfs, data disk) are present in
/// `speck_home`, downloading them from GitHub Releases if not.
///
/// This makes `spk up` self-bootstrapping: first run works with no separate
/// init step, exactly like `colima start`.
async fn ensure_assets(speck_home: &Path, requested_disk_gb: u64) -> anyhow::Result<()> {
    std::fs::create_dir_all(speck_home.join("kernel"))?;
    std::fs::create_dir_all(speck_home.join("initrd"))?;

    let kernel = speck_home.join("kernel/vmlinux");
    let initrd = speck_home.join(format!("initrd/{KATA_INITRD_FILE}"));
    let rootfs = speck_home.join("rootfs.img");
    let data = speck_home.join("data.img");

    if !kernel.exists() || !initrd.exists() {
        fetch_kata_assets(speck_home)
            .await
            .context("failed to download kernel + initrd")?;
    }

    let custom_initrd = speck_home.join("initrd/initrd.cpio.gz");
    if !custom_initrd.exists() {
        fetch_initrd(speck_home)
            .await
            .context("failed to download initrd")?;
    }

    if !rootfs.exists() {
        fetch_rootfs(speck_home)
            .await
            .context("failed to download rootfs")?;
    }

    reconcile_data_disk(&data, requested_disk_gb).context("failed to reconcile data disk")?;

    Ok(())
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
    let status = tokio::process::Command::new("bash")
        .args([
            "-c",
            &format!(
                r#"curl -fsSL {url} | zstdcat -c | tar -C /tmp/speck-kata-assets-{pid} \
                    --strip-components=5 -xf - \
                    ./opt/kata/share/kata-containers/{KATA_KERNEL_FILE} \
                    ./opt/kata/share/kata-containers/{KATA_INITRD_FILE}"#,
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
    let base =
        format!("https://github.com/wasilak/speck/releases/download/initrd-{INITRD_VERSION}");
    let name = format!("speck-initrd-{INITRD_VERSION}-arm64.cpio.gz");

    println!("  Downloading initrd {INITRD_VERSION}...");

    let status = tokio::process::Command::new("curl")
        .args(["-fsSL", "--progress-bar", &format!("{base}/{name}")])
        .arg("-o")
        .arg(&dest)
        .status()
        .await
        .context("curl failed")?;
    anyhow::ensure!(status.success(), "initrd download failed");

    println!("  Initrd ready: {}", dest.display());
    Ok(())
}

/// Download `speck-rootfs-{VERSION}-{BACKEND}-arm64.img.gz` from GitHub
/// Releases, verify its SHA-256 checksum, and decompress to `rootfs.img`.
async fn fetch_rootfs(speck_home: &Path) -> anyhow::Result<()> {
    let dest = speck_home.join("rootfs.img");
    let tmp = speck_home.join("rootfs.img.tmp");
    let gz_tmp = speck_home.join("rootfs.img.gz.tmp");
    let base =
        format!("https://github.com/wasilak/speck/releases/download/rootfs-{ROOTFS_VERSION}");
    let gz_name = format!("speck-rootfs-{ROOTFS_VERSION}-{ROOTFS_BACKEND}-arm64.img.gz");
    let sum_name = format!("speck-rootfs-{ROOTFS_VERSION}-{ROOTFS_BACKEND}-arm64.img.sha256");

    println!("  Downloading rootfs {ROOTFS_VERSION} ({ROOTFS_BACKEND})...");

    let curl_status = tokio::process::Command::new("curl")
        .args(["-fsSL", "--progress-bar", &format!("{base}/{gz_name}")])
        .arg("-o")
        .arg(&gz_tmp)
        .status()
        .await
        .context("curl failed")?;
    anyhow::ensure!(curl_status.success(), "rootfs download failed");

    let tmp_file = std::fs::File::create(&tmp)
        .with_context(|| format!("failed to create {}", tmp.display()))?;
    let status = tokio::process::Command::new("gunzip")
        .arg("-c")
        .arg(&gz_tmp)
        .stdout(Stdio::from(tmp_file))
        .status()
        .await
        .context("gunzip failed")?;
    anyhow::ensure!(status.success(), "rootfs download failed");
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

    let actual_out = tokio::process::Command::new("shasum")
        .args(["-a", "256"])
        .arg(&tmp)
        .output()
        .await
        .context("shasum failed")?;
    let actual = std::str::from_utf8(&actual_out.stdout)
        .ok()
        .and_then(|s| s.split_whitespace().next())
        .context("shasum produced no output")?
        .to_string();

    anyhow::ensure!(
        expected == actual,
        "rootfs SHA-256 mismatch (expected {expected}, got {actual})"
    );

    std::fs::rename(&tmp, &dest)?;
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

    // Auto-download kernel, initrd, rootfs, and data disk on first run.
    ensure_assets(speck_home, effective.vm.disk_gb).await?;

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
        .cmdline("console=hvc0 panic=-1 container_backend=dockerd docker_vsock_port=9003 ready_vsock_port=9000 dns_vsock_port=53 speck_guest_ip=172.16.0.2 speck_gateway=172.16.0.1")
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

    let spinner_for_start = spinner.clone();
    let guest = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        guest.start().context("failed to start VM")?;
        spinner_for_start.set_message("Waiting for guest...");
        guest
            .wait_for_ready()
            .context("guest did not become ready")?;
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
    let _dockerd = SpeckDockerd::start(guest.clone(), sock_path.clone())
        .context("failed to start Docker API server")?;
    tracing::info!("Docker API server started at {}", sock_path.display());

    spinner.finish_with_message(format!("{NEON_CYAN}Speck is running{RESET}"));

    println!();
    print!("{}", shell::render_env(speck_home, EnvShell::Posix));
    print!("{}", shell::render_speck_home(speck_home, EnvShell::Posix));
    println!();

    std::fs::create_dir_all(speck_home.join("run"))?;
    let ctrl_sock_path = speck_home.join("run/control.sock");
    let _ = std::fs::remove_file(&ctrl_sock_path);
    let listener = UnixListener::bind(&ctrl_sock_path).context("failed to bind control socket")?;
    std::fs::set_permissions(&ctrl_sock_path, std::fs::Permissions::from_mode(0o600))?;
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((mut stream, _)) => {
                    tokio::spawn(async move {
                        let _ = stream.write_all(b"PONG\n").await;
                    });
                }
                Err(e) => {
                    tracing::warn!("control socket accept error: {e}");
                    break;
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
    Ok(())
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
    fn run_up_no_longer_uses_fixed_512_mib_data_disk_creation() {
        let source = include_str!("up.rs");
        let run_up_start = source.find("pub async fn run_up").unwrap();
        let tests_start = source.find("#[cfg(test)]").unwrap();
        let run_up = &source[run_up_start..tests_start];

        assert!(run_up.contains("ensure_assets(speck_home, effective.vm.disk_gb).await"));
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
    fn daemonize_keepalive_true_runataload_false() {
        let source = include_str!("up.rs");

        assert!(source.contains("KeepAlive") && source.contains("<true/>"));
        assert!(source.contains("RunAtLoad") && source.contains("<false/>"));
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
}
