use smoltcp::iface::{SocketHandle, SocketSet};
use smoltcp::socket::tcp;

/// Ensure at least `desired` TCP listener sockets exist for `port`.
///
/// Called each poll cycle so that listener slots consumed by accepted
/// connections are replenished. `From<u16>` converts the port to a
/// `IpListenEndpoint` with `addr: None`, matching any destination IP
/// (transparent proxy mode).
pub fn ensure_listeners(sockets: &mut SocketSet, port: u16, desired: usize) {
    let current = sockets
        .iter()
        .filter(|(_, socket)| match socket {
            smoltcp::socket::Socket::Tcp(tcp) => {
                tcp.state() == tcp::State::Listen && tcp.listen_endpoint().port == port
            }
            _ => false,
        })
        .count();

    for _ in current..desired {
        let rx_buf = tcp::SocketBuffer::new(vec![0u8; 1024 * 1024]);
        let tx_buf = tcp::SocketBuffer::new(vec![0u8; 1024 * 1024]);
        let mut socket = tcp::Socket::new(rx_buf, tx_buf);
        // `port` as u16 → IpListenEndpoint { addr: None, port } = accept on any IP
        if socket.listen(port).is_ok() {
            sockets.add(socket);
        }
    }
}

/// Ports with listeners kept open permanently. 443 and 80 cover registry pulls
/// and most web traffic, so they skip the on-demand path's extra poll cycle.
const PREWARM: &[(u16, usize)] = &[(443, 8), (80, 4)];

fn prewarm(port: u16) -> usize {
    PREWARM
        .iter()
        .find(|(p, _)| *p == port)
        .map_or(0, |(_, n)| *n)
}

/// Ensure listeners for the pre-warmed ports plus one per held guest SYN.
///
/// `syn_ports` has one entry per SYN frame waiting in the device, so a burst
/// of connections to one port gets enough listeners to accept all of them.
pub fn ensure_listeners_for(sockets: &mut SocketSet, syn_ports: &[u16]) {
    for (port, n) in PREWARM {
        let wanted = syn_ports.iter().filter(|p| *p == port).count();
        ensure_listeners(sockets, *port, (*n).max(wanted));
    }
    let mut ports: Vec<u16> = syn_ports.to_vec();
    ports.sort_unstable();
    ports.dedup();
    for port in ports.into_iter().filter(|p| prewarm(*p) == 0) {
        let wanted = syn_ports.iter().filter(|p| **p == port).count();
        ensure_listeners(sockets, port, wanted);
    }
}

/// Remove listeners beyond the pre-warmed count for each port.
///
/// On-demand listeners are only needed until smoltcp has processed the SYN
/// they were created for. Call this only after the released SYNs were polled.
pub fn trim_surplus_listeners(sockets: &mut SocketSet) {
    let mut kept: std::collections::HashMap<u16, usize> = std::collections::HashMap::new();
    let surplus: Vec<SocketHandle> = sockets
        .iter()
        .filter_map(|(handle, socket)| match socket {
            smoltcp::socket::Socket::Tcp(tcp) if tcp.state() == tcp::State::Listen => {
                let port = tcp.listen_endpoint().port;
                let count = kept.entry(port).or_insert(0);
                if *count < prewarm(port) {
                    *count += 1;
                    None
                } else {
                    Some(handle)
                }
            }
            _ => None,
        })
        .collect();
    for handle in surplus {
        sockets.remove(handle);
    }
}

/// Remove TCP sockets that reached `Closed` and are no longer owned by a bridge.
///
/// Without this every accepted connection leaves its socket (and 2 MB of
/// buffers) in the set forever, and each poll cycle scans an ever-growing set.
/// `TimeWait` sockets are left alone: smoltcp moves them to `Closed` itself
/// once the timer expires. `in_use` must return `true` for handles a bridge
/// still references, so `get_mut` on them never panics.
pub fn sweep_closed(sockets: &mut SocketSet, in_use: impl Fn(SocketHandle) -> bool) -> usize {
    let closed: Vec<SocketHandle> = sockets
        .iter()
        .filter_map(|(handle, socket)| match socket {
            smoltcp::socket::Socket::Tcp(tcp) if tcp.state() == tcp::State::Closed => {
                (!in_use(handle)).then_some(handle)
            }
            _ => None,
        })
        .collect();

    for handle in &closed {
        sockets.remove(*handle);
    }
    closed.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tcp_count(sockets: &SocketSet) -> usize {
        sockets
            .iter()
            .filter(|(_, s)| matches!(s, smoltcp::socket::Socket::Tcp(_)))
            .count()
    }

    /// Simulates the poll loop: each cycle one listener is consumed by a
    /// connection that later finishes (aborted → Closed), then the loop
    /// replenishes listeners. The set must stay flat instead of growing by one
    /// socket per connection.
    #[test]
    fn sweep_keeps_socket_set_flat_across_connection_cycles() {
        let mut sockets = SocketSet::new(vec![]);
        for _ in 0..1000 {
            ensure_listeners(&mut sockets, 443, 8);
            let handle = sockets
                .iter()
                .find(|(_, s)| matches!(s, smoltcp::socket::Socket::Tcp(t) if t.state() == tcp::State::Listen))
                .map(|(h, _)| h)
                .unwrap();
            sockets.get_mut::<tcp::Socket>(handle).abort();
            sweep_closed(&mut sockets, |_| false);
        }
        ensure_listeners(&mut sockets, 443, 8);
        assert_eq!(tcp_count(&sockets), 8);
    }

    fn listen_count(sockets: &SocketSet, port: u16) -> usize {
        sockets
            .iter()
            .filter(|(_, s)| {
                matches!(s, smoltcp::socket::Socket::Tcp(t)
                    if t.state() == tcp::State::Listen && t.listen_endpoint().port == port)
            })
            .count()
    }

    #[test]
    fn on_demand_listeners_cover_every_held_syn_and_are_trimmed_after() {
        let mut sockets = SocketSet::new(vec![]);
        ensure_listeners_for(&mut sockets, &[22, 5432, 5432, 443]);
        assert_eq!(listen_count(&sockets, 22), 1);
        assert_eq!(listen_count(&sockets, 5432), 2);
        assert_eq!(listen_count(&sockets, 443), 8);
        assert_eq!(listen_count(&sockets, 80), 4);

        trim_surplus_listeners(&mut sockets);
        assert_eq!(listen_count(&sockets, 22), 0);
        assert_eq!(listen_count(&sockets, 5432), 0);
        assert_eq!(listen_count(&sockets, 443), 8);
        assert_eq!(listen_count(&sockets, 80), 4);
    }

    #[test]
    fn sweep_skips_sockets_still_owned_by_a_bridge() {
        let mut sockets = SocketSet::new(vec![]);
        let owned = sockets.add(tcp::Socket::new(
            tcp::SocketBuffer::new(vec![0u8; 64]),
            tcp::SocketBuffer::new(vec![0u8; 64]),
        ));
        let orphan = sockets.add(tcp::Socket::new(
            tcp::SocketBuffer::new(vec![0u8; 64]),
            tcp::SocketBuffer::new(vec![0u8; 64]),
        ));
        ensure_listeners(&mut sockets, 80, 2);

        let removed = sweep_closed(&mut sockets, |h| h == owned);

        assert_eq!(removed, 1);
        assert!(sockets.iter().any(|(h, _)| h == owned));
        assert!(!sockets.iter().any(|(h, _)| h == orphan));
        assert_eq!(tcp_count(&sockets), 3); // owned + 2 listeners
    }
}
