use std::path::Path;

use anyhow::Context as _;
use indicatif::ProgressBar;
use speck_net::config::NetworkConfig;
use speck_vz::config::GuestConfig;

use crate::UpArgs;
use crate::theme::{NEON_CYAN, RESET};

pub async fn run_up(args: UpArgs, speck_home: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(speck_home)
        .with_context(|| format!("failed to create speck_home directory: {}", speck_home.display()))?;

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
        // Podman backend spike (phase 06.2): instruct vminitd to start Podman
        // instead of containerd and forward its Docker-compatible API socket on
        // vsock port 9003.  Remove this block to revert to containerd.
        .podman_vsock_port(9003)
        .cmdline("container_backend=podman podman_vsock_port=9003")
        .speck_home(speck_home)
        .network(NetworkConfig::default())
        .dns_vsock_port(53)
        .build();

    let guest = speck_vz::Guest::new(config);
    let spinner = ProgressBar::new_spinner();
    spinner.set_message("Starting VM...");
    spinner.enable_steady_tick(std::time::Duration::from_millis(100));

    guest.start()?;
    spinner.set_message("Waiting for guest...");
    guest.wait_for_ready()?;

    let (port_map_tx, port_map_rx) =
        tokio::sync::mpsc::channel::<speck_net::PortMapConfig>(64);
    guest.set_port_map_channel(port_map_tx)?;
    let netstack_fd = guest.netstack_fd()?;
    let dns_vsock_fd = guest.dns_vsock_fd().ok();
    let net_config = speck_net::config::NetworkConfig::default();
    let _netstack_handles = speck_net::SpeckNet::new(net_config, Some(53))
        .spawn(netstack_fd, dns_vsock_fd, vec![], Some(port_map_rx));

    spinner.set_message("Starting Docker API proxy...");
    let sock_path = speck_home.join("speck.sock");
    // Podman backend spike (phase 06.2): bypass speck-dockerd; proxy Unix socket traffic
    // directly to guest Podman over vsock.  speck-dockerd is retained for comparison.
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
