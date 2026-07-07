//! Mount orchestration functions extracted from the vminitd binary.
//!
//! Each top-level function accepts `&dyn Syscalls` so that mount ordering
//! and error behaviour can be tested with mock implementations that do not
//! require root or a Linux host.
//!
//! # Design
//!
//! - Functions that perform `libc::mount` / `libc::chroot` calls accept
//!   `&dyn Syscalls` and route all syscall operations through the trait.
//! - Helper functions that use `std::process::Command` or `std::fs` (e.g.,
//!   `grow_data_filesystem_if_needed`, `run_resize_tool`,
//!   `data_disk_has_no_filesystem_signature`) are plain `pub` functions
//!   that do not require the trait — they compile on any platform.

use crate::Syscalls;
use std::io;
use std::path::Path;

// ---------------------------------------------------------------------------
// Constants — raw Linux mount flags (MS_BIND and MS_RELATIME are not
// available on macOS via the libc crate, so we define them locally).
// ---------------------------------------------------------------------------

const MS_BIND: u64 = 4096;
const MS_RELATIME: u64 = 2097152;

// ---------------------------------------------------------------------------
// Early filesystem mounts
// ---------------------------------------------------------------------------

/// Mount /proc, /sys, and /dev (devtmpfs) before any disk operations.
///
/// Each mount is non-fatal — errors are logged but execution continues
/// (may already be mounted by the kernel during early boot).
pub fn mount_early_filesystems(syscalls: &dyn Syscalls) {
    // procfs
    let ret = syscalls.mount(b"proc\0", b"/proc\0", b"proc\0", 0);
    if ret < 0 {
        tracing::warn!("mount /proc failed: {:?}", io::Error::last_os_error());
    }

    // sysfs
    let ret = syscalls.mount(b"sysfs\0", b"/sys\0", b"sysfs\0", 0);
    if ret < 0 {
        tracing::warn!("mount /sys failed: {:?}", io::Error::last_os_error());
    }

    // devtmpfs
    let ret = syscalls.mount(b"devtmpfs\0", b"/dev\0", b"devtmpfs\0", 0);
    if ret < 0 {
        tracing::warn!("mount /dev failed: {:?}", io::Error::last_os_error());
    }
}

// ---------------------------------------------------------------------------
// Bind-mount helper
// ---------------------------------------------------------------------------

/// Bind-mount `source` onto `target` using MS_BIND.
///
/// Non-fatal: logs the error and returns.
pub(crate) fn bind_mount(syscalls: &dyn Syscalls, source: &str, target: &str) {
    use std::ffi::CString;
    let src = CString::new(source).unwrap_or_default();
    let tgt = CString::new(target).unwrap_or_default();

    let ret = syscalls.mount(
        src.as_bytes_with_nul(),
        tgt.as_bytes_with_nul(),
        b"",
        MS_BIND,
    );
    if ret < 0 {
        tracing::warn!(
            source = %source,
            target = %target,
            error = %io::Error::last_os_error(),
            "bind mount failed"
        );
    }
}

// ---------------------------------------------------------------------------
// Runtime filesystem mounts inside /rootfs
// ---------------------------------------------------------------------------

/// Bind-mount /proc, /sys, /dev from the initrd namespace into /rootfs,
/// and mount a fresh tmpfs at /rootfs/run.
///
/// This is required before running dockerd (or any process) in a chroot
/// rooted at /rootfs: the chroot'd process must be able to see proc, sys,
/// and dev, which only exist in the outer namespace after
/// `mount_early_filesystems()` runs.
///
/// Call order: must run AFTER `mount_disks()` so that /rootfs is a
/// valid ext4 mount point, and AFTER `mount_early_filesystems()` so
/// that the bind sources (/proc, /sys, /dev) themselves exist.
///
/// Mount failures are non-fatal — errors are logged and execution
/// continues.  A chroot'd process that cannot see /proc will typically
/// fail to start on its own; the error log is sufficient for diagnosis.
pub fn mount_rootfs_runtime_filesystems(syscalls: &dyn Syscalls) {
    // /rootfs/proc — bind from /proc
    let _ = std::fs::create_dir_all("/rootfs/proc");
    bind_mount(syscalls, "/proc", "/rootfs/proc");

    // /rootfs/sys — bind from /sys
    let _ = std::fs::create_dir_all("/rootfs/sys");
    bind_mount(syscalls, "/sys", "/rootfs/sys");

    // /rootfs/sys/fs/cgroup — mount cgroup2 hierarchy.
    // A plain MS_BIND of /sys does NOT carry cgroupv2 submounts;
    // crun sees sysfs type at /sys/fs/cgroup and rejects it with
    // "invalid file system type". Mount cgroup2 directly on top.
    let _ = std::fs::create_dir_all("/rootfs/sys/fs/cgroup");
    let ret = syscalls.mount(
        b"cgroup2\0",
        b"/rootfs/sys/fs/cgroup\0",
        b"cgroup2\0",
        0,
    );
    if ret < 0 {
        tracing::warn!(
            error = %io::Error::last_os_error(),
            "mount cgroup2 at /rootfs/sys/fs/cgroup failed"
        );
    } else {
        tracing::info!("mounted cgroup2 at /rootfs/sys/fs/cgroup");
    }

    // /rootfs/dev — bind from /dev
    let _ = std::fs::create_dir_all("/rootfs/dev");
    bind_mount(syscalls, "/dev", "/rootfs/dev");

    // /rootfs/run — fresh tmpfs (runtime sockets + pid files are ephemeral)
    let _ = std::fs::create_dir_all("/rootfs/run");
    let ret = syscalls.mount(b"tmpfs\0", b"/rootfs/run\0", b"tmpfs\0", 0);
    if ret < 0 {
        tracing::warn!(
            error = %io::Error::last_os_error(),
            "mount tmpfs at /rootfs/run failed"
        );
    }
}

