use std::path::Path;
use std::process::Stdio;

use anyhow::Context as _;
use indicatif::ProgressBar;
use speck_net::config::NetworkConfig;
use speck_vz::config::GuestConfig;

use crate::config::EffectiveConfig;
use crate::UpArgs;
use crate::theme::{NEON_CYAN, RESET};

const ROOTFS_VERSION: &str = "0.2.0";
const ROOTFS_BACKEND: &str = "moby";
const KATA_VERSION: &str = "3.32.0";
const KATA_KERNEL_FILE: &str = "vmlinux-6.18.35-197";
const KATA_INITRD_FILE: &str = "kata-alpine-3.22.initrd";

/// Ensure all VM assets (kernel, initrd, rootfs, data disk) are present in
/// `speck_home`, downloading them from GitHub Releases if not.
///
/// This makes `spk up` self-bootstrapping: first run works with no separate
/// init step, exactly like `colima start`.
async fn ensure_assets(speck_home: &Path) -> anyhow::Result<()> {
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

    if !rootfs.exists() {
        fetch_rootfs(speck_home)
            .await
            .context("failed to download rootfs")?;
    }

    if !data.exists() {
        create_data_disk(&data).context("failed to create data disk")?;
    }

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

/// Create a blank 512 MiB ext4 data disk image.
fn create_data_disk(path: &Path) -> anyhow::Result<()> {
    let tmp = path.with_extension("img.tmp");
    println!("  Creating data disk...");

    let status = std::process::Command::new("dd")
        .args([
            "if=/dev/zero",
            &format!("of={}", tmp.display()),
            "bs=1M",
            "count=512",
        ])
        .status()
        .context("dd failed")?;
    anyhow::ensure!(status.success(), "dd failed creating data disk");

    std::fs::rename(&tmp, path)?;
    println!("  Data disk ready: {}", path.display());
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
    _effective: EffectiveConfig,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(speck_home).with_context(|| {
        format!(
            "failed to create speck_home directory: {}",
            speck_home.display()
        )
    })?;

    // Auto-download kernel, initrd, rootfs, and data disk on first run.
    ensure_assets(speck_home).await?;

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

    kill_stale_vm_holders(&[&rootfs_disk_path, &data_disk_path]);

    let mut builder = GuestConfig::builder()
        .kernel_path(kernel_path)
        .initrd_path(initrd_path)
        .rootfs_disk_path(rootfs_disk_path)
        .data_disk_path(data_disk_path)
        .containerd_vsock_port(9001)
        .buildkitd_vsock_port(9002)
        .ready_vsock_port(9000)
        .docker_vsock_port(9003)
        .cmdline("console=hvc0 panic=-1 container_backend=dockerd docker_vsock_port=9003 ready_vsock_port=9000 dns_vsock_port=53 speck_guest_ip=172.16.0.2 speck_gateway=172.16.0.1")
        .speck_home(speck_home)
        .network(NetworkConfig::default())
        .dns_vsock_port(53)
        .add_identity_mount("/Users");

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

    let (port_map_tx, port_map_rx) = tokio::sync::mpsc::channel::<speck_net::PortMapConfig>(64);
    guest.set_port_map_channel(port_map_tx)?;
    let netstack_fd = guest.netstack_fd()?;
    let dns_vsock_fd = match guest.connect_dns_vsock(53) {
        Ok(fd) => {
            eprintln!("[spk] DNS vsock connected, fd={fd}");
            Some(fd)
        }
        Err(e) => {
            eprintln!("[spk] DNS vsock connect failed: {e}");
            None
        }
    };
    let net_config = speck_net::config::NetworkConfig::default();
    let _netstack_handles = speck_net::SpeckNet::new(net_config, Some(53)).spawn(
        netstack_fd,
        dns_vsock_fd,
        vec![],
        Some(port_map_rx),
    );

    spinner.set_message("Starting Docker API proxy...");
    let sock_path = speck_home.join("speck.sock");
    let _docker_proxy_path = guest.docker_api_unix_proxy(sock_path.clone())?;

    spinner.finish_with_message(format!("{NEON_CYAN}Speck is running{RESET}"));

    println!();
    println!("export DOCKER_HOST=unix://{}", sock_path.display());
    println!("export SPECK_HOME={}", speck_home.display());
    println!();

    tokio::signal::ctrl_c().await?;
    println!("\nShutting down...");

    Ok(())
}

#[cfg(test)]
mod tests {
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
}
