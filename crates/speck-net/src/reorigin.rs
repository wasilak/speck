use std::collections::HashMap;
use std::time::Duration;

use smoltcp::iface::{SocketHandle, SocketSet};
use smoltcp::socket::tcp;
use smoltcp::wire::{IpAddress, IpEndpoint};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};

use crate::mtu;

/// Number of 64 KB chunks buffered per direction per bridge.
/// 128 × 64 KB = 8 MB of read-ahead per active connection.
const CHANNEL_CAP: usize = 128;
const CHUNK_SIZE: usize = 65536;

/// Per-connection bridge: channels connect the smoltcp poll loop (sync) to
/// per-direction tokio tasks (async) that do the actual TCP I/O.
///
/// H2G (host → guest): TCP reader task fills h2g_rx; poll loop drains it
///   into smoltcp's TX buffer every cycle.
/// G2H (guest → host): poll loop drains smoltcp's RX buffer into g2h_tx;
///   TCP writer task drains g2h_rx and writes to the host stream.
struct BridgeState {
    h2g_rx: mpsc::Receiver<Vec<u8>>,
    /// Chunk partially consumed into smoltcp TX last cycle (offset = bytes already sent).
    h2g_overflow: Option<(Vec<u8>, usize)>,

    g2h_tx: mpsc::Sender<Vec<u8>>,
    /// Data read from smoltcp RX that didn't fit in g2h_tx (channel was full).
    g2h_stash: Option<Vec<u8>>,
}

impl BridgeState {
    fn new(stream: tokio::net::TcpStream) -> Self {
        let (h2g_tx, h2g_rx) = mpsc::channel::<Vec<u8>>(CHANNEL_CAP);
        let (g2h_tx, mut g2h_rx) = mpsc::channel::<Vec<u8>>(CHANNEL_CAP);

        let (mut read_half, mut write_half) = stream.into_split();

        // H2G: read from the host TCP stream and enqueue for the poll loop.
        tokio::spawn(async move {
            let mut buf = vec![0u8; CHUNK_SIZE];
            loop {
                match read_half.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if h2g_tx.send(buf[..n].to_vec()).await.is_err() {
                            break; // poll loop dropped h2g_rx → bridge torn down
                        }
                    }
                }
            }
        });

        // G2H: drain the g2h channel and write to the host TCP stream.
        tokio::spawn(async move {
            while let Some(data) = g2h_rx.recv().await {
                if write_half.write_all(&data).await.is_err() {
                    break;
                }
            }
        });

        BridgeState {
            h2g_rx,
            h2g_overflow: None,
            g2h_tx,
            g2h_stash: None,
        }
    }

    /// Drain buffered host data into smoltcp's TX buffer.
    /// Returns `true` when the host TCP connection has closed (h2g channel disconnected).
    fn poll_h2g(&mut self, socket: &mut tcp::Socket) -> bool {
        // Finish sending any chunk that was only partially consumed last cycle.
        if let Some((data, offset)) = &mut self.h2g_overflow {
            let send_avail = socket.send_capacity().saturating_sub(socket.send_queue());
            if send_avail == 0 {
                return false;
            }
            let remaining = &data[*offset..];
            let n = remaining.len().min(send_avail);
            let _ = socket.send_slice(&remaining[..n]);
            *offset += n;
            if *offset >= data.len() {
                self.h2g_overflow = None;
            } else {
                return false; // TX buffer still full
            }
        }

        // Drain as many channel items as smoltcp's TX buffer will accept.
        loop {
            let send_avail = socket.send_capacity().saturating_sub(socket.send_queue());
            match self.h2g_rx.try_recv() {
                Ok(data) => {
                    let n = data.len().min(send_avail);
                    if n > 0 {
                        let _ = socket.send_slice(&data[..n]);
                    }
                    if n < data.len() {
                        self.h2g_overflow = Some((data, n));
                        return false; // TX buffer full; resume next cycle
                    }
                }
                Err(mpsc::error::TryRecvError::Empty) => return false,
                Err(mpsc::error::TryRecvError::Disconnected) => return true, // host closed
            }
        }
    }

    /// Drain smoltcp's RX buffer into the g2h channel.
    /// Returns `true` when the G2H TCP writer task has exited.
    fn poll_g2h(&mut self, socket: &mut tcp::Socket) -> bool {
        // Retry stashed data that didn't fit in the channel last cycle.
        if let Some(data) = self.g2h_stash.take() {
            match self.g2h_tx.try_send(data) {
                Ok(_) => {}
                Err(mpsc::error::TrySendError::Full(d)) => {
                    self.g2h_stash = Some(d);
                    return false; // Channel still full → backpressure to guest
                }
                Err(mpsc::error::TrySendError::Closed(_)) => return true,
            }
        }

        while socket.can_recv() {
            let mut buf = vec![0u8; CHUNK_SIZE];
            match socket.recv_slice(&mut buf) {
                Ok(0) => break,
                Ok(n) => match self.g2h_tx.try_send(buf[..n].to_vec()) {
                    Ok(_) => {}
                    Err(mpsc::error::TrySendError::Full(d)) => {
                        self.g2h_stash = Some(d);
                        break; // Channel full → stop draining → TCP zero-window to guest
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => return true,
                },
                Err(_) => break,
            }
        }
        false
    }
}

