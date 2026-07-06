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

                    // Guest → Host: drain smoltcp recv buffer into the host TcpStream.
                    // Loop to avoid leaving data in the buffer when the guest sends a
                    // burst larger than MTU.
                    let mut g2h_err = false;
                    while socket.can_recv() {
                        let mut buf = vec![0u8; 16384];
                        match socket.recv_slice(&mut buf) {
                            Ok(0) => break,
                            Ok(len) => {
                                if stream.write_all(&buf[..len]).is_err() {
                                    g2h_err = true;
                                    break;
                                }
                            }
                            Err(_) => break,
                        }
                    }

                    if g2h_err {
                        let _ = stream.shutdown(Shutdown::Both);
                        // socket borrow already released by the inner block; close below
                        true
                    } else {
                        // Host → Guest: limit read to available smoltcp send capacity so we
                        // never lose bytes when send_slice can't accept the full read.
                        let send_avail = socket.send_capacity().saturating_sub(socket.send_queue());
                        if send_avail == 0 {
                            // TX buffer full; try again next poll cycle
                            self.bridges.insert(handle, stream);
                            false
                        } else {
                            let read_cap = send_avail.min(65536);
                            let mut host_buf = vec![0u8; read_cap];
                            match stream.read(&mut host_buf) {
                                Ok(0) => true,
                                Ok(n) => {
                                    // n <= send_avail, so send_slice accepts all n bytes
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
                        // local_endpoint is the DESTINATION the guest wanted to reach
                        // (smoltcp updates it from Unspecified to the real dst on accept).
                        // remote_endpoint is the guest's SOURCE — not where we connect to.
                        let endpoint = tcp_socket.local_endpoint()?;
                        if let IpAddress::Ipv4(v4) = endpoint.addr {
                            let o = v4.octets();
                            // Skip loopback and the virtual gateway/subnet itself
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

    #[allow(dead_code)]
    pub fn bridge_count(&self) -> usize {
        self.bridges.len()
    }
}
