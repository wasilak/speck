//! Guest UDP re-originated from the host network stack.
//!
//! Each guest flow (guest endpoint → destination endpoint) gets its own host
//! UDP socket, connected to the destination, so traffic follows the live host
//! routing table (VPN/WARP) like TCP does. Replies are sent back to the guest
//! from the original destination address. smoltcp UDP sockets are opened per
//! destination port on demand: the device holds a guest datagram for one poll
//! cycle until its port has a socket.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::time::{Duration, Instant};

use smoltcp::iface::{SocketHandle, SocketSet};
use smoltcp::socket::udp;
use smoltcp::wire::{IpAddress, IpEndpoint};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;

use crate::connector::is_unroutable;

/// Idle time after which a flow (and its host socket) is dropped.
const FLOW_IDLE: Duration = Duration::from_secs(60);
/// Upper bound on concurrent flows; new flows beyond it are dropped.
const MAX_FLOWS: usize = 1024;
/// Datagrams queued from host sockets to the poll loop.
const REPLY_QUEUE: usize = 1024;
/// smoltcp per-port socket buffers: datagram slots and payload bytes.
const SOCKET_SLOTS: usize = 64;
const SOCKET_BYTES: usize = 256 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct FlowKey {
    guest: SocketAddrV4,
    dst: SocketAddrV4,
}

struct Flow {
    /// Send side: a plain nonblocking socket. tokio's `try_send` reports
    /// WouldBlock until the reactor has seen the socket writable, which would
    /// drop the first datagram of every flow.
    host: std::net::UdpSocket,
    reader: tokio::task::JoinHandle<()>,
    last_used: Instant,
}

impl Drop for Flow {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

pub(crate) struct UdpReorigin {
    /// smoltcp UDP socket per destination port, with last activity.
    ports: HashMap<u16, (SocketHandle, Instant)>,
    flows: HashMap<FlowKey, Flow>,
    replies_tx: mpsc::Sender<(FlowKey, Vec<u8>)>,
    replies_rx: mpsc::Receiver<(FlowKey, Vec<u8>)>,
}

impl Default for UdpReorigin {
    fn default() -> Self {
        let (replies_tx, replies_rx) = mpsc::channel(REPLY_QUEUE);
        Self {
            ports: HashMap::new(),
            flows: HashMap::new(),
            replies_tx,
            replies_rx,
        }
    }
}

impl UdpReorigin {
    /// Open a smoltcp UDP socket for `port` (any destination IP) if missing.
    pub fn ensure_port(&mut self, sockets: &mut SocketSet<'static>, port: u16) {
        if let Some((_, last)) = self.ports.get_mut(&port) {
            *last = Instant::now();
            return;
        }
        let mut socket = udp::Socket::new(
            udp::PacketBuffer::new(
                vec![udp::PacketMetadata::EMPTY; SOCKET_SLOTS],
                vec![0u8; SOCKET_BYTES],
            ),
            udp::PacketBuffer::new(
                vec![udp::PacketMetadata::EMPTY; SOCKET_SLOTS],
                vec![0u8; SOCKET_BYTES],
            ),
        );
        if socket.bind(port).is_ok() {
            let handle = sockets.add(socket);
            self.ports.insert(port, (handle, Instant::now()));
        }
    }

    /// Move datagrams between smoltcp sockets and host flows, and expire idle state.
    pub fn poll(&mut self, sockets: &mut SocketSet<'static>) {
        self.forward_guest_datagrams(sockets);
        self.deliver_replies(sockets);
        self.expire(sockets);
    }

