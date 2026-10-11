pub mod ca_trust;
#[cfg(target_os = "linux")]
pub mod dns_forwarder;
pub mod dns_wire;
#[cfg(target_os = "linux")]
pub mod log_relay;
#[cfg(target_os = "linux")]
pub mod sock_forwarder;
#[cfg(target_os = "linux")]
pub mod tcp_forwarder;
pub mod time_sync;
#[cfg(target_os = "linux")]
pub mod vsock_echo;

// ---------------------------------------------------------------------------
// Syscalls abstraction — defined unconditionally so tests compile on macOS.
// ---------------------------------------------------------------------------

/// Abstraction over Linux syscalls used by vminitd mount orchestration.
///
/// `Send + Sync + 'static` so implementors can be passed as `&dyn Syscalls`
/// across thread boundaries.  The `LibcSyscalls` struct provides the real
/// production implementation on Linux; tests use a mock.
#[cfg_attr(test, mockall::automock)]
pub trait Syscalls: Send + Sync + 'static {
    /// Mount a filesystem.  Mirrors `libc::mount`.
    fn mount(&self, source: &[u8], target: &[u8], fstype: &[u8], flags: u64) -> i32;

    /// Change root directory.  Mirrors `libc::chroot`.
    fn chroot(&self, path: &[u8]) -> i32;

    /// Change working directory.  Mirrors `libc::chdir`.
    fn chdir(&self, path: &[u8]) -> i32;

    /// Detach a mount point. Mirrors `libc::umount2` (Linux `umount2(2)`).
    fn umount2(&self, path: &[u8], flags: i32) -> i32;

    /// Change working directory via an open file descriptor. Mirrors `libc::fchdir`.
    fn fchdir(&self, fd: i32) -> i32;

    /// Write a kernel parameter via `sysctl`.  Mirrors `libc::sysctl` or
    /// `/proc/sys/` file write.
    fn sysctl_write(&self, name: &str, value: &str) -> Result<(), std::io::Error>;

    /// Grow an ext4 filesystem to match the block device size.
    ///
    /// On Linux this calls the real `grow_data_filesystem_if_needed` which
    /// runs `/sbin/resize2fs`.  Tests use a mock to control the outcome.
    fn grow_filesystem(&self, device: &str, mountpoint: &str) -> Result<(), std::io::Error>;
}

/// Production syscall implementation backed by raw `libc` calls.
///
/// Only available on Linux — the struct is defined unconditionally but its
/// `impl Syscalls` block is gated on `#[cfg(target_os = "linux")]`.
pub struct LibcSyscalls;

#[cfg(target_os = "linux")]
impl Syscalls for LibcSyscalls {
    fn mount(&self, source: &[u8], target: &[u8], fstype: &[u8], flags: u64) -> i32 {
        unsafe {
            libc::mount(
                source.as_ptr() as *const libc::c_char,
                target.as_ptr() as *const libc::c_char,
                fstype.as_ptr() as *const libc::c_char,
                flags,
                std::ptr::null(),
            )
        }
    }

    fn chroot(&self, path: &[u8]) -> i32 {
        unsafe { libc::chroot(path.as_ptr() as *const libc::c_char) }
    }

    fn chdir(&self, path: &[u8]) -> i32 {
        unsafe { libc::chdir(path.as_ptr() as *const libc::c_char) }
    }

    fn umount2(&self, path: &[u8], flags: i32) -> i32 {
        unsafe { libc::umount2(path.as_ptr() as *const libc::c_char, flags) }
    }

    fn fchdir(&self, fd: i32) -> i32 {
        unsafe { libc::fchdir(fd) }
    }

    fn sysctl_write(&self, name: &str, value: &str) -> Result<(), std::io::Error> {
        let path = format!("/proc/sys/{}", name.replace('.', "/"));
        match std::fs::write(&path, value) {
            // Knob absent from this kernel (e.g. unprivileged_userns_clone is a
            // distro patch; upstream allows user namespaces by default).
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("vminitd: sysctl {name} not available on this kernel — skipped");
                Ok(())
            }
            other => other,
        }
    }

    fn grow_filesystem(&self, device: &str, mountpoint: &str) -> Result<(), std::io::Error> {
        crate::mount::grow_data_filesystem_if_needed(device, mountpoint)
    }
}

// ---------------------------------------------------------------------------
// Mount orchestration — extracted from the vminitd binary for testability.
// ---------------------------------------------------------------------------

/// Mount orchestration functions that accept `&dyn Syscalls`.
///
/// All functions are `pub` so integration tests in `tests/` can import them.
pub mod mount;
