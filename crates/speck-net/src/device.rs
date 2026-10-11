use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetFrame, EthernetProtocol, IpProtocol, Ipv4Packet, TcpPacket};
use std::collections::VecDeque;
use std::os::unix::io::RawFd;

/// A wrapper around a raw file descriptor that implements `smoltcp::phy::Device`.
///
/// Reads/writes raw L2 Ethernet frames from a socketpair fd. The fd is set to
/// `O_NONBLOCK` and is closed on `Drop`.
pub struct FdDevice {
    fd: RawFd,
    mtu: usize,
    /// Guest TCP SYN frames held back until a listener exists for their port.
    held_syns: Vec<Vec<u8>>,
    /// Frames released from `held_syns`, handed to smoltcp before new reads.
    ready: VecDeque<Vec<u8>>,
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
        Self {
            fd,
            mtu,
            held_syns: Vec::new(),
            ready: VecDeque::new(),
        }
    }

    /// Destination ports of the held SYN frames, one entry per frame.
    pub fn held_syn_ports(&self) -> Vec<u16> {
        self.held_syns
            .iter()
            .filter_map(|f| syn_dst_port(f))
            .collect()
    }

    /// Hand held SYN frames to smoltcp on the next poll (listeners now exist).
    pub fn release_held_syns(&mut self) {
        self.ready.extend(self.held_syns.drain(..));
    }

    /// Whether frames are waiting that will not trigger fd readiness again.
    pub fn has_pending_frames(&self) -> bool {
        !self.held_syns.is_empty() || !self.ready.is_empty()
    }

    /// Whether released frames are still waiting to be processed by smoltcp.
    pub fn has_ready_frames(&self) -> bool {
        !self.ready.is_empty()
    }
}

/// Destination port of a guest IPv4 TCP SYN (without ACK), else `None`.
///
/// Used to open a smoltcp listener on demand before the SYN reaches smoltcp,
/// so guest connections to any port are accepted instead of reset.
pub(crate) fn syn_dst_port(frame: &[u8]) -> Option<u16> {
    let eth = EthernetFrame::new_checked(frame).ok()?;
    if eth.ethertype() != EthernetProtocol::Ipv4 {
        return None;
    }
    let ip = Ipv4Packet::new_checked(eth.payload()).ok()?;
    if ip.next_header() != IpProtocol::Tcp {
        return None;
    }
    let tcp = TcpPacket::new_checked(ip.payload()).ok()?;
    (tcp.syn() && !tcp.ack()).then(|| tcp.dst_port())
}

impl Drop for FdDevice {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.fd);
        }
    }
}

/// Token that holds received frame data.
pub struct FdRxToken(Vec<u8>);

impl RxToken for FdRxToken {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(&self.0)
    }
}

/// Token that holds the fd for writing outbound frames.
pub struct FdTxToken {
    fd: RawFd,
}

impl TxToken for FdTxToken {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut buf = vec![0u8; len];
        let result = f(&mut buf);
        let n = unsafe { libc::write(self.fd, buf.as_ptr() as *const libc::c_void, len) };
        if n < 0 || (n as usize) != len {
            tracing::debug!(
                len,
                result = n,
                "dropped egress frame (socketpair write failed/short)"
            );
        }
        result
    }
}

impl Device for FdDevice {
    type RxToken<'a> = FdRxToken;
    type TxToken<'a> = FdTxToken;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        if let Some(frame) = self.ready.pop_front() {
            return Some((FdRxToken(frame), FdTxToken { fd: self.fd }));
        }
        loop {
            let mut rx_buf = vec![0u8; self.mtu + 14]; // MTU + Ethernet header
            match unsafe {
                libc::read(
                    self.fd,
                    rx_buf.as_mut_ptr() as *mut libc::c_void,
                    rx_buf.len(),
                )
            } {
                n if n > 0 => {
                    rx_buf.truncate(n as usize);
                    if syn_dst_port(&rx_buf).is_some() {
                        self.held_syns.push(rx_buf);
                        continue;
                    }
                    return Some((FdRxToken(rx_buf), FdTxToken { fd: self.fd }));
                }
                _ => return None, // EAGAIN, EWOULDBLOCK, or error → no packet
            }
        }
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        Some(FdTxToken { fd: self.fd })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = self.mtu;
        // smoltcp multiplies max_burst_size by MSS and clamps the advertised TCP
        // receive window in every outgoing segment — `Some(1)` forced stop-and-wait
        // for guest→host transfers. Unbounded is safe now that the socketpair has
        // 4MB kernel buffers.
        caps.max_burst_size = None;
        caps
    }
}

#[cfg(test)]
mod tests {
    use super::syn_dst_port;

    /// Ethernet + IPv4 + 20-byte TCP header with the given flags and dst port.
    fn frame(proto: u8, flags: u8, dst_port: u16) -> Vec<u8> {
        let mut f = vec![0u8; 14 + 20 + 20];
        f[12..14].copy_from_slice(&0x0800u16.to_be_bytes()); // IPv4
        let ip = &mut f[14..34];
        ip[0] = 0x45; // version 4, IHL 5
        ip[2..4].copy_from_slice(&40u16.to_be_bytes()); // total length
        ip[8] = 64; // TTL
        ip[9] = proto;
        ip[12..16].copy_from_slice(&[172, 16, 0, 2]);
        ip[16..20].copy_from_slice(&[140, 82, 121, 4]);
        let tcp = &mut f[34..54];
        tcp[0..2].copy_from_slice(&40000u16.to_be_bytes());
        tcp[2..4].copy_from_slice(&dst_port.to_be_bytes());
        tcp[12] = 0x50; // data offset 5
        tcp[13] = flags;
        f
    }

    const SYN: u8 = 0x02;
    const ACK: u8 = 0x10;

    #[test]
    fn pure_syn_yields_destination_port() {
        assert_eq!(syn_dst_port(&frame(6, SYN, 22)), Some(22));
        assert_eq!(syn_dst_port(&frame(6, SYN, 5432)), Some(5432));
    }

    #[test]
    fn syn_ack_ack_and_non_tcp_are_ignored() {
        assert_eq!(syn_dst_port(&frame(6, SYN | ACK, 22)), None);
        assert_eq!(syn_dst_port(&frame(6, ACK, 22)), None);
        assert_eq!(syn_dst_port(&frame(17, SYN, 22)), None); // UDP
        assert_eq!(syn_dst_port(&[0u8; 10]), None); // truncated
    }
}
