use smoltcp::iface::SocketSet;
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
