//! Network-free, root-free mount orchestration tests.
//!
//! Uses `mockall::mock!` locally because `#[cfg_attr(test, mockall::automock)]
//! on the `Syscalls` trait only generates `MockSyscalls` when compiling the
//! library with `--cfg test` (unit tests).  Integration tests compile the crate
//! as a normal dependency where `#[cfg(test)]` is inactive.
//!
//! Tests verify the mount ordering and call sequences of the extracted
//! mount functions without requiring root or a Linux host.

use mockall::Sequence;
use speck_guest::mount::{
    chroot_into_rootfs, mount_disks, mount_early_filesystems,
    mount_rootfs_runtime_filesystems,
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
        fn grow_filesystem(&self, device: &str, mountpoint: &str) -> Result<(), std::io::Error>;
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
        .withf(|_, target, fstype, _| {
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
        .withf(|_, target, fstype, _| {
            target == b"/rootfs/run\0" && fstype == b"tmpfs\0"
        })
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_, _, _, _| 0);

    mount_rootfs_runtime_filesystems(&mock);
}

// ---------------------------------------------------------------------------
// Root switch (chroot + chdir) tests
// ---------------------------------------------------------------------------

#[test]
fn chroot_into_rootfs_calls_chroot_before_chdir() {
    let mut mock = MockSyscallProxy::new();
    let mut seq = Sequence::new();

    mock.expect_chroot()
        .withf(|path| path == b"/rootfs\0")
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_| 0);

    mock.expect_chdir()
        .withf(|path| path == b"/\0")
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_| 0);

    let result = chroot_into_rootfs(&mock);
    assert!(result.is_ok());
}

#[test]
fn chroot_into_rootfs_returns_error_when_chroot_fails() {
    let mut mock = MockSyscallProxy::new();

    mock.expect_chroot()
        .withf(|path| path == b"/rootfs\0")
        .times(1)
        .returning(|_| -1);

    // chdir should never be called if chroot fails
    mock.expect_chdir().never();

    let result = chroot_into_rootfs(&mock);
    assert!(result.is_err());
}

#[test]
fn chroot_into_rootfs_returns_error_when_chdir_fails() {
    let mut mock = MockSyscallProxy::new();

    mock.expect_chroot()
        .withf(|path| path == b"/rootfs\0")
        .times(1)
        .returning(|_| 0);

    mock.expect_chdir()
        .withf(|path| path == b"/\0")
        .times(1)
        .returning(|_| -1);

    let result = chroot_into_rootfs(&mock);
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// Disk mount ordering test (uses mock for grow step)
// ---------------------------------------------------------------------------

#[test]
fn mount_disks_mounts_vda_before_vdb_before_grow_before_dirs() {
    let mut mock = MockSyscallProxy::new();
    let mut seq = Sequence::new();

    // Expect /dev/vda → /rootfs mount first
    mock.expect_mount()
        .withf(|source, target, _, _| {
            source == b"/dev/vda\0" && target == b"/rootfs\0"
        })
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_, _, _, _| 0);

    // Expect /dev/vdb → /rootfs/var/lib/containerd mount second
    mock.expect_mount()
        .withf(|source, target, _, _| {
            source == b"/dev/vdb\0" && target == b"/rootfs/var/lib/containerd\0"
        })
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_, _, _, _| 0);

    // Expect grow_filesystem after mounts succeed
    mock.expect_grow_filesystem()
        .withf(|device, mountpoint| {
            device == "/dev/vdb" && mountpoint == "/rootfs/var/lib/containerd"
        })
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_, _| Ok(()));

    mount_disks(&mock);
    // Expectations verified by mockall on drop
}
