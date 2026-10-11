//! Guest ICMP echo (ping) re-originated from the host (EGRESS-07).
//!
//! The netstack re-originates TCP and UDP, but ICMP is a separate protocol:
//! guest echo requests arrive on a smoltcp raw ICMP socket (so IP fragments
//! are already reassembled), are sent from the host through an unprivileged
//! ICMP datagram socket (no root needed on macOS), and real replies go back
//! through the raw socket from the pinged address (smoltcp fragments replies
//! larger than the MTU). Pings to the netstack gateway are answered locally,
//! because the gateway is Speck itself.
//!
//! macOS specifics (verified): the kernel keeps the echo identifier, the
//! sender must compute the ICMP checksum, and received datagrams include the
//! IPv4 header.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::time::{Duration, Instant};

use smoltcp::iface::{SocketHandle, SocketSet};
use smoltcp::phy::ChecksumCapabilities;
use smoltcp::socket::raw;
use smoltcp::wire::{Icmpv4Packet, Icmpv4Repr, IpProtocol, IpVersion, Ipv4Packet, Ipv4Repr};
use tokio::sync::mpsc;

/// How long an echo request waits for its reply before it is forgotten.
const ECHO_TIMEOUT: Duration = Duration::from_secs(10);
/// Replies queued from the host socket reader to the poll loop.
const REPLY_QUEUE: usize = 256;
/// smoltcp raw socket buffers: packet slots and payload bytes.
const RAW_SLOTS: usize = 64;
const RAW_BYTES: usize = 256 * 1024;

/// A guest echo request, with what is needed to address the reply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EchoRequest {
    pub guest_ip: Ipv4Addr,
    pub dst: Ipv4Addr,
    pub ident: u16,
    pub seq: u16,
    pub data: Vec<u8>,
}

/// Parse a guest IPv4 packet (from the raw socket) as an ICMP echo request.
pub(crate) fn echo_request(packet: &[u8]) -> Option<EchoRequest> {
    let ip = Ipv4Packet::new_checked(packet).ok()?;
    if ip.next_header() != IpProtocol::Icmp {
        return None;
    }
    let icmp = Icmpv4Packet::new_checked(ip.payload()).ok()?;
    match Icmpv4Repr::parse(&icmp, &ChecksumCapabilities::default()).ok()? {
        Icmpv4Repr::EchoRequest {
            ident,
            seq_no,
            data,
        } => Some(EchoRequest {
            guest_ip: ip.src_addr(),
            dst: ip.dst_addr(),
            ident,
            seq: seq_no,
            data: data.to_vec(),
        }),
        _ => None,
    }
}

/// IPv4 packet carrying the echo reply for `req`, sent from `req.dst`.
pub(crate) fn echo_reply_packet(req: &EchoRequest, data: &[u8]) -> Vec<u8> {
    let caps = ChecksumCapabilities::default();
    let icmp = Icmpv4Repr::EchoReply {
        ident: req.ident,
        seq_no: req.seq,
        data,
    };
    let ip = Ipv4Repr {
        src_addr: req.dst,
        dst_addr: req.guest_ip,
        next_header: IpProtocol::Icmp,
        payload_len: icmp.buffer_len(),
        hop_limit: 64,
    };
    let mut buf = vec![0u8; ip.buffer_len() + icmp.buffer_len()];
    let mut packet = Ipv4Packet::new_unchecked(&mut buf[..]);
    ip.emit(&mut packet, &caps);
    icmp.emit(
        &mut Icmpv4Packet::new_unchecked(packet.payload_mut()),
        &caps,
    );
    buf
}

/// ICMP echo request bytes (header + data, checksum filled in) for the host socket.
fn echo_request_bytes(ident: u16, seq: u16, data: &[u8]) -> Vec<u8> {
    let repr = Icmpv4Repr::EchoRequest {
        ident,
        seq_no: seq,
        data,
    };
    let mut buf = vec![0u8; repr.buffer_len()];
    repr.emit(
        &mut Icmpv4Packet::new_unchecked(&mut buf[..]),
        &ChecksumCapabilities::default(),
    );
    buf
}

/// Parse a datagram from the host ICMP socket (IPv4 header included on
/// macOS) as an echo reply: (ident, seq, data).
fn parse_host_reply(datagram: &[u8]) -> Option<(u16, u16, Vec<u8>)> {
    let offset = match datagram.first()? >> 4 {
        4 => usize::from(datagram[0] & 0x0f) * 4,
        _ => 0,
    };
    let icmp = Icmpv4Packet::new_checked(datagram.get(offset..)?).ok()?;
    match Icmpv4Repr::parse(&icmp, &ChecksumCapabilities::default()).ok()? {
        Icmpv4Repr::EchoReply {
            ident,
            seq_no,
            data,
        } => Some((ident, seq_no, data.to_vec())),
        _ => None,
    }
}