pub(crate) struct ReoriginBridge {
    bridges: HashMap<SocketHandle, BridgeState>,
    /// Async connects in flight. Each resolves to a tokio TcpStream (or error).
    pending: HashMap<SocketHandle, oneshot::Receiver<std::io::Result<tokio::net::TcpStream>>>,
    max_bridges: usize,
    mtu: u16,
}

impl ReoriginBridge {
    pub fn new(mtu: u16) -> Self {
        Self {
            bridges: HashMap::new(),
            pending: HashMap::new(),
            max_bridges: 256,
            mtu,
        }
    }

    /// Promote completed async connects into active bridges; close sockets on failure.
    pub fn poll_pending(&mut self, sockets: &mut SocketSet) {
        let handles: Vec<SocketHandle> = self.pending.keys().copied().collect();
        for handle in handles {
            let rx = self.pending.get_mut(&handle).unwrap();
            match rx.try_recv() {
                Ok(Ok(stream)) => {
                    self.pending.remove(&handle);
                    let socket = sockets.get_mut::<tcp::Socket>(handle);
                    if socket.is_open() {
                        self.bridges.insert(handle, BridgeState::new(stream));
                    } else {
                        socket.close(); // guest closed while we were connecting
                    }
                }
                Ok(Err(_)) => {
                    self.pending.remove(&handle);
                    sockets.get_mut::<tcp::Socket>(handle).close();
                }
                Err(oneshot::error::TryRecvError::Empty) => {}
                Err(oneshot::error::TryRecvError::Closed) => {
                    self.pending.remove(&handle);
                    sockets.get_mut::<tcp::Socket>(handle).close();
                }
            }
        }
    }

    /// Forward data between active bridges and smoltcp sockets.
    pub fn poll_bridges(&mut self, sockets: &mut SocketSet) {
        let handles: Vec<SocketHandle> = self.bridges.keys().copied().collect();
        for handle in handles {
            let socket = sockets.get_mut::<tcp::Socket>(handle);
            if !socket.is_open() {
                self.bridges.remove(&handle);
                socket.close();
                continue;
            }

            let bridge = self.bridges.get_mut(&handle).unwrap();
            let h2g_done = bridge.poll_h2g(socket);
            let g2h_done = bridge.poll_g2h(socket);

            if h2g_done || g2h_done {
                self.bridges.remove(&handle);
                socket.close();
            }
        }
    }

    /// Detect newly-established smoltcp sockets and spawn async host connects.
    pub fn handle_new_connections(&mut self, sockets: &mut SocketSet) {
        if self.bridges.len() + self.pending.len() >= self.max_bridges {
            return;
        }

        let new_connections: Vec<(SocketHandle, IpEndpoint)> = sockets
            .iter()
            .filter_map(|(handle, socket)| {
                if self.bridges.contains_key(&handle) || self.pending.contains_key(&handle) {
                    return None;
                }
                match socket {
                    smoltcp::socket::Socket::Tcp(tcp_socket) => {
                        if tcp_socket.state() != tcp::State::Established {
                            return None;
                        }
                        let endpoint = tcp_socket.local_endpoint()?;
                        if let IpAddress::Ipv4(v4) = endpoint.addr {
                            let o = v4.octets();
                            if o[0] == 127 || o == [0, 0, 0, 0] {
                                return None;
                            }
                            if o == [172, 16, 0, 1] || o == [172, 16, 0, 2] {
                                return None;
                            }
                        }
                        Some((handle, endpoint))
                    }
                    _ => None,
                }
            })
            .collect();

        let _mss = mtu::mss_for_mtu(self.mtu);

        for (handle, endpoint) in new_connections {
            let addr = format!("{}:{}", endpoint.addr, endpoint.port);
            let (tx, rx) = oneshot::channel::<std::io::Result<tokio::net::TcpStream>>();

            tokio::spawn(async move {
                let result = async {
                    let stream = tokio::net::TcpStream::connect(&addr).await?;
                    // Enable TCP keepalive so WARP/NAT doesn't kill long-lived
                    // connections mid-transfer (e.g. large blob downloads).
                    let std_stream = stream.into_std()?;
                    let sock: socket2::Socket = std_stream.into();
                    let ka = socket2::TcpKeepalive::new()
                        .with_time(Duration::from_secs(30))
                        .with_interval(Duration::from_secs(10));
                    let _ = sock.set_tcp_keepalive(&ka);
                    let std_stream: std::net::TcpStream = sock.into();
                    std_stream.set_nonblocking(true)?;
                    let stream = tokio::net::TcpStream::from_std(std_stream)?;
                    Ok(stream)
                }
                .await;
                let _ = tx.send(result);
            });

            self.pending.insert(handle, rx);
        }
    }

    #[allow(dead_code)]
    pub fn bridge_count(&self) -> usize {
        self.bridges.len()
    }
}
