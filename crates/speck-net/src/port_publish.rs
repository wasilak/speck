use std::collections::{HashMap, HashSet};
use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};

use serde::{Deserialize, Serialize};
use smoltcp::iface::{Context, SocketHandle, SocketSet};
use smoltcp::socket::tcp;
use smoltcp::wire::IpAddress;

const GUEST_IP: IpAddress = IpAddress::v4(172, 16, 0, 2);
const GATEWAY_IP: IpAddress = IpAddress::v4(172, 16, 0, 1);
const EPHEMERAL_START: u16 = 49152;

/// A host TCP port published into the guest network namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortMapConfig {
    pub host_port: u16,
    pub container_port: u16,
}

/// Bridges host-side published TCP ports into the guest through smoltcp.
pub struct PortPublishBridge {
    bridges: HashMap<SocketHandle, TcpStream>,
    listeners: Vec<(TcpListener, PortMapConfig)>,
    next_ephemeral_port: u16,
    used_ephemeral_ports: HashSet<u16>,
    mtu: u16,
}

impl PortPublishBridge {
    pub fn new(mtu: u16) -> Self {
        Self {
            bridges: HashMap::new(),
            listeners: Vec::new(),
            next_ephemeral_port: EPHEMERAL_START,
            used_ephemeral_ports: HashSet::new(),
            mtu,
        }
    }

    pub fn add_port_map(&mut self, config: PortMapConfig) -> std::io::Result<()> {
        if self
            .listeners
            .iter()
            .any(|(_, existing)| *existing == config)
        {
            return Ok(());
        }

        let listener = TcpListener::bind(("127.0.0.1", config.host_port))?;
        listener.set_nonblocking(true)?;
        self.listeners.push((listener, config));
        Ok(())
    }

    pub fn poll_new_host_connections(&mut self, sockets: &mut SocketSet, cx: &mut Context) {
        let mut accepted = Vec::new();

        for (listener, config) in &self.listeners {
            loop {
                match listener.accept() {
                    Ok((stream, _addr)) => accepted.push((stream, *config)),
                    Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(e) => {
                        tracing::warn!(host_port = config.host_port, error = %e, "port publish accept failed");
                        break;
                    }
                }
            }
        }

        for (stream, config) in accepted {
            let Some(ephemeral_port) = self.allocate_ephemeral_port() else {
                tracing::warn!(
                    host_port = config.host_port,
                    container_port = config.container_port,
                    "no ephemeral ports available for published connection"
                );
                let _ = stream.shutdown(Shutdown::Both);
                continue;
            };

            if let Err(e) = stream.set_nonblocking(true) {
                tracing::warn!(error = %e, "failed to set published stream nonblocking");
                self.used_ephemeral_ports.remove(&ephemeral_port);
                let _ = stream.shutdown(Shutdown::Both);
                continue;
            }

            let rx_buffer = tcp::SocketBuffer::new(vec![0u8; 65535]);
            let tx_buffer = tcp::SocketBuffer::new(vec![0u8; 65535]);
            let mut socket = tcp::Socket::new(rx_buffer, tx_buffer);

            match socket.connect(
                cx,
                (GUEST_IP, config.container_port),
                (GATEWAY_IP, ephemeral_port),
            ) {
                Ok(()) => {
                    let handle = sockets.add(socket);
                    self.bridges.insert(handle, stream);
                }
                Err(e) => {
                    tracing::warn!(error = ?e, "failed to active-connect published port socket");
                    self.used_ephemeral_ports.remove(&ephemeral_port);
                    let _ = stream.shutdown(Shutdown::Both);
                }
            }
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

                    let mut host_buf = [0u8; 65536];
                    match stream.read(&mut host_buf) {
                        Ok(0) => true,
                        Ok(n) => {
                            let _ = socket.send_slice(&host_buf[..n]);
                            if socket.can_recv() {
                                let mut buf = vec![0u8; self.mtu as usize];
                                if let Ok(len) = socket.recv_slice(&mut buf) && len > 0 {
                                    let _ = stream.write_all(&buf[..len]);
                                }
                            }
                            self.bridges.insert(handle, stream);
                            false
                        }
                        Err(e) if e.kind() == ErrorKind::WouldBlock => {
                            if socket.can_recv() {
                                let mut buf = vec![0u8; self.mtu as usize];
                                if let Ok(len) = socket.recv_slice(&mut buf) && len > 0 {
                                    let _ = stream.write_all(&buf[..len]);
                                }
                            }
                            self.bridges.insert(handle, stream);
                            false
                        }
                        Err(_) => true,
                    }
                }
            };

            if close_bridge {
                self.close_bridge(handle, sockets);
            }
        }
    }

    pub fn bridge_count(&self) -> usize {
        self.bridges.len()
    }

    pub fn port_map_count(&self) -> usize {
        self.listeners.len()
    }

    pub fn next_ephemeral_port(&self) -> u16 {
        self.next_ephemeral_port
    }

    fn allocate_ephemeral_port(&mut self) -> Option<u16> {
        let start = self.next_ephemeral_port;
        loop {
            let candidate = self.next_ephemeral_port;
            self.next_ephemeral_port = if self.next_ephemeral_port == u16::MAX {
                EPHEMERAL_START
            } else {
                self.next_ephemeral_port + 1
            };

            if self.used_ephemeral_ports.insert(candidate) {
                return Some(candidate);
            }

            if self.next_ephemeral_port == start {
                return None;
            }
        }
    }

    fn close_bridge(&mut self, handle: SocketHandle, sockets: &mut SocketSet) {
        if let Some(stream) = self.bridges.remove(&handle) {
            let _ = stream.shutdown(Shutdown::Both);
        }

        let socket = sockets.get_mut::<tcp::Socket>(handle);
        if let Some(endpoint) = socket.local_endpoint() {
            self.used_ephemeral_ports.remove(&endpoint.port);
        }
        socket.close();
    }
}

#[cfg(test)]
mod tests {
    use super::{PortMapConfig, PortPublishBridge};

    #[test]
    fn test_portmap_config_serialization() {
        let config = PortMapConfig {
            host_port: 8080,
            container_port: 80,
        };

        let encoded = serde_json::to_string(&config).unwrap();
        let decoded: PortMapConfig = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, config);
    }

    #[test]
    fn test_portpublish_bridge_new() {
        let bridge = PortPublishBridge::new(1500);

        assert_eq!(bridge.bridge_count(), 0);
        assert_eq!(bridge.port_map_count(), 0);
        assert_eq!(bridge.next_ephemeral_port(), 49152);
    }
}
