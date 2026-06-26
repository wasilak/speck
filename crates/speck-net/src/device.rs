use std::os::unix::io::RawFd;
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::time::Instant;

/// A wrapper around a raw file descriptor that implements `smoltcp::phy::Device`.
///
/// Reads/writes raw L2 Ethernet frames from a socketpair fd. The fd is set to
/// `O_NONBLOCK` and is closed on `Drop`.
pub(crate) struct FdDevice {
    fd: RawFd,
    mtu: usize,
}

impl FdDevice {
    /// Create a new `FdDevice` wrapping the given file descriptor.
    ///
    /// The fd is set to `O_NONBLOCK` via `fcntl`. The caller must ensure `fd`
    /// is a valid, open file descriptor — `FdDevice` takes ownership and will
    /// close it on `Drop`.
    pub fn new(fd: RawFd, mtu: usize) -> Self {
        // Get existing flags to preserve them
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 {
            panic!(
                "FdDevice::new: fcntl(F_GETFL) failed: {}",
                std::io::Error::last_os_error()
            );
        }
        // Add O_NONBLOCK
        let result = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
        if result < 0 {
            panic!(
                "FdDevice::new: fcntl(F_SETFL, O_NONBLOCK) failed: {}",
                std::io::Error::last_os_error()
            );
        }
        Self { fd, mtu }
    }
}

impl Drop for FdDevice {
    fn drop(&mut self) {
        unsafe { libc::close(self.fd); }
    }
}

/// Token that holds received frame data.
pub(crate) struct FdRxToken(Vec<u8>);

impl RxToken for FdRxToken {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(&self.0)
    }
}

/// Token that holds the fd for writing outbound frames.
pub(crate) struct FdTxToken {
    fd: RawFd,
}

impl TxToken for FdTxToken {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut buf = vec![0u8; len];
        let result = f(&mut buf);
        unsafe {
            libc::write(self.fd, buf.as_ptr() as *const libc::c_void, len);
        }
        result
    }
}

impl Device for FdDevice {
    type RxToken<'a> = FdRxToken;
    type TxToken<'a> = FdTxToken;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let mut rx_buf = vec![0u8; self.mtu + 14]; // MTU + Ethernet header
        match unsafe {
            libc::read(self.fd, rx_buf.as_mut_ptr() as *mut libc::c_void, rx_buf.len())
        } {
            n if n > 0 => {
                rx_buf.truncate(n as usize);
                Some((FdRxToken(rx_buf), FdTxToken { fd: self.fd }))
            }
            _ => None, // EAGAIN, EWOULDBLOCK, or error → no packet
        }
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        Some(FdTxToken { fd: self.fd })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = self.mtu;
        caps.max_burst_size = Some(1);
        caps
    }
}