type ReplyKey = (Ipv4Addr, u16, u16);

pub(crate) struct IcmpReorigin {
    gateway: Ipv4Addr,
    raw: SocketHandle,
    socket: Option<std::net::UdpSocket>,
    socket_failed: bool,
    inflight: HashMap<ReplyKey, (EchoRequest, Instant)>,
    replies_tx: mpsc::Sender<(ReplyKey, Vec<u8>)>,
    replies_rx: mpsc::Receiver<(ReplyKey, Vec<u8>)>,
}

impl IcmpReorigin {
    /// Add the raw ICMP socket that receives guest pings to `sockets`.
    pub fn new(gateway: Ipv4Addr, sockets: &mut SocketSet<'static>) -> Self {
        let (replies_tx, replies_rx) = mpsc::channel(REPLY_QUEUE);
        let buffer = || {
            raw::PacketBuffer::new(
                vec![raw::PacketMetadata::EMPTY; RAW_SLOTS],
                vec![0u8; RAW_BYTES],
            )
        };
        let raw = sockets.add(raw::Socket::new(
            Some(IpVersion::Ipv4),
            Some(IpProtocol::Icmp),
            buffer(),
            buffer(),
        ));
        Self {
            gateway,
            raw,
            socket: None,
            socket_failed: false,
            inflight: HashMap::new(),
            replies_tx,
            replies_rx,
        }
    }

    /// Forward new guest pings and deliver replies; expire unanswered ones.
    pub fn poll(&mut self, sockets: &mut SocketSet<'static>) {
        let mut requests = Vec::new();
        {
            let socket = sockets.get_mut::<raw::Socket>(self.raw);
            while let Ok(packet) = socket.recv() {
                if let Some(req) = echo_request(packet) {
                    requests.push(req);
                }
            }
        }
        let mut replies = Vec::new();
        for req in requests {
            if let Some(reply) = self.submit(req) {
                replies.push(reply);
            }
        }
        while let Ok((key, data)) = self.replies_rx.try_recv() {
            if let Some((req, _)) = self.inflight.remove(&key) {
                replies.push(echo_reply_packet(&req, &data));
            }
        }
        self.inflight
            .retain(|_, (_, sent)| sent.elapsed() < ECHO_TIMEOUT);

        let socket = sockets.get_mut::<raw::Socket>(self.raw);
        for reply in replies {
            // A full TX buffer drops the reply (ICMP semantics).
            let _ = socket.send_slice(&reply);
        }
    }

    /// Answer a gateway ping locally (returned) or forward it from the host.
    fn submit(&mut self, req: EchoRequest) -> Option<Vec<u8>> {
        if req.dst == self.gateway {
            return Some(echo_reply_packet(&req, &req.data));
        }
        let dst = req.dst;
        if dst.is_loopback() || dst.is_unspecified() || dst.is_broadcast() || dst.is_multicast() {
            return None;
        }
        let socket = self.socket()?;
        let bytes = echo_request_bytes(req.ident, req.seq, &req.data);
        if let Err(e) = socket.send_to(&bytes, SocketAddrV4::new(dst, 0)) {
            tracing::debug!(%dst, error = %e, "icmp echo send failed");
            return None;
        }
        self.inflight
            .insert((dst, req.ident, req.seq), (req, Instant::now()));
        None
    }

    /// Whether replies are still expected.
    pub fn is_active(&self) -> bool {
        !self.inflight.is_empty()
    }

    /// The shared host ICMP socket, created on first use with a reader task.
    fn socket(&mut self) -> Option<&std::net::UdpSocket> {
        if self.socket.is_none() && !self.socket_failed {
            match open_host_socket(self.replies_tx.clone()) {
                Ok(s) => self.socket = Some(s),
                Err(e) => {
                    self.socket_failed = true;
                    tracing::warn!(error = %e, "host ICMP socket unavailable; guest ping disabled");
                }
            }
        }
        self.socket.as_ref()
    }
}

