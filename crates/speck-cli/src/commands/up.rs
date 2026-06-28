use std::path::Path;
use std::sync::Arc;

use indicatif::ProgressBar;
use speck_vz::config::GuestConfig;

use crate::UpArgs;
use crate::theme::{NEON_CYAN, RESET};

pub async fn run_up(args: UpArgs, speck_home: &Path) -> anyhow::Result<()> {
    let kernel_path = args
        .kernel
        .clone()
        .unwrap_or_else(|| speck_home.join("vmlinuz"));
    let initrd_path: std::path::PathBuf = args
        .initrd
        .clone()
        .or_else(|| {
            let p = speck_home.join("initrd");
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

    let config = GuestConfig::builder()
        .kernel_path(kernel_path)
        .initrd_path(initrd_path)
        .rootfs_disk_path(rootfs_disk_path)
        .data_disk_path(data_disk_path)
        .containerd_vsock_port(9001)
        .buildkitd_vsock_port(9002)
        .ready_vsock_port(9000)
        .build();

    let guest = speck_vz::Guest::new(config);
    let spinner = ProgressBar::new_spinner();
    spinner.set_message("Starting VM...");
    spinner.enable_steady_tick(std::time::Duration::from_millis(100));

    guest.start()?;
    spinner.set_message("Waiting for guest...");
    guest.wait_for_ready()?;

    spinner.set_message("Starting Docker API server...");
    let sock_path = speck_home.join("speck.sock");
    let _dockerd = speck_dockerd::SpeckDockerd::start(Arc::new(guest), sock_path.clone())?;

    spinner.finish_with_message(format!("{NEON_CYAN}Speck is running{RESET}"));

    println!();
    println!("export DOCKER_HOST=unix://{}", sock_path.display());
    println!("export SPECK_HOME={}", speck_home.display());
    println!();

    tokio::signal::ctrl_c().await?;
    println!("\nShutting down...");

    Ok(())
}
