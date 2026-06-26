use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, TcpStream};

use smoltcp::iface::{SocketHandle, SocketSet};
use smoltcp::socket::tcp;
use smoltcp::wire::{IpAddress, IpEndpoint};

use crate::mtu;

pub(crate) struct ReoriginBridge {
    bridges: HashMap<SocketHandle, TcpStream>,
    max_bridges: usize,
    mtu: u16,
}

impl ReoriginBridge {
    pub fn new(mtu: u16) -> Self {
        Self {
            bridges: HashMap::new(),
            max_bridges: 256,
            mtu,
        }
    }

    pub fn poll_bridges(&mut self, sockets: &mut SocketSet) {
        let handles: Vec<SocketHandle> = self.bridges.keys().copied().collect();

        for handle in handles {
            let close_bridge = {
                let socket = sockets.get_mut::<tcp::Socket>(handle);
                if !socket.is_open() {
                    true
                } else {
                    let mut stream = self.bridges.remove(&handle).unwrap();

                    if socket.can_recv() {
                        let mut buf = vec![0u8; self.mtu as usize];
                        let n = socket.recv_slice(&mut buf);
                        if let Ok(len) = n {
                            if len > 0 {
                                let _ = stream.write_all(&buf[..len]);
                            }
                        }
                    }

                    let mut host_buf = [0u8; 65536];
                    match stream.read(&mut host_buf) {
                        Ok(0) => true,
                        Ok(n) => {
                            let _ = socket.send_slice(&host_buf[..n]);
                            self.bridges.insert(handle, stream);
                            false
                        }
                        Err(e) if e.kind() == ErrorKind::WouldBlock => {
                            self.bridges.insert(handle, stream);
                            false
                        }
                        Err(_) => true,
                    }
                }
            };

            if close_bridge {
                if let Some(stream) = self.bridges.remove(&handle) {
                    let _ = stream.shutdown(Shutdown::Both);
                }
                let socket = sockets.get_mut::<tcp::Socket>(handle);
                socket.close();
            }
        }
    }

    pub fn handle_new_connections(&mut self, sockets: &mut SocketSet) {
        if self.bridges.len() >= self.max_bridges {
            return;
        }

        let new_connections: Vec<(SocketHandle, IpEndpoint)> = sockets
            .iter()
            .filter_map(|(handle, socket)| {
                if self.bridges.contains_key(&handle) {
                    return None;
                }
                match socket {
                    smoltcp::socket::Socket::Tcp(tcp_socket) => {
                        if tcp_socket.state() != tcp::State::Established {
                            return None;
                        }
                        let endpoint = tcp_socket.remote_endpoint()?;
                        if let IpAddress::Ipv4(v4) = endpoint.addr {
                            if v4.octets()[0] == 127 {
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
            match TcpStream::connect(&addr) {
                Ok(stream) => {
                    let _ = stream.set_nonblocking(true);
                    self.bridges.insert(handle, stream);
                }
                Err(_) => {
                    let s = sockets.get_mut::<tcp::Socket>(handle);
                    s.close();
                }
            }
        }
    }

    pub fn bridge_count(&self) -> usize {
        self.bridges.len()
    }
}