    fn forward_guest_datagrams(&mut self, sockets: &mut SocketSet<'static>) {
        let mut buf = vec![0u8; 65536];
        let handles: Vec<(u16, SocketHandle)> =
            self.ports.iter().map(|(p, (h, _))| (*p, *h)).collect();
        for (port, handle) in handles {
            loop {
                let socket = sockets.get_mut::<udp::Socket>(handle);
                let Ok((n, meta)) = socket.recv_slice(&mut buf) else {
                    break;
                };
                let (Some(guest), Some(IpAddress::Ipv4(dst_ip))) =
                    (v4(meta.endpoint), meta.local_address)
                else {
                    continue;
                };
                if is_unroutable(dst_ip) || dst_ip.is_broadcast() || dst_ip.is_multicast() {
                    continue;
                }
                let key = FlowKey {
                    guest,
                    dst: SocketAddrV4::new(dst_ip, port),
                };
                if let Some((_, last)) = self.ports.get_mut(&port) {
                    *last = Instant::now();
                }
                let Some(flow) = self.flow(key) else {
                    continue;
                };
                flow.last_used = Instant::now();
                // UDP semantics: a full host socket buffer drops the datagram.
                let _ = flow.host.send(&buf[..n]);
            }
        }
    }

    /// Existing flow for `key`, or a new one with a connected host socket.
    fn flow(&mut self, key: FlowKey) -> Option<&mut Flow> {
        if !self.flows.contains_key(&key) {
            if self.flows.len() >= MAX_FLOWS {
                tracing::debug!(dst = %key.dst, "udp flow table full; dropping datagram");
                return None;
            }
            let (host, recv) = match host_socket(key.dst) {
                Ok(s) => s,
                Err(e) => {
                    tracing::debug!(dst = %key.dst, error = %e, "udp host socket failed");
                    return None;
                }
            };
            let reader = tokio::spawn(read_replies(recv, key, self.replies_tx.clone()));
            self.flows.insert(
                key,
                Flow {
                    host,
                    reader,
                    last_used: Instant::now(),
                },
            );
        }
        self.flows.get_mut(&key)
    }

    fn deliver_replies(&mut self, sockets: &mut SocketSet<'static>) {
        while let Ok((key, data)) = self.replies_rx.try_recv() {
            let Some((handle, _)) = self.ports.get(&key.dst.port()) else {
                continue;
            };
            if let Some(flow) = self.flows.get_mut(&key) {
                flow.last_used = Instant::now();
            }
            let meta = udp::UdpMetadata {
                endpoint: endpoint(key.guest),
                local_address: Some(IpAddress::Ipv4(*key.dst.ip())),
                meta: Default::default(),
            };
            // A full smoltcp TX buffer drops the datagram (UDP semantics).
            let _ = sockets
                .get_mut::<udp::Socket>(*handle)
                .send_slice(&data, meta);
        }
    }

    fn expire(&mut self, sockets: &mut SocketSet<'static>) {
        self.flows.retain(|_, f| f.last_used.elapsed() < FLOW_IDLE);
        let active: std::collections::HashSet<u16> =
            self.flows.keys().map(|k| k.dst.port()).collect();
        self.ports.retain(|port, (handle, last)| {
            let keep = active.contains(port) || last.elapsed() < FLOW_IDLE;
            if !keep {
                sockets.remove(*handle);
            }
            keep
        });
    }

    /// Whether any UDP flow is live.
    pub fn is_active(&self) -> bool {
        !self.flows.is_empty()
    }

    #[cfg(test)]
    fn flow_count(&self) -> usize {
        self.flows.len()
    }
}

/// Destination port of a guest IPv4 UDP datagram that should be re-originated.
/// DHCP (67/68) is answered by the netstack itself and is never held.
pub(crate) fn udp_dst_port(frame: &[u8]) -> Option<u16> {
    use smoltcp::wire::{EthernetFrame, EthernetProtocol, IpProtocol, Ipv4Packet, UdpPacket};
    let eth = EthernetFrame::new_checked(frame).ok()?;
    if eth.ethertype() != EthernetProtocol::Ipv4 {
        return None;
    }
    let ip = Ipv4Packet::new_checked(eth.payload()).ok()?;
    if ip.next_header() != IpProtocol::Udp {
        return None;
    }
    let dst = ip.dst_addr();
    if dst.is_broadcast() || dst.is_multicast() {
        return None;
    }
    let udp = UdpPacket::new_checked(ip.payload()).ok()?;
    let port = udp.dst_port();
    (port != 67 && port != 68).then_some(port)
}

/// Host socket connected to `dst`: a std handle for sending and a tokio
/// handle (same socket) for the reply reader task.
fn host_socket(dst: SocketAddrV4) -> std::io::Result<(std::net::UdpSocket, UdpSocket)> {
    let sock = std::net::UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))?;
    sock.connect(dst)?;
    sock.set_nonblocking(true)?;
    let recv = UdpSocket::from_std(sock.try_clone()?)?;
    Ok((sock, recv))
}