// ---------------------------------------------------------------------------
// Root switch (chroot + chdir)
// ---------------------------------------------------------------------------

/// Chroot into `/rootfs` and switch the working directory to `/`.
///
/// This is the equivalent of calling `chroot /rootfs` followed by `cd /`
/// from a shell script.  It is used by the dockerd pre-exec closure to
/// set up a chrooted process environment rooted at the guest rootfs.
///
/// # Errors
///
/// Returns an error if the `chroot` or `chdir` syscall fails.
pub fn chroot_into_rootfs(syscalls: &dyn Syscalls) -> Result<(), io::Error> {
    let ret = syscalls.chroot(b"/rootfs\0");
    if ret < 0 {
        return Err(io::Error::last_os_error());
    }
    let ret = syscalls.chdir(b"/\0");
    if ret < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Sysctl initialization
// ---------------------------------------------------------------------------

/// Write kernel parameters needed for container runtime operation.
///
/// Calls `sysctl_write` for each required parameter in a fixed order.
/// Returns the first error encountered; subsequent parameters are not
/// attempted.
///
/// # Parameters set
///
/// | Key | Value | Reason |
/// |-----|-------|--------|
/// | `net.ipv4.ip_forward` | `1` | Enable IP forwarding for container bridge/networking |
/// | `kernel.unprivileged_userns_clone` | `1` | Allow unprivileged user namespace cloning (required by crun) |
pub fn configure_sysctl_params(syscalls: &dyn Syscalls) -> Result<(), io::Error> {
    syscalls.sysctl_write("net.ipv4.ip_forward", "1")?;
    syscalls.sysctl_write("kernel.unprivileged_userns_clone", "1")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Rootfs + data disk mounts
// ---------------------------------------------------------------------------

/// Mount the rootfs and data disks.
///
/// 1. Mount /dev/vda (rootfs) at /rootfs as ext4.
/// 2. Mount /dev/vdb (data disk) at /rootfs/var/lib/containerd as ext4.
///    If the data disk has no filesystem (EINVAL), format it with mke2fs
///    only after proving it has no filesystem signature, then retry.
///    Failure on the second attempt is fatal.
/// 3. Grow the data filesystem to match the block device size.
/// 4. Create /rootfs/run/ and /rootfs/tmp/.
pub fn mount_disks(syscalls: &dyn Syscalls) {
    // Create root mount point
    let _ = std::fs::create_dir_all("/rootfs");

    // Mount /dev/vda → /rootfs (rootfs disk)
    let ret = syscalls.mount(b"/dev/vda\0", b"/rootfs\0", b"ext4\0", MS_RELATIME);
    if ret < 0 {
        tracing::error!(
            error = %io::Error::last_os_error(),
            "mount /dev/vda → /rootfs failed"
        );
        std::process::exit(1);
    }

    // Create containerd data directory on rootfs
    let _ = std::fs::create_dir_all("/rootfs/var/lib/containerd");

    // Try to mount /dev/vdb → /rootfs/var/lib/containerd (data disk)
    let ret = syscalls.mount(
        b"/dev/vdb\0",
        b"/rootfs/var/lib/containerd\0",
        b"ext4\0",
        MS_RELATIME,
    );
    if ret < 0 {
        let err = io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EINVAL) {
            if !data_disk_has_no_filesystem_signature("/dev/vdb") {
                tracing::error!(
                    error = %err,
                    "/dev/vdb mount failed but disk is not proven blank; refusing to format"
                );
                std::process::exit(1);
            }
            tracing::info!("/dev/vdb has no filesystem signature — formatting with mke2fs");
            let mke2fs_status = std::process::Command::new("/sbin/mke2fs")
                .args(["-t", "ext4", "/dev/vdb"])
                .status()
                .expect("vminitd: failed to start /sbin/mke2fs");
            if !mke2fs_status.success() {
                tracing::error!(status = %mke2fs_status, "mke2fs failed");
                std::process::exit(1);
            }
            // Retry mount after formatting
            let ret = syscalls.mount(
                b"/dev/vdb\0",
                b"/rootfs/var/lib/containerd\0",
                b"ext4\0",
                MS_RELATIME,
            );
            if ret < 0 {
                tracing::error!(
                    error = %io::Error::last_os_error(),
                    "mount /dev/vdb → /rootfs/var/lib/containerd failed after format"
                );
                std::process::exit(1);
            }
        } else {
            tracing::error!(
                error = %err,
                "mount /dev/vdb → /rootfs/var/lib/containerd failed"
            );
            std::process::exit(1);
        }
    }

    if let Err(e) = syscalls.grow_filesystem("/dev/vdb", "/rootfs/var/lib/containerd") {
        tracing::error!(
            device = "/dev/vdb",
            error = %e,
            "data filesystem resize failed"
        );
        std::process::exit(1);
    }

    // Create runtime directories needed by containerd
    let _ = std::fs::create_dir_all("/rootfs/run");
    let _ = std::fs::create_dir_all("/rootfs/tmp");
}

// ---------------------------------------------------------------------------
// Helpers — these use std::process::Command / std::fs, not syscalls,
// so they do not need the trait.
// ---------------------------------------------------------------------------

/// Grow the mounted ext4 data filesystem to match the current block device size.
///
/// The caller (`mount_disks`) is responsible for calling `std::process::exit(1)`
/// if this function returns an error — that preserves the production behaviour
/// of failing closed while allowing this function to be called from test mock
/// implementations that should not exit the test process.
///
/// # Errors
///
/// Returns an error if the mountpoint is missing, the resize tool is missing,
/// or the resize tool fails to execute.
pub fn grow_data_filesystem_if_needed(device: &str, mountpoint: &str) -> io::Result<()> {
    if !Path::new(mountpoint).exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("data filesystem mountpoint {mountpoint} missing"),
        ));
    }

    let resize_tool = "/sbin/resize2fs";
    if !Path::new(resize_tool).exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("data filesystem resize tool {resize_tool} missing"),
        ));
    }

    tracing::info!(device, "growing data filesystem");
    match run_resize_tool(resize_tool, &[device]) {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(io::Error::new(
            io::ErrorKind::Other,
            format!("resize2fs failed with {status}"),
        )),
        Err(e) => Err(io::Error::new(
            io::ErrorKind::Other,
            format!("data filesystem resize tool failed: {e}"),
        )),
    }
}

