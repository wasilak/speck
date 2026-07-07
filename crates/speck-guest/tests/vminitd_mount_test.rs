//! Network-free, root-free mount orchestration tests.
//!
//! Uses `mockall::mock!` locally because `#[cfg_attr(test, mockall::automock)]`
//! on the `Syscalls` trait only generates `MockSyscalls` when compiling the
//! library with `--cfg test` (unit tests).  Integration tests compile the crate
//! as a normal dependency where `#[cfg(test)]` is inactive.
//!
//! Tests verify the mount ordering and call sequences of the extracted
//! mount functions without requiring root or a Linux host.
//!
//! # RED phase
//!
//! The mount functions are currently stubs — they do NOT call
//! `syscalls.mount()`.  The mockall expectations below require specific
//! mount calls, so each test will fail with a mockall panic until the
//! GREEN phase fills in the real implementations.

use mockall::predicate::*;
use mockall::Sequence;
use speck_guest::mount::{
    mount_disks, mount_early_filesystems, mount_rootfs_runtime_filesystems,
};
use speck_guest::Syscalls;

// ---------------------------------------------------------------------------
// Mock syscall implementation
// ---------------------------------------------------------------------------

mockall::mock! {
    pub SyscallProxy {}
    impl Syscalls for SyscallProxy {
        fn mount(&self, source: &[u8], target: &[u8], fstype: &[u8], flags: u64) -> i32;
        fn chroot(&self, path: &[u8]) -> i32;
        fn chdir(&self, path: &[u8]) -> i32;
        fn sysctl_write(&self, name: &str, value: &str) -> Result<(), std::io::Error>;
    }
}

// ---------------------------------------------------------------------------
// Early filesystem mount tests
// ---------------------------------------------------------------------------

#[test]
fn mount_early_filesystems_calls_proc_before_sys_before_dev() {
    let mut mock = MockSyscallProxy::new();
    let mut seq = Sequence::new();

    // Expect /proc mount first
    mock.expect_mount()
        .withf(|_, target, _, _| target == b"/proc\0")
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_, _, _, _| 0);

    // Expect /sys mount second
    mock.expect_mount()
        .withf(|_, target, _, _| target == b"/sys\0")
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_, _, _, _| 0);

    // Expect /dev mount third
    mock.expect_mount()
        .withf(|_, target, _, _| target == b"/dev\0")
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_, _, _, _| 0);

    mount_early_filesystems(&mock);
    // Expectations verified by mockall on drop
}

#[test]
fn mount_early_filesystems_uses_correct_fstypes() {
    let mut mock = MockSyscallProxy::new();

    // proc
    mock.expect_mount()
        .withf(|source, target, fstype, _| {
            source == b"proc\0" && target == b"/proc\0" && fstype == b"proc\0"
        })
        .times(1)
        .returning(|_, _, _, _| 0);

    // sysfs
    mock.expect_mount()
        .withf(|source, target, fstype, _| {
            source == b"sysfs\0" && target == b"/sys\0" && fstype == b"sysfs\0"
        })
        .times(1)
        .returning(|_, _, _, _| 0);

    // devtmpfs
    mock.expect_mount()
        .withf(|source, target, fstype, _| {
            source == b"devtmpfs\0" && target == b"/dev\0" && fstype == b"devtmpfs\0"
        })
        .times(1)
        .returning(|_, _, _, _| 0);

    mount_early_filesystems(&mock);
}

// ---------------------------------------------------------------------------
// Disk mount tests
// ---------------------------------------------------------------------------

#[test]
fn mount_disks_mounts_vda_before_vdb() {
    let mut mock = MockSyscallProxy::new();
    let mut seq = Sequence::new();

    // Expect /dev/vda → /rootfs as ext4 first
    mock.expect_mount()
        .withf(|source, target, fstype, _flags| {
            source == b"/dev/vda\0"
                && target == b"/rootfs\0"
                && fstype == b"ext4\0"
        })
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_, _, _, _| 0);

    // Expect /dev/vdb → /rootfs/var/lib/containerd as ext4 second
    mock.expect_mount()
        .withf(|source, target, fstype, _flags| {
            source == b"/dev/vdb\0"
                && target == b"/rootfs/var/lib/containerd\0"
                && fstype == b"ext4\0"
        })
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_, _, _, _| 0);

    mount_disks(&mock);
}

// ---------------------------------------------------------------------------
// Runtime filesystem mount tests
// ---------------------------------------------------------------------------

#[test]
fn mount_rootfs_runtime_mounts_proc_sys_dev_then_tmpfs_run() {
    let mut mock = MockSyscallProxy::new();
    let mut seq = Sequence::new();

    // Expect bind mount of /proc → /rootfs/proc first
    mock.expect_mount()
        .withf(|_, target, _, _| target == b"/rootfs/proc\0")
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_, _, _, _| 0);

    // Expect bind mount of /sys → /rootfs/sys second
    mock.expect_mount()
        .withf(|_, target, _, _| target == b"/rootfs/sys\0")
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_, _, _, _| 0);

    // Expect cgroup2 mount at /rootfs/sys/fs/cgroup third
    mock.expect_mount()
        .withf(|source, target, fstype, _flags| {
            target == b"/rootfs/sys/fs/cgroup\0" && fstype == b"cgroup2\0"
        })
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_, _, _, _| 0);

    // Expect bind mount of /dev → /rootfs/dev fourth
    mock.expect_mount()
        .withf(|_, target, _, _| target == b"/rootfs/dev\0")
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_, _, _, _| 0);

    // Expect tmpfs mount at /rootfs/run fifth
    mock.expect_mount()
        .withf(|source, target, fstype, _flags| {
            target == b"/rootfs/run\0" && fstype == b"tmpfs\0"
        })
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_, _, _, _| 0);

    mount_rootfs_runtime_filesystems(&mock);
}
