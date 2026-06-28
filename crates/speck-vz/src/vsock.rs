use std::io;
use std::os::unix::io::AsRawFd;

/// A host-side vsock connection wrapping a raw file descriptor.
///
/// Obtained from `Guest::vsock_connect()` (via `VZVirtioSocketConnection::fileDescriptor`).
/// The fd is owned and closed on `Drop`.
#[derive(Debug)]
pub struct VzSocket {
    fd: std::os::unix::io::RawFd,
}

impl VzSocket {
    /// Create a `VzSocket` from a raw file descriptor.
    ///
    /// # Safety
    ///
    /// `fd` must be a valid, open file descriptor suitable for read/write.
    /// `VzSocket` takes ownership and will close it on `Drop`.
    pub(crate) unsafe fn from_raw_fd(fd: std::os::unix::io::RawFd) -> Self {
        Self { fd }
    }

    /// Read up to `buf.len()` bytes. Returns the number of bytes read.
    /// On EOF (read returns 0), returns `Ok(0)`.
    pub fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        let ret = unsafe { libc::read(self.fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
        if ret < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(ret as usize)
        }
    }

    /// Write up to `buf.len()` bytes. Returns the number of bytes written.
    pub fn write(&self, buf: &[u8]) -> io::Result<usize> {
        let ret = unsafe { libc::write(self.fd, buf.as_ptr() as *const libc::c_void, buf.len()) };
        if ret < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(ret as usize)
        }
    }
}

impl AsRawFd for VzSocket {
    fn as_raw_fd(&self) -> std::os::unix::io::RawFd {
        self.fd
    }
}

impl Drop for VzSocket {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.fd);
        }
    }
}
