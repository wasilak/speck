use smoltcp::iface::{Interface, SocketSet};
use smoltcp::wire::{EthernetAddress, IpAddress, IpCidr};

use crate::config;

/// Manages a smoltcp [`Interface`] and [`SocketSet`] for the Speck netstack.
///
/// Owns the smoltcp device and provides a [`poll()`](Self::poll) method that drives
/// packet processing. Future modules (re-origination, DNS) will add sockets to the
/// socket set.
pub struct SmoltcpInterface {
    iface: Interface,
    device: super::device::FdDevice,
    sockets: SocketSet<'static>,
}

impl SmoltcpInterface {
    /// Create a new `SmoltcpInterface` from a device and network config.
    pub fn new(mut device: super::device::FdDevice, config: &config::NetworkConfig) -> Self {
        // Configure the interface with the MAC address from NetworkConfig
        let ethernet_addr = EthernetAddress(config.mac);
        let iface_config = smoltcp::iface::Config::new(ethernet_addr.into());
        let mut iface = Interface::new(iface_config, &mut device, smoltcp::time::Instant::now());

        // Configure smoltcp as the gateway (172.16.0.1) for the guest subnet.
        // set_any_ip(true) enables "AnyIP" / transparent proxy mode: has_ip_addr()
        // returns true for any destination, so smoltcp responds to ARP for any IP
        // the guest sends to (including ghcr.io, registry-1.docker.io, etc.) and
        // accepts TCP connections for any destination IP. This is the correct
        // smoltcp API for transparent proxying.
        let gw_octets = config.gateway.octets();
        iface.update_ip_addrs(|addrs| {
            addrs
                .push(IpCidr::new(
                    IpAddress::v4(gw_octets[0], gw_octets[1], gw_octets[2], gw_octets[3]),
                    config.subnet_prefix,
                ))
                .unwrap();
            // Published-port proxying dials containers on Docker's default bridge
            // network (`172.17.0.0/16`) from the bridge gateway address
            // `172.17.0.1`. Add that address explicitly so reply packets to the
            // source IP are accepted by the interface instead of being dropped.
            addrs
                .push(IpCidr::new(IpAddress::v4(172, 17, 0, 1), 16))
                .unwrap();
        });
        iface.set_any_ip(true);

        // Empty socket set — sockets are added by re-origination and DNS modules later
        let sockets = SocketSet::new(vec![]);

        Self {
            iface,
            device,
            sockets,
        }
    }

    /// Poll the interface, processing inbound and outbound packets.
    ///
    /// Returns `true` if socket state may have changed (new data available, etc.).
    pub fn poll(&mut self, timestamp: smoltcp::time::Instant) -> bool {
        matches!(
            self.iface
                .poll(timestamp, &mut self.device, &mut self.sockets),
            smoltcp::iface::PollResult::SocketStateChanged
        )
    }

    /// Read-only access to the interface.
    #[allow(dead_code)]
    pub fn iface(&self) -> &Interface {
        &self.iface
    }

    /// Mutable access to the interface.
    #[allow(dead_code)]
    pub fn iface_mut(&mut self) -> &mut Interface {
        &mut self.iface
    }

    /// Read-only access to the socket set.
    #[allow(dead_code)]
    pub fn sockets(&self) -> &SocketSet<'static> {
        &self.sockets
    }

    /// Mutable access to the socket set.
    #[allow(dead_code)]
    pub fn sockets_mut(&mut self) -> &mut SocketSet<'static> {
        &mut self.sockets
    }

    /// Poll published-port listeners and active-connect accepted host streams.
    pub fn poll_port_publish(&mut self, bridge: &mut crate::port_publish::PortPublishBridge) {
        let cx = self.iface.context();
        bridge.poll_new_host_connections(&mut self.sockets, cx);
    }
}
