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
// Constants — raw Linux mount flags (not available on macOS via the libc
// crate, so we define them locally).
// ---------------------------------------------------------------------------

const MS_RELATIME: u64 = 2097152;
const MS_MOVE: u64 = 8192;
const MS_REC: u64 = 16384;
const MS_PRIVATE: u64 = 262144;

// ---------------------------------------------------------------------------
// Mount propagation
// ---------------------------------------------------------------------------

/// Recursively mark the entire mount tree rooted at `/` as `MS_PRIVATE`.
///
/// This must run as the very first mount-related operation during boot,
/// before any other mount (including `mount_early_filesystems`) — matching
/// the first action every real init system (systemd, dracut, busybox)
/// takes for exactly this reason: without it, `MS_MOVE`/propagation-related
/// operations later in boot operate against the kernel's default (shared)
/// propagation, which this project's own prior debug session
/// (`.planning/debug/resolved/buildx-http2-frame-too-large.md`) already
/// found pervasive throughout the guest's mount tree. Kept as a defensive,
/// standard-practice measure even though the specific live-boot bug this
/// plan root-caused (fresh vs. bind-mounted `/proc`/`/sys`/`/dev` surviving
/// `switch_root_to_rootfs`'s old-root detach differently — see
/// `mount_rootfs_runtime_filesystems`'s doc comment) turned out to be about
/// bind-mount vs. fresh-mount ancestry, not propagation type — propagation
/// was already private by the time that bug was isolated.
///
/// Non-fatal: logs the error and continues, matching the non-fatal posture
/// of every other propagation-adjacent helper in this module.
pub fn make_mount_tree_private(syscalls: &dyn Syscalls) {
    let ret = syscalls.mount(b"none\0", b"/\0", b"\0", MS_REC | MS_PRIVATE);
    if ret < 0 {
        tracing::warn!(
            error = %io::Error::last_os_error(),
            "mount(MS_REC | MS_PRIVATE) on / failed"
        );
    }
}

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
// Runtime filesystem mounts inside /rootfs
// ---------------------------------------------------------------------------

