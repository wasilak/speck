use std::net::SocketAddrV4;

use smoltcp::iface::{SocketHandle, SocketSet};
use smoltcp::socket::tcp;
use smoltcp::wire::{IpAddress, IpListenEndpoint};

/// Open a listener for exactly `dst`, whose guest SYN the host already
/// connected for (see `connector`).
///
/// The listener is bound to the destination address, not just the port, so a
/// SYN whose host connect failed is never accepted by it and gets a reset
/// from smoltcp instead. With `any_ip` every destination counts as local.
pub fn open_listener_for(sockets: &mut SocketSet, dst: SocketAddrV4) {
    let rx_buf = tcp::SocketBuffer::new(vec![0u8; 1024 * 1024]);
    let tx_buf = tcp::SocketBuffer::new(vec![0u8; 1024 * 1024]);
    let mut socket = tcp::Socket::new(rx_buf, tx_buf);
    let endpoint = IpListenEndpoint {
        addr: Some(IpAddress::Ipv4(*dst.ip())),
        port: dst.port(),
    };
    if socket.listen(endpoint).is_ok() {
        sockets.add(socket);
    }
}

/// Remove listeners that did not pick up their SYN.
///
/// Listeners only live between `open_listener_for` and smoltcp processing the
/// released SYN; one still listening afterwards matched nothing (the guest
/// gave up). Call this only after all released SYNs were polled.
pub fn trim_listeners(sockets: &mut SocketSet) {
    let idle: Vec<SocketHandle> = sockets
        .iter()
        .filter_map(|(handle, socket)| match socket {
            smoltcp::socket::Socket::Tcp(tcp) if tcp.state() == tcp::State::Listen => Some(handle),
            _ => None,
        })
        .collect();
    for handle in idle {
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
    use std::net::Ipv4Addr;

    fn dst(port: u16) -> SocketAddrV4 {
        SocketAddrV4::new(Ipv4Addr::new(140, 82, 121, 4), port)
    }

    fn tcp_count(sockets: &SocketSet) -> usize {
        sockets
            .iter()
            .filter(|(_, s)| matches!(s, smoltcp::socket::Socket::Tcp(_)))
            .count()
    }

    fn first_listener(sockets: &SocketSet) -> Option<SocketHandle> {
        sockets
            .iter()
            .find(|(_, s)| {
                matches!(s, smoltcp::socket::Socket::Tcp(t) if t.state() == tcp::State::Listen)
            })
            .map(|(h, _)| h)
    }

    /// Simulates the poll loop: each cycle a listener is opened for a SYN,
    /// the connection later finishes (aborted → Closed), and the loop sweeps.
    /// The set must stay empty instead of growing by one socket per connection.
    #[test]
    fn sweep_keeps_socket_set_flat_across_connection_cycles() {
        let mut sockets = SocketSet::new(vec![]);
        for i in 0..1000u16 {
            open_listener_for(&mut sockets, dst(1000 + i));
            let handle = first_listener(&sockets).unwrap();
            sockets.get_mut::<tcp::Socket>(handle).abort();
            sweep_closed(&mut sockets, |_| false);
        }
        assert_eq!(tcp_count(&sockets), 0);
    }

    #[test]
    fn listener_is_bound_to_the_exact_destination() {
        let mut sockets = SocketSet::new(vec![]);
        open_listener_for(&mut sockets, dst(5432));
        let handle = first_listener(&sockets).unwrap();
        let ep = sockets.get::<tcp::Socket>(handle).listen_endpoint();
        assert_eq!(ep.addr, Some(IpAddress::v4(140, 82, 121, 4)));
        assert_eq!(ep.port, 5432);
    }

    #[test]
    fn trim_removes_only_idle_listeners() {
        let mut sockets = SocketSet::new(vec![]);
        let closed = sockets.add(tcp::Socket::new(
            tcp::SocketBuffer::new(vec![0u8; 64]),
            tcp::SocketBuffer::new(vec![0u8; 64]),
        ));
        open_listener_for(&mut sockets, dst(22));
        open_listener_for(&mut sockets, dst(22));

        trim_listeners(&mut sockets);

        assert_eq!(tcp_count(&sockets), 1);
        assert!(sockets.iter().any(|(h, _)| h == closed));
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
        open_listener_for(&mut sockets, dst(80));

        let removed = sweep_closed(&mut sockets, |h| h == owned);

        assert_eq!(removed, 1);
        assert!(sockets.iter().any(|(h, _)| h == owned));
        assert!(!sockets.iter().any(|(h, _)| h == orphan));
        assert_eq!(tcp_count(&sockets), 2); // owned + listener
    }
}
