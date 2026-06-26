pub mod config;
pub mod device;
pub mod error;
pub mod interface;
pub mod mtu;
pub(crate) mod dhcp;
pub(crate) mod dns;
pub(crate) mod reorigin;

pub use dns::spawn_dns_proxy;
pub use error::{Error, Result};
pub use mtu::{detect_host_mtu, mss_for_mtu};

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
/// [`spawn()`](Self::spawn) which returns tokio tasks running the poll loop
/// and DNS proxy.
pub struct SpeckNet {
    config: config::NetworkConfig,
    dns_vsock_port: Option<u32>,
}

impl SpeckNet {
    pub fn new(config: config::NetworkConfig, dns_vsock_port: Option<u32>) -> Self {
        Self {
            config,
            dns_vsock_port,
        }
    }

    /// Spawn the netstack tasks.
    ///
    /// Returns a vector of `JoinHandle`s:
    /// - A DNS proxy task if `vsock_fd` is provided
    /// - The main netstack poll loop (TCP re-origination, DHCP, packet processing)
    pub fn spawn(
        self,
        fd: RawFd,
        vsock_fd: Option<RawFd>,
    ) -> Vec<tokio::task::JoinHandle<std::result::Result<(), Error>>> {
        let mut handles = Vec::new();

        if let Some(vfd) = vsock_fd {
            handles.push(dns::spawn_dns_proxy(vfd));
        }

        handles.push(tokio::task::spawn(async move {
            let mtu = self.config.mtu as usize;

            // Wrap the fd for AsyncFd (readiness notification without closing on drop)
            let async_fd = tokio::io::unix::AsyncFd::new(NetFd(fd))
                .map_err(|e| Error::Netstack(format!("AsyncFd::new: {e}")))?;

            // Dup the fd so FdDevice gets its own copy
            let device_fd = unsafe { libc::dup(fd) };
            if device_fd < 0 {
                return Err(Error::Netstack(format!(
                    "dup failed: {}",
                    std::io::Error::last_os_error()
                )));
            }

            // Build the smoltcp device and interface
            let device = device::FdDevice::new(device_fd, mtu);
            let mut net = interface::SmoltcpInterface::new(device, &self.config);

            // Re-origination bridge and DHCP server
            let mut reorigin = reorigin::ReoriginBridge::new(self.config.mtu);
            let mut dhcp = dhcp::DhcpServer::new(&self.config);
            dhcp.add_to_set(net.sockets_mut());

            // Poll loop
            loop {
                let mut guard = async_fd
                    .readable()
                    .await
                    .map_err(|e| Error::Netstack(format!("readable: {e}")))?;
                guard.clear_ready();

                let timestamp = smoltcp::time::Instant::now();

                // Process inbound packets (data arrives on fd)
                net.poll(timestamp);

                // Handle new TCP connections from the guest and bridge data
                reorigin.handle_new_connections(net.sockets_mut());
                reorigin.poll_bridges(net.sockets_mut());

                // Process DHCP requests
                dhcp.poll(net.sockets_mut());

                // Process egress frames (tx queued during poll/reorigin/dhcp)
                net.poll(smoltcp::time::Instant::now());
            }
        }));

        handles
    }
}
