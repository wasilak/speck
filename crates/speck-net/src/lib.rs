pub mod config;
pub mod device;
pub(crate) mod dhcp;
pub(crate) mod dns;
pub mod error;
pub mod resolver_table;
pub mod interface;
pub mod mtu;
pub mod port_publish;
pub(crate) mod reorigin;
pub(crate) mod tcp_listener;

pub use dns::spawn_dns_proxy;
pub use error::{Error, Result};
pub use resolver_table::{ResolverTable, spawn_resolver_watcher};
pub use mtu::{detect_host_mtu, mss_for_mtu};
pub use port_publish::{PortMapConfig, PortPublishBridge};

use std::os::unix::io::RawFd;
use std::time::Duration;
use tokio::sync::mpsc;

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
        port_maps: Vec<PortMapConfig>,
        mut port_map_rx: Option<mpsc::Receiver<PortMapConfig>>,
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
            let mut port_publish = PortPublishBridge::new(self.config.mtu);
            for port_map in port_maps {
                if let Err(e) = port_publish.add_port_map(port_map) {
                    tracing::warn!(host_port = port_map.host_port, error = %e, "failed to add initial port map");
                }
            }
            let mut dhcp = dhcp::DhcpServer::new(&self.config);
            dhcp.add_to_set(net.sockets_mut());

            // Seed initial TCP listener sockets for transparent re-origination.
            // Ports 443 (HTTPS) and 80 (HTTP) cover registry pulls.
            tcp_listener::ensure_listeners(net.sockets_mut(), 443, 8);
            tcp_listener::ensure_listeners(net.sockets_mut(), 80, 4);

            // Poll loop
            loop {
                // Replenish listener slots consumed by accepted connections.
                tcp_listener::ensure_listeners(net.sockets_mut(), 443, 8);
                tcp_listener::ensure_listeners(net.sockets_mut(), 80, 4);

                // Use a short timeout so host→guest data is forwarded promptly even
                // when the guest is waiting (e.g., TLS handshake ServerHello).
                match tokio::time::timeout(Duration::from_millis(5), async_fd.readable()).await {
                    Ok(Ok(mut guard)) => guard.clear_ready(),
                    Ok(Err(e)) => {
                        return Err(Error::Netstack(format!("readable: {e}")));
                    }
                    Err(_) => {} // timeout — fall through to poll bridges
                }

                let timestamp = smoltcp::time::Instant::now();

                // Process inbound packets (data arrives on fd)
                net.poll(timestamp);

                // Handle new TCP connections from the guest and bridge data
                reorigin.handle_new_connections(net.sockets_mut());
                reorigin.poll_bridges(net.sockets_mut());

                if let Some(rx) = port_map_rx.as_mut() {
                    while let Ok(port_map) = rx.try_recv() {
                        if let Err(e) = port_publish.add_port_map(port_map) {
                            tracing::warn!(host_port = port_map.host_port, error = %e, "failed to add dynamic port map");
                        }
                    }
                }

                // Handle host TCP connections for published ports and bridge data.
                net.poll_port_publish(&mut port_publish);
                port_publish.poll_bridges(net.sockets_mut());

                // Process DHCP requests
                dhcp.poll(net.sockets_mut());

                // Process egress frames (tx queued during poll/reorigin/dhcp)
                net.poll(smoltcp::time::Instant::now());
            }
        }));

        handles
    }
}
