//! Task 9 integration tests: guest serial console capture to console.log.
//!
//! These tests verify that the hvc0 serial console output is captured to
//! $SPECK_HOME/console.log during VM lifetime, covering both successful
//! and failed boots.
//!
//! All tests are `#[ignore]`'d because they require:
//!   - A codesigned binary with `com.apple.security.virtualization` entitlement
//!   - `$SPECK_HOME/kernel/vmlinux` (Kata arm64 kernel)
//!   - `$SPECK_HOME/initrd/initrd.cpio.gz` (vminitd static binary)
//!   - `$SPECK_HOME/rootfs.img` and `$SPECK_HOME/data.img`
//!
//! Run manually: cargo test -p speck-vz --test integration_09 -- --ignored

use speck_vz::{Guest, GuestConfig};
use std::path::PathBuf;
use std::time::Duration;

fn speck_home() -> PathBuf {
    std::env::var("SPECK_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
            PathBuf::from(home).join(".speck")
        })
}

fn make_console_test_config() -> GuestConfig {
    let home = speck_home();
    GuestConfig::builder()
        .kernel_path(home.join("kernel/vmlinux"))
        .initrd_path(home.join("initrd/initrd.cpio.gz"))
        .rootfs_disk_path(home.join("rootfs.img"))
        .data_disk_path(home.join("data.img"))
        .speck_home(&home)
        .cmdline(
            "console=hvc0 panic=-1 \
             ready_vsock_port=9000 \
             containerd_vsock_port=9001 \
             buildkitd_vsock_port=9002 \
             docker_vsock_port=9003",
        )
        .cpu_count(2)
        .memory_size_bytes(2 * 1024 * 1024 * 1024)
        .stop_timeout(Duration::from_secs(15))
        .ready_vsock_port(9000)
        .containerd_vsock_port(9001)
        .buildkitd_vsock_port(9002)
        .docker_vsock_port(9003)
        .build()
}

/// After a successful VM boot, console.log must exist and contain kernel messages.
#[test]
#[ignore = "requires VM entitlement + kernel + initrd + rootfs artifacts"]
fn console_log_exists_with_kernel_messages_after_boot() {
    let home = speck_home();
    let console_log = home.join("console.log");

    // Remove any stale log from a previous run — do_start truncates, but make sure
    // we don't accidentally pass because of an old file.
    let _ = std::fs::remove_file(&console_log);

    let config = make_console_test_config();
    let guest = Guest::new(config);
    guest.start().expect("VM start");
    guest.wait_for_ready().expect("VM ready");

    // console.log must exist and contain at least some kernel boot output.
    assert!(
        console_log.exists(),
        "console.log must be created at {}",
        console_log.display()
    );

    let content = std::fs::read_to_string(&console_log).expect("failed to read console.log");

    assert!(
        !content.is_empty(),
        "console.log must not be empty after a successful boot"
    );
    assert!(
        content.contains("Linux") || content.contains("Booting") || content.contains("kernel"),
        "console.log must contain recognizable kernel boot messages, got first 200 chars: {:?}",
        &content[..content.len().min(200)]
    );

    guest.stop().expect("VM stop");
}

/// console.log must be truncated at the start of each VM boot (not appended across reboots).
#[test]
#[ignore = "requires VM entitlement + kernel + initrd + rootfs artifacts"]
fn console_log_is_truncated_on_each_vm_start() {
    let home = speck_home();
    let console_log = home.join("console.log");

    // Write a sentinel that must NOT appear in the second boot's log.
    std::fs::write(&console_log, b"STALE_SENTINEL_FROM_PREVIOUS_RUN\n").expect("write sentinel");

    let config = make_console_test_config();

    // First boot
    let guest = Guest::new(config.clone());
    guest.start().expect("first VM start");
    guest.wait_for_ready().expect("first VM ready");
    guest.stop().expect("first VM stop");

    let after_first =
        std::fs::read_to_string(&console_log).expect("read console.log after first boot");
    assert!(
        !after_first.contains("STALE_SENTINEL_FROM_PREVIOUS_RUN"),
        "console.log must be truncated at VM start — stale sentinel must not appear"
    );

    let size_after_first = after_first.len();

    // Second boot
    let guest2 = Guest::new(config);
    guest2.start().expect("second VM start");
    guest2.wait_for_ready().expect("second VM ready");
    guest2.stop().expect("second VM stop");

    let after_second =
        std::fs::read_to_string(&console_log).expect("read console.log after second boot");

    // The log should be approximately the same size as after the first boot (not doubled).
    assert!(
        after_second.len() < size_after_first * 2,
        "console.log must not accumulate across reboots: first={} bytes, second={} bytes",
        size_after_first,
        after_second.len()
    );
}

/// A boot that fails to reach READY must still produce a console.log with error context.
#[test]
#[ignore = "requires VM entitlement + kernel artifact (no initrd — intentional bad config)"]
fn console_log_exists_on_failed_boot() {
    let home = speck_home();
    let console_log = home.join("console.log");
    let _ = std::fs::remove_file(&console_log);

    // Deliberately omit the initrd so vminitd never runs and READY never fires.
    // The kernel will still boot and write console output before panicking.
    let config = GuestConfig::builder()
        .kernel_path(home.join("kernel/vmlinux"))
        .speck_home(&home)
        .cmdline("console=hvc0 panic=1 ready_vsock_port=9000")
        .cpu_count(1)
        .memory_size_bytes(512 * 1024 * 1024)
        .stop_timeout(Duration::from_secs(10))
        .ready_vsock_port(9000)
        .build();

    let guest = Guest::new(config);
    guest.start().expect("VM start (kernel only)");

    // wait_for_ready will fail (no vminitd), but console.log should still exist.
    let _ = guest.wait_for_ready();

    assert!(
        console_log.exists(),
        "console.log must be created even when the boot fails to reach READY"
    );

    let content = std::fs::read_to_string(&console_log).expect("read console.log");
    assert!(
        !content.is_empty(),
        "console.log must contain kernel output even on a failed boot"
    );

    let _ = guest.stop();
}
