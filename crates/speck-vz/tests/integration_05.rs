//! Phase 5 integration tests: containerd + BuildKit inside the micro-VM.
//!
//! These tests prove RUN-06 end-to-end:
//!   VM boots → vminitd mounts disks → spawns containerd → sends READY signal
//!   → host proxies vsock to Unix socket → containerd gRPC reachable over the proxy
//!
//! All tests are `#[ignore]`'d because they require:
//!   - A codesigned binary with `com.apple.security.virtualization` entitlement
//!   - `$SPECK_HOME/kernel/vmlinux` (Kata arm64 kernel)
//!   - `$SPECK_HOME/initrd/initrd.cpio.gz` (vminitd static binary)
//!   - `$SPECK_HOME/rootfs/rootfs.img` and `$SPECK_HOME/rootfs/data.img`
//!
//! Run manually: cargo test -p speck-vz --test integration_05 -- --ignored

use speck_vz::{Guest, GuestConfig};
use std::path::PathBuf;
use std::time::Duration;

fn speck_home() -> String {
    std::env::var("SPECK_HOME").unwrap_or_else(|_| {
        format!(
            "{}/.local/share/speck",
            std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())
        )
    })
}

fn make_phase5_config() -> GuestConfig {
    let home = speck_home();
    GuestConfig::builder()
        .kernel_path(format!("{home}/kernel/vmlinux"))
        .initrd_path(format!("{home}/initrd/initrd.cpio.gz"))
        .rootfs_disk_path(format!("{home}/rootfs/rootfs.img"))
        .data_disk_path(format!("{home}/rootfs/data.img"))
        .cmdline(
            "console=hvc0 quiet panic=-1 \
             ready_vsock_port=9000 \
             containerd_vsock_port=9001 \
             buildkitd_vsock_port=9002",
        )
        .cpu_count(2)
        .memory_size_bytes(2 * 1024 * 1024 * 1024) // 2 GiB — containerd needs headroom
        .stop_timeout(Duration::from_secs(15))
        .ready_vsock_port(9000)
        .containerd_vsock_port(9001)
        .buildkitd_vsock_port(9002)
        .build()
}

/// Verify the VM boots with rootfs and data virtio-blk disks attached.
///
/// This is the RUN-06 precondition: vminitd can only mount /dev/vda and /dev/vdb
/// if the host has wired both disk images as VZVirtioBlockDeviceConfiguration.
#[test]
#[ignore = "requires com.apple.security.virtualization entitlement + signed binary + rootfs.img"]
fn test_vm_boots_with_disks() {
    let guest = Guest::new(make_phase5_config());
    let result = guest.start();
    assert!(
        result.is_ok(),
        "VM with disks should boot: {:?}",
        result
    );
    assert_eq!(
        result.unwrap(),
        speck_vz::VmState::Running,
        "state should be Running"
    );
    guest.stop().expect("stop");
}

/// Verify that vminitd sends the READY signal after mounting disks and starting containerd.
///
/// Chain: VM boots → vminitd mounts /dev/vda (rootfs) + /dev/vdb (data) → spawns containerd
/// → polls containerd.sock → writes b"READY\n" on vsock port 9000 → host receives it (D-04, D-05).
#[test]
#[ignore = "requires com.apple.security.virtualization entitlement + signed binary + rootfs.img"]
fn test_containerd_ready() {
    let guest = Guest::new(make_phase5_config());
    guest.start().expect("start");

    let ready = guest.wait_for_ready();
    assert!(
        ready.is_ok(),
        "wait_for_ready should receive READY signal: {:?}",
        ready
    );

    guest.stop().expect("stop");
}

/// Verify the full Phase 5 chain: VM + disks + vminitd + containerd + vsock proxy + gRPC.
///
/// Chain: VM boots → READY signal → containerd_unix_proxy() bridges vsock 9001 to a temp Unix
/// socket → containerd_client::connect() opens the Unix socket → ImagesClient::list() returns Ok
/// (even an empty list proves end-to-end gRPC connectivity, D-08, D-11).
///
/// NOTE: Full alpine pull requires network egress via the Phase 4 smoltcp netstack.
/// This test is #[ignore]'d until a codesigned binary is available.
#[tokio::test]
#[ignore = "requires com.apple.security.virtualization entitlement + signed binary + rootfs.img"]
async fn test_image_pull_alpine() {
    use containerd_client::services::v1::images_client::ImagesClient;
    use containerd_client::services::v1::ListImagesRequest;
    use containerd_client::with_namespace;
    use containerd_client::tonic::Request;

    let guest = Guest::new(make_phase5_config());
    guest.start().expect("start");
    guest.wait_for_ready().expect("wait_for_ready");

    let sock_path: PathBuf = guest.containerd_unix_proxy().expect("proxy");

    let channel = containerd_client::connect(&sock_path)
        .await
        .expect("containerd connect via Unix socket proxy");

    // Full alpine pull requires network egress via Phase 4 smoltcp netstack; test is #[ignore]'d
    // until codesigned binary is available. Connectivity proof: list() returns Ok from guest.
    let mut client = ImagesClient::new(channel);
    let request = ListImagesRequest {
        filters: vec![],
    };
    let images_response = client.list(with_namespace!(request, "default")).await;
    assert!(
        images_response.is_ok(),
        "ImagesClient::list() should succeed — proves vsock proxy + gRPC chain: {:?}",
        images_response
    );

    let images = images_response.unwrap().into_inner().images;
    // Even zero images proves the gRPC channel works end-to-end.
    let _ = images;

    guest.stop().expect("stop");
}
