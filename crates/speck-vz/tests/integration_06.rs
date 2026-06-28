//! Phase 6 end-to-end integration tests (DOCKER-03, DOCKER-04, RUN-01).
//!
//! Run with: `cargo test -p speck-vz --test integration_06 -- --include-ignored`
//!
//! These tests require:
//!   - A codesigned binary with `com.apple.security.virtualization` entitlement
//!   - `$SPECK_HOME/kernel/vmlinux` (Kata arm64 kernel)
//!   - `$SPECK_HOME/initrd/initrd.cpio.gz` (vminitd static binary)
//!   - `$SPECK_HOME/rootfs/rootfs.img` and `$SPECK_HOME/rootfs/data.img`
//!   - An active `spk up` session (or the tests start one internally)

use std::path::PathBuf;
use std::time::Duration;

fn speck_home() -> PathBuf {
    if let Ok(home) = std::env::var("SPECK_HOME") {
        return PathBuf::from(home);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".local/share/speck")
}

fn build_guest_config(speck_home: &PathBuf) -> speck_vz::config::GuestConfig {
    speck_vz::config::GuestConfig::builder()
        .kernel_path(speck_home.join("kernel/vmlinux"))
        .initrd_path(speck_home.join("initrd/initrd.cpio.gz"))
        .rootfs_disk_path(speck_home.join("rootfs/rootfs.img"))
        .data_disk_path(speck_home.join("rootfs/data.img"))
        .cmdline(
            "console=hvc0 quiet panic=-1 \
             ready_vsock_port=9000 \
             containerd_vsock_port=9001 \
             buildkitd_vsock_port=9002",
        )
        .cpu_count(2)
        .memory_size_bytes(2 * 1024 * 1024 * 1024)
        .stop_timeout(Duration::from_secs(15))
        .ready_vsock_port(9000)
        .containerd_vsock_port(9001)
        .buildkitd_vsock_port(9002)
        .build()
}

/// Verify that `spk` can boot a VM, pull Alpine, and run a container with output.
///
/// Chain: VM boots → vminitd sends READY → containerd gRPC reachable → pull alpine →
/// create + start + wait → collect logs → assert expected output.
#[test]
#[ignore = "requires signed binary + speck_home artifacts + running spk up"]
fn test_spk_run_alpine() {
    let home = speck_home();
    let config = build_guest_config(&home);
    let guest = speck_vz::Guest::new(config);
    guest.start().expect("VM should start");
    guest.wait_for_ready().expect("wait_for_ready");

    let sock_path = guest
        .containerd_unix_proxy()
        .expect("containerd unix proxy");

    let _sock_display = sock_path.display().to_string();

    // TODO: In a full test, use bollard::Docker::connect_with_unix to:
    //   1. Pull alpine:latest
    //   2. Create container with Cmd=["echo", "integration-06-hello"]
    //   3. Start + wait + collect logs
    //   4. Assert "integration-06-hello" in log output
    //   5. Cleanup
    //
    // This requires the SpeckDockerd to be running and listening on a Unix socket.
    // The test currently validates the VM boots, vminitd starts, and containerd is
    // reachable via vsock proxy. Full bollard integration is added once the
    // SpeckDockerd lifecycle is wired into the test.

    guest.stop().expect("VM should stop");
}

/// Verify that the Ryuk-compatible `/var/run/docker.sock` symlink is accessible
/// inside a container (DOCKER-04, D-14).
///
/// Creates a container with a VirtioFS volume mount mapping the Speck home
/// directory to `/var/run/`. Inside the container, `/var/run/docker.sock` should
/// exist as a symlink to `speck.sock` (set up by vminitd on the guest).
#[test]
#[ignore = "requires signed binary + speck_home artifacts + VirtioFS support"]
fn test_ryuk_socket_accessible() {
    let home = speck_home();
    let config = build_guest_config(&home);
    let guest = speck_vz::Guest::new(config);
    guest.start().expect("VM should start");
    guest.wait_for_ready().expect("wait_for_ready");

    // TODO: In a full test, use bollard to:
    //   1. Create container with HostConfig.Binds = ["<speck_home>:/var/run/"]
    //   2. Exec inside: ls -la /var/run/docker.sock
    //   3. Assert output shows the symlink exists (no "No such file" in output)
    //
    // This validates D-14: the docker.sock → speck.sock symlink resolves inside
    // a container via VirtioFS mount, allowing Ryuk (and other tools like
    // testcontainers) to discover the Docker socket at the expected path.

    guest.stop().expect("VM should stop");
}

/// Simulate a minimal `docker compose` lifecycle (DOCKER-03).
///
/// Pre-flight checks that compose would perform: create network, create volume,
/// verify events stream works, pull image, create/start/wait container, then
/// cleanup by deleting the network and volume.
#[test]
#[ignore = "requires signed binary + speck_home artifacts"]
fn test_docker_compose_like_lifecycle() {
    let home = speck_home();
    let config = build_guest_config(&home);
    let guest = speck_vz::Guest::new(config);
    guest.start().expect("VM should start");
    guest.wait_for_ready().expect("wait_for_ready");

    // TODO: In a full test, use bollard to simulate compose lifecycle:
    //   1. POST /networks/create {"Name": "compose-test-net"}
    //   2. POST /volumes/create {"Name": "compose-test-vol"}
    //   3. GET /events (check stream starts)
    //   4. Pull alpine; create container with Env=["TEST_KEY=test_value"]; start; wait
    //   5. Assert exit code 0
    //   6. DELETE network + volume
    //   7. Cleanup

    guest.stop().expect("VM should stop");
}