/// Run an external tool and return its exit status.
pub fn run_resize_tool(tool: &str, args: &[&str]) -> std::io::Result<std::process::ExitStatus> {
    std::process::Command::new(tool).args(args).status()
}

/// Return true only when a bounded signature probe positively reports that
/// the data disk has no recognizable filesystem signature.
///
/// Fail-safe semantics: recognized signatures, missing probe tools,
/// execution failures, and ambiguous output all return false, which means
/// the caller must not format the disk.
pub fn data_disk_has_no_filesystem_signature(device: &str) -> bool {
    let output = match std::process::Command::new("/sbin/blkid")
        .arg(device)
        .output()
    {
        Ok(output) => output,
        Err(e) => {
            tracing::warn!(device, error = %e, "failed to run /sbin/blkid");
            return false;
        }
    };

    if output.status.success() {
        tracing::warn!(device, "/sbin/blkid found a signature; refusing to format");
        return false;
    }

    let stdout_empty = output.stdout.iter().all(|b| b.is_ascii_whitespace());
    let stderr_empty = output.stderr.iter().all(|b| b.is_ascii_whitespace());

    match output.status.code() {
        Some(2) if stdout_empty && stderr_empty => true,
        code => {
            tracing::warn!(device, ?code, "/sbin/blkid did not prove disk is blank");
            false
        }
    }
}