async fn read_replies(host: UdpSocket, key: FlowKey, tx: mpsc::Sender<(FlowKey, Vec<u8>)>) {
    let mut buf = vec![0u8; 65536];
    while let Ok(n) = host.recv(&mut buf).await {
        if tx.send((key, buf[..n].to_vec())).await.is_err() {
            break;
        }
    }
}

fn v4(ep: IpEndpoint) -> Option<SocketAddrV4> {
    match ep.addr {
        IpAddress::Ipv4(ip) => Some(SocketAddrV4::new(ip, ep.port)),
        #[allow(unreachable_patterns)]
        _ => None,
    }
}

fn endpoint(addr: SocketAddrV4) -> IpEndpoint {
    IpEndpoint::new(IpAddress::Ipv4(*addr.ip()), addr.port())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;

    /// Ethernet + IPv4 + UDP header to `dst`:`port`.
    fn frame(dst: [u8; 4], port: u16) -> Vec<u8> {
        let mut f = vec![0u8; 14 + 20 + 8];
        f[12..14].copy_from_slice(&0x0800u16.to_be_bytes());
        let ip = &mut f[14..34];
        ip[0] = 0x45;
        ip[2..4].copy_from_slice(&28u16.to_be_bytes());
        ip[8] = 64;
        ip[9] = 17; // UDP
        ip[12..16].copy_from_slice(&[172, 16, 0, 2]);
        ip[16..20].copy_from_slice(&dst);
        let udp = &mut f[34..42];
        udp[0..2].copy_from_slice(&40000u16.to_be_bytes());
        udp[2..4].copy_from_slice(&port.to_be_bytes());
        udp[4..6].copy_from_slice(&8u16.to_be_bytes());
        f
    }

    #[test]
    fn udp_dst_port_skips_dhcp_broadcast_and_multicast() {
        assert_eq!(udp_dst_port(&frame([162, 159, 200, 1], 123)), Some(123));
        assert_eq!(udp_dst_port(&frame([1, 1, 1, 1], 443)), Some(443));
        assert_eq!(udp_dst_port(&frame([172, 16, 0, 1], 67)), None);
        assert_eq!(udp_dst_port(&frame([255, 255, 255, 255], 5353)), None);
        assert_eq!(udp_dst_port(&frame([224, 0, 0, 251], 5353)), None);
    }

    #[test]
    fn ensure_port_opens_one_socket_per_port() {
        let mut sockets = SocketSet::new(vec![]);
        let mut u = UdpReorigin::default();
        u.ensure_port(&mut sockets, 123);
        u.ensure_port(&mut sockets, 123);
        u.ensure_port(&mut sockets, 443);
        assert_eq!(sockets.iter().count(), 2);
    }

    /// A datagram to a real UDP server on loopback goes out through a host
    /// flow and the server's reply comes back on the flow's channel.
    #[tokio::test]
    async fn flow_round_trips_through_a_host_socket() {
        let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let SocketAddr::V4(server_addr) = server.local_addr().unwrap() else {
            unreachable!()
        };
        let mut u = UdpReorigin::default();
        let key = FlowKey {
            guest: SocketAddrV4::new(Ipv4Addr::new(172, 16, 0, 2), 40000),
            dst: server_addr,
        };
        u.flow(key).unwrap().host.send(b"ping").unwrap();

        let mut buf = [0u8; 16];
        let (n, from) = server.recv_from(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"ping");
        server.send_to(b"pong", from).await.unwrap();

        let (got_key, data) = u.replies_rx.recv().await.unwrap();
        assert_eq!(got_key, key);
        assert_eq!(data, b"pong");
        assert_eq!(u.flow_count(), 1);
    }
}