/// Mount fresh /proc, /sys, /dev instances inside /rootfs, plus a fresh
/// tmpfs at /rootfs/run.
///
/// This is required before running dockerd (or any process) in a chroot
/// rooted at /rootfs: the chroot'd process must be able to see proc, sys,
/// and dev.
///
/// These are deliberately FRESH mounts (matching the same `mount(2)` calls
/// `mount_early_filesystems()` uses for the outer namespace), not bind
/// mounts of the outer `/proc`/`/sys`/`/dev`. Bind-mounting them from the
/// outer namespace was tried first and found to be **incompatible** with
/// `switch_root_to_rootfs`'s old-root detach step (D-06): live-boot testing
/// (2026-07-17, Phase 25 Plan 02 Task 2) showed that `umount2(old_root,
/// MNT_DETACH)` on the anonymous initramfs root also silently took down
/// every bind-mounted *copy* of a mount that was still live under that old
/// root (`/proc`, `/sys`, `/dev`) — while mounts with no live alias
/// elsewhere in the namespace (the `/rootfs/run` tmpfs, the
/// `/rootfs/sys/fs/cgroup` cgroup2 mount, the VirtioFS shares) survived the
/// same move+detach untouched. Mounting fresh here (same technique already
/// proven to survive by the tmpfs/cgroup2 mounts below) sidesteps the
/// interaction entirely and matches what real switch_root implementations
/// (systemd, dracut) do: mount /proc, /sys, /dev anew after switching root,
/// rather than carrying bind-mounted copies through the switch.
///
/// Call order: must run AFTER `mount_disks()` so that /rootfs is a
/// valid ext4 mount point.
///
/// Mount failures are non-fatal — errors are logged and execution
/// continues.  A chroot'd process that cannot see /proc will typically
/// fail to start on its own; the error log is sufficient for diagnosis.
pub fn mount_rootfs_runtime_filesystems(syscalls: &dyn Syscalls) {
    // /rootfs/proc — fresh procfs mount
    let _ = std::fs::create_dir_all("/rootfs/proc");
    let ret = syscalls.mount(b"proc\0", b"/rootfs/proc\0", b"proc\0", 0);
    if ret < 0 {
        tracing::warn!(
            error = %io::Error::last_os_error(),
            "mount proc at /rootfs/proc failed"
        );
    }

    // /rootfs/sys — fresh sysfs mount
    let _ = std::fs::create_dir_all("/rootfs/sys");
    let ret = syscalls.mount(b"sysfs\0", b"/rootfs/sys\0", b"sysfs\0", 0);
    if ret < 0 {
        tracing::warn!(
            error = %io::Error::last_os_error(),
            "mount sysfs at /rootfs/sys failed"
        );
    }

    // /rootfs/sys/fs/cgroup — mount cgroup2 hierarchy.
    // A plain sysfs mount does NOT carry cgroupv2 submounts;
    // crun sees sysfs type at /sys/fs/cgroup and rejects it with
    // "invalid file system type". Mount cgroup2 directly on top.
    let _ = std::fs::create_dir_all("/rootfs/sys/fs/cgroup");
    let ret = syscalls.mount(b"cgroup2\0", b"/rootfs/sys/fs/cgroup\0", b"cgroup2\0", 0);
    if ret < 0 {
        tracing::warn!(
            error = %io::Error::last_os_error(),
            "mount cgroup2 at /rootfs/sys/fs/cgroup failed"
        );
    } else {
        tracing::info!("mounted cgroup2 at /rootfs/sys/fs/cgroup");
    }

    // /rootfs/dev — fresh devtmpfs mount (devtmpfs has a single kernel-wide
    // instance, so a second mount surfaces the same device nodes as the
    // outer /dev, without needing to bind-mount the outer instance).
    let _ = std::fs::create_dir_all("/rootfs/dev");
    let ret = syscalls.mount(b"devtmpfs\0", b"/rootfs/dev\0", b"devtmpfs\0", 0);
    if ret < 0 {
        tracing::warn!(
            error = %io::Error::last_os_error(),
            "mount devtmpfs at /rootfs/dev failed"
        );
    }

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
// Switch root (MS_MOVE)
// ---------------------------------------------------------------------------

/// Switch the process root to `/rootfs` via a canonical `MS_MOVE`-based
/// `switch_root`, matching the sequence used by util-linux's own
/// `switch_root(8)` (and busybox/systemd/dracut): `chdir(/rootfs)` ->
/// `mount(/rootfs, "/", MS_MOVE)` -> `chroot(".")` -> `chdir("/")`.
///
/// This re-parents the `/rootfs` mount onto `/` in the mount namespace
/// itself (not just the calling process's `fs_struct.root` pointer, which
/// is all a chroot-only approach changes) — see D-22/BOOT-01.
///
/// # Old-root detach — deliberately NOT attempted
///
/// D-06 originally called for a best-effort `fchdir` + `umount2(MNT_DETACH)`
/// detach of the orphaned old root (the initramfs "rootfs" pseudo-
/// filesystem) after the move, captured via an fd taken before the move (the
/// only way to reference the old root once no path resolves to it anymore).
/// This was implemented and live-boot tested (2026-07-17, Phase 25 Plan 02
/// Task 2) and found to be **actively harmful**, not merely
/// occasionally-failing-and-therefore-harmless as D-06 anticipated: when the
/// `umount2(MNT_DETACH)` call *succeeds* (returns 0, no error), it also
/// silently detaches `/proc`, `/sys`, and `/dev` inside the new root —
/// reproduced identically whether those were bind mounts of the outer
/// namespace's `/proc`/`/sys`/`/dev` or freshly-mounted instances created
/// directly under `/rootfs` (ruling out "bind mount" as the distinguishing
/// factor; some property of how the kernel accounts for the old root's
/// still-attached children survives the move and is disturbed by detaching
/// it). This broke `dockerd`'s embedded containerd startup on every single
/// boot (`open /proc/self/mountinfo: no such file or directory`, `open
/// /dev/null: no such file or directory`), confirmed via an isolated A/B
/// test: skipping only the `umount2` call (keeping everything else
/// identical) fixed dockerd startup immediately (socket ready on the very
/// first attempt, vs. an infinite crash-restart loop with it enabled).
///
/// Not attempting the detach matches busybox's own `switch_root.c`
/// (quoted in `.planning/debug/resolved/buildx-http2-frame-too-large.md`,
/// the prior debug session in this project): "Don't try to unmount the old
/// '/', there's no way to do it." The leftover, permanently-unreachable old
/// root mount is harmless — no path resolves to it after the move, it isn't
/// carried into container mount namespaces at all (that's a structurally
/// separate mount from `/rootfs`, which is what `unshare(CLONE_NEWNS)`
/// clones for each container) — matching D-06's own original framing that
/// a *non-executed* detach "leaves the guest at least as functional as a
/// chroot-only baseline". This function therefore performs the move only;
/// it does not attempt any old-root cleanup.
///
/// After this call succeeds, all paths that previously started with
/// `/rootfs/` are valid at `/` — e.g. `/rootfs/etc` becomes `/etc`.
///
/// # Errors
///
/// Returns an error if the `chdir(/rootfs)`, `mount(MS_MOVE)`,
/// `chroot(".")`, or post-move `chdir("/")` syscall fails — the caller
/// should exit, since the guest cannot function without a valid root.
pub fn switch_root_to_rootfs(syscalls: &dyn Syscalls) -> Result<(), io::Error> {
    // chdir into the new root before the move. After MS_MOVE re-parents
    // /rootfs onto "/", the string "/rootfs" no longer resolves to
    // anything meaningful — "." (this pre-established cwd) is the only
    // valid way to reference the new root afterward.
    let ret = syscalls.chdir(b"/rootfs\0");
    if ret < 0 {
        return Err(io::Error::last_os_error());
    }

    let ret = syscalls.mount(b"/rootfs\0", b"/\0", b"\0", MS_MOVE);
    if ret < 0 {
        return Err(io::Error::last_os_error());
    }

    // Must be "." here, never "/rootfs" — see comment above.
    let ret = syscalls.chroot(b".\0");
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
        Ok(status) => Err(io::Error::other(format!("resize2fs failed with {status}"))),
        Err(e) => Err(io::Error::other(format!(
            "data filesystem resize tool failed: {e}"
        ))),
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

    let stdout_empty = output.stdout.iter().all(|b| b.is_ascii_whitespace());
    let stderr_empty = output.stderr.iter().all(|b| b.is_ascii_whitespace());

    if output.status.success() && stdout_empty && stderr_empty {
        return true;
    }

    if output.status.success() {
        tracing::warn!(device, "/sbin/blkid found a signature; refusing to format");
        return false;
    }

    match output.status.code() {
        Some(2) if stdout_empty && stderr_empty => true,
        code => {
            tracing::warn!(device, ?code, "/sbin/blkid did not prove disk is blank");
            false
        }
    }
}
