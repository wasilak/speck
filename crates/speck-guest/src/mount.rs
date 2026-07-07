//! Mount orchestration functions extracted from the vminitd binary.
//!
//! Each top-level function accepts `&dyn Syscalls` so that mount ordering
//! and error behaviour can be tested with mock implementations that do not
//! require root or a Linux host.
//!
//! # RED phase stubs
//!
//! These functions contain minimal placeholder implementations.  The
//! integration tests in `tests/vminitd_mount_test.rs` expect specific
//! `mount` call sequences; the stubs below do NOT call `syscalls.mount()`,
//! so the mock expectations fail — confirming the tests detect missing
//! behaviour.  The GREEN phase fills in the real implementation.

use crate::Syscalls;

// ---------------------------------------------------------------------------
// Constants — raw Linux mount flags (MS_BIND and MS_RELATIME are not
// available on macOS via the libc crate).
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
///
/// # RED stub
///
/// Currently does NOT call `syscalls.mount()`, deliberately failing the
/// integration test expectations.
pub fn mount_early_filesystems(_syscalls: &dyn Syscalls) {
    // STUB: will call syscalls.mount() in GREEN phase
}

// ---------------------------------------------------------------------------
// Rootfs + data disk mounts
// ---------------------------------------------------------------------------

/// Mount the rootfs and data disks.
///
/// 1. Mount /dev/vda (rootfs) at /rootfs as ext4.
/// 2. Mount /dev/vdb (data disk) at /rootfs/var/lib/containerd as ext4.
///    If the data disk has no filesystem (EINVAL), format it with mke2fs
///    then retry.  Failure on the second attempt is fatal.
/// 3. Grow the data filesystem to match the block device size.
/// 4. Create /rootfs/run/ and /rootfs/tmp/.
///
/// # RED stub
///
/// Currently does NOT call `syscalls.mount()`, deliberately failing the
/// integration test expectations.
pub fn mount_disks(_syscalls: &dyn Syscalls) {
    // STUB: will call syscalls.mount() in GREEN phase
}

// ---------------------------------------------------------------------------
// Runtime filesystem mounts inside /rootfs
// ---------------------------------------------------------------------------

/// Bind-mount /proc, /sys, /dev from the initrd namespace into /rootfs,
/// and mount a fresh tmpfs at /rootfs/run.
///
/// # RED stub
///
/// Currently does NOT call `syscalls.mount()`, deliberately failing the
/// integration test expectations.
pub fn mount_rootfs_runtime_filesystems(_syscalls: &dyn Syscalls) {
    // STUB: will call syscalls.mount() and bind_mount() in GREEN phase
}

// ---------------------------------------------------------------------------
// Helpers — these use std::process::Command / std::fs, not syscalls,
// so they do not need the trait.
// ---------------------------------------------------------------------------

/// Grow the mounted ext4 data filesystem to match the current block device size.
pub fn grow_data_filesystem_if_needed(device: &str, mountpoint: &str) {
    if !std::path::Path::new(mountpoint).exists() {
        tracing::error!(device, mountpoint, "data filesystem mountpoint missing");
        std::process::exit(1);
    }

    let resize_tool = "/sbin/resize2fs";
    if !std::path::Path::new(resize_tool).exists() {
        tracing::error!(
            device,
            resize_tool,
            "data filesystem resize tool missing"
        );
        std::process::exit(1);
    }

    tracing::info!(device, "growing data filesystem");
    match run_resize_tool(resize_tool, &[device]) {
        Ok(status) if status.success() => {}
        Ok(status) => {
            tracing::error!(device, %status, "resize2fs failed");
            std::process::exit(1);
        }
        Err(e) => {
            tracing::error!(device, error = %e, "data filesystem resize tool failed");
            std::process::exit(1);
        }
    }
}

/// Run an external tool and return its exit status.
pub fn run_resize_tool(tool: &str, args: &[&str]) -> std::io::Result<std::process::ExitStatus> {
    std::process::Command::new(tool).args(args).status()
}

/// Return true only when a bounded signature probe positively reports that
/// the data disk has no recognizable filesystem signature.
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
