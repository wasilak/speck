pub mod config;
pub mod device;
pub mod error;
pub mod interface;

pub use error::{Error, Result};

use std::os::unix::io::RawFd;

/// Minimal wrapper around `RawFd` that implements `AsRawFd` for use with
/// `tokio::io::unix::AsyncFd`.
struct NetFd(RawFd);

impl std::os::unix::io::AsRawFd for NetFd {
    fn as_raw_fd(&self) -> RawFd {
        self.0
    }
}

/// The Speck netstack, managing a smoltcp-based TCP/IP stack over a socketpair fd.
///
/// Created with [`new()`](Self::new) and [`NetworkConfig`], then started with
/// [`spawn()`](Self::spawn) which returns a tokio task running the poll loop.
pub struct SpeckNet {
    fd: Option<RawFd>,
    config: config::NetworkConfig,
}

impl SpeckNet {
    /// Create a new `SpeckNet` with the given network configuration.
    ///
    /// The fd is provided later via [`spawn()`](Self::spawn).
    pub fn new(config: config::NetworkConfig) -> Self {
        Self { fd: None, config }
    }

    /// Start the netstack poll loop on a tokio task.
    ///
    /// Takes ownership of `fd` (the host end of a datagram socketpair), dups it,
    /// and spawns a tokio task that runs a `smoltcp::Interface` poll loop driven
    /// by `AsyncFd` readiness notifications.
    pub fn spawn(mut self, fd: RawFd) -> tokio::task::JoinHandle<std::result::Result<(), Error>> {
        self.fd = Some(fd);
        tokio::task::spawn(async move {
            let fd = self.fd.take().expect("SpeckNet: fd not set");

            // Wrap the fd for AsyncFd (readiness notification without closing on drop)
            let async_fd = tokio::io::unix::AsyncFd::new(NetFd(fd))
                .map_err(|e| Error::Netstack(format!("AsyncFd::new: {e}")))?;

            // Dup the fd so FdDevice gets its own copy (takes ownership, closes on drop)
            let device_fd = unsafe { libc::dup(fd) };
            if device_fd < 0 {
                return Err(Error::Netstack(format!(
                    "dup failed: {}",
                    std::io::Error::last_os_error()
                )));
            }

            // Build the smoltcp device and interface
            let device = device::FdDevice::new(device_fd, self.config.mtu as usize);
            let mut net = interface::SmoltcpInterface::new(device, &self.config);

            // Poll loop — waits for fd readiness then drives the smoltcp stack
            loop {
                let mut guard = async_fd
                    .readable()
                    .await
                    .map_err(|e| Error::Netstack(format!("readable: {e}")))?;
                guard.clear_ready();
                let timestamp = smoltcp::time::Instant::now();
                net.poll(timestamp);
            }
        })
    }
}