/// Unprivileged ICMP datagram socket. The std UdpSocket wrapper only issues
/// sendto/recvfrom, which work on any datagram socket.
fn open_host_socket(tx: mpsc::Sender<(ReplyKey, Vec<u8>)>) -> std::io::Result<std::net::UdpSocket> {
    let raw = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::DGRAM,
        Some(socket2::Protocol::ICMPV4),
    )?;
    raw.set_nonblocking(true)?;
    let send: std::net::UdpSocket = raw.into();
    let recv = tokio::net::UdpSocket::from_std(send.try_clone()?)?;
    tokio::spawn(async move {
        let mut buf = vec![0u8; 65536];
        while let Ok((n, from)) = recv.recv_from(&mut buf).await {
            let std::net::SocketAddr::V4(from) = from else {
                continue;
            };
            if let Some((ident, seq, data)) = parse_host_reply(&buf[..n])
                && tx.send(((*from.ip(), ident, seq), data)).await.is_err()
            {
                break;
            }
        }
    });
    Ok(send)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(dst: [u8; 4]) -> EchoRequest {
        EchoRequest {
            guest_ip: Ipv4Addr::new(172, 16, 0, 2),
            dst: Ipv4Addr::from(dst),
            ident: 0x1234,
            seq: 7,
            data: b"speck-ping".to_vec(),
        }
    }

    /// Guest echo request IPv4 packet.
    fn request_packet(req: &EchoRequest) -> Vec<u8> {
        let caps = ChecksumCapabilities::default();
        let icmp = Icmpv4Repr::EchoRequest {
            ident: req.ident,
            seq_no: req.seq,
            data: &req.data,
        };
        let ip = Ipv4Repr {
            src_addr: req.guest_ip,
            dst_addr: req.dst,
            next_header: IpProtocol::Icmp,
            payload_len: icmp.buffer_len(),
            hop_limit: 64,
        };
        let mut buf = vec![0u8; ip.buffer_len() + icmp.buffer_len()];
        let mut packet = Ipv4Packet::new_unchecked(&mut buf[..]);
        ip.emit(&mut packet, &caps);
        icmp.emit(
            &mut Icmpv4Packet::new_unchecked(packet.payload_mut()),
            &caps,
        );
        buf
    }

    #[test]
    fn echo_requests_are_recognised() {
        let req = request([1, 1, 1, 1]);
        assert_eq!(echo_request(&request_packet(&req)), Some(req));
        assert_eq!(echo_request(&[0u8; 20]), None);
    }

    #[test]
    fn reply_comes_from_the_pinged_address() {
        let req = request([1, 1, 1, 1]);
        let reply = echo_reply_packet(&req, b"pong-data");
        let ip = Ipv4Packet::new_checked(&reply[..]).unwrap();
        assert!(ip.verify_checksum());
        assert_eq!(ip.src_addr(), Ipv4Addr::new(1, 1, 1, 1));
        assert_eq!(ip.dst_addr(), req.guest_ip);
        let icmp = Icmpv4Packet::new_checked(ip.payload()).unwrap();
        assert_eq!(
            Icmpv4Repr::parse(&icmp, &ChecksumCapabilities::default()).unwrap(),
            Icmpv4Repr::EchoReply {
                ident: 0x1234,
                seq_no: 7,
                data: b"pong-data"
            }
        );
    }

    #[test]
    fn host_replies_parse_with_and_without_ip_header() {
        let icmp = {
            let repr = Icmpv4Repr::EchoReply {
                ident: 9,
                seq_no: 3,
                data: b"x",
            };
            let mut b = vec![0u8; repr.buffer_len()];
            repr.emit(
                &mut Icmpv4Packet::new_unchecked(&mut b[..]),
                &ChecksumCapabilities::default(),
            );
            b
        };
        let mut with_ip = vec![0x45u8];
        with_ip.extend_from_slice(&[0u8; 19]);
        with_ip.extend_from_slice(&icmp);
        assert_eq!(parse_host_reply(&with_ip), Some((9, 3, b"x".to_vec())));
        assert_eq!(parse_host_reply(&icmp), Some((9, 3, b"x".to_vec())));
        // Echo requests (e.g. our own looped back) are not replies.
        assert_eq!(parse_host_reply(&echo_request_bytes(9, 3, b"x")), None);
    }

    #[tokio::test]
    async fn gateway_pings_are_answered_locally() {
        let mut sockets = SocketSet::new(vec![]);
        let mut icmp = IcmpReorigin::new(Ipv4Addr::new(172, 16, 0, 1), &mut sockets);
        let reply = icmp.submit(request([172, 16, 0, 1])).unwrap();
        assert!(icmp.socket.is_none(), "no host socket for the gateway");
        let ip = Ipv4Packet::new_checked(&reply[..]).unwrap();
        assert_eq!(ip.src_addr(), Ipv4Addr::new(172, 16, 0, 1));
    }
}
