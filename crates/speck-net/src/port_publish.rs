use std::collections::{HashMap, HashSet};
use std::io::{ErrorKind, Read, Write};
use std::net::Ipv4Addr;
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
    pub target_ip: Option<Ipv4Addr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortMapUpdate {
    Add(PortMapConfig),
    Remove(PortMapConfig),
}

/// Bridges host-side published TCP ports into the guest through smoltcp.
pub struct PortPublishBridge {
    bridges: HashMap<SocketHandle, PortBridgeState>,
    listeners: Vec<(TcpListener, PortMapConfig)>,
    next_ephemeral_port: u16,
    used_ephemeral_ports: HashSet<u16>,
    mtu: u16,
}

struct PortBridgeState {
    stream: TcpStream,
    h2g_overflow: Option<(Vec<u8>, usize)>,
}

impl PortBridgeState {
    fn new(stream: TcpStream) -> Self {
        Self {
            stream,
            h2g_overflow: None,
        }
    }

    fn flush_h2g(&mut self, socket: &mut tcp::Socket) {
        if let Some((data, offset)) = &mut self.h2g_overflow {
            let send_avail = socket.send_capacity().saturating_sub(socket.send_queue());
            if send_avail == 0 {
                return;
            }
            let remaining = &data[*offset..];
            let n = remaining.len().min(send_avail);
            let _ = socket.send_slice(&remaining[..n]);
            *offset += n;
            if *offset >= data.len() {
                self.h2g_overflow = None;
            }
        }
    }
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

    pub fn remove_port_map(&mut self, config: PortMapConfig) {
        self.listeners.retain(|(_, existing)| *existing != config);
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
            tracing::info!(
                host_port = config.host_port,
                guest_port = config.container_port,
                target_ip = ?config.target_ip,
                "accepted host connection for published port"
            );
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

            let rx_buffer = tcp::SocketBuffer::new(vec![0u8; 1024 * 1024]);
            let tx_buffer = tcp::SocketBuffer::new(vec![0u8; 1024 * 1024]);
            let mut socket = tcp::Socket::new(rx_buffer, tx_buffer);

            match socket.connect(
                cx,
                (
                    config.target_ip.map(IpAddress::from).unwrap_or(GUEST_IP),
                    config.container_port,
                ),
                (
                    if config.target_ip.is_some() {
                        GUEST_IP
                    } else {
                        GATEWAY_IP
                    },
                    ephemeral_port,
                ),
            ) {
                Ok(()) => {
                    tracing::info!(
                        host_port = config.host_port,
                        guest_port = config.container_port,
                        target_ip = ?config.target_ip,
                        ephemeral_port,
                        "created smoltcp published-port connection"
                    );
                    let handle = sockets.add(socket);
                    self.bridges.insert(handle, PortBridgeState::new(stream));
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
                    let mut bridge = self.bridges.remove(&handle).unwrap();

                    if socket.can_send() {
                        tracing::info!(state = ?socket.state(), "published-port socket can_send");
                        bridge.flush_h2g(socket);
                    }

                    let host_result = if socket.can_send() && bridge.h2g_overflow.is_none() {
                        let mut host_buf = [0u8; 65536];
                        match bridge.stream.read(&mut host_buf) {
                            Ok(0) => Some(true),
                            Ok(n) => {
                                tracing::info!(bytes = n, state = ?socket.state(), "read host bytes for published port");
                                let send_avail =
                                    socket.send_capacity().saturating_sub(socket.send_queue());
                                let to_send = n.min(send_avail);
                                if to_send > 0 {
                                    tracing::info!(bytes = to_send, state = ?socket.state(), "forwarding host bytes into guest published-port socket");
                                    let _ = socket.send_slice(&host_buf[..to_send]);
                                }
                                if to_send < n {
                                    bridge.h2g_overflow = Some((host_buf[..n].to_vec(), to_send));
                                }
                                Some(false)
                            }
                            Err(e) if e.kind() == ErrorKind::WouldBlock => Some(false),
                            Err(_) => Some(true),
                        }
                    } else {
                        None
                    };

                    if socket.can_recv() {
                        let mut buf = vec![0u8; self.mtu as usize];
                        if let Ok(len) = socket.recv_slice(&mut buf)
                            && len > 0
                        {
                            tracing::info!(bytes = len, state = ?socket.state(), "received guest bytes for published port");
                            let _ = bridge.stream.write_all(&buf[..len]);
                        }
                    }

                    match host_result {
                        Some(true) => true,
                        Some(false) => {
                            self.bridges.insert(handle, bridge);
                            false
                        }
                        None => {
                            self.bridges.insert(handle, bridge);
                            false
                        }
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
        if let Some(bridge) = self.bridges.remove(&handle) {
            let _ = bridge.stream.shutdown(Shutdown::Both);
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
            target_ip: None,
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

    #[test]
    fn test_remove_port_map_drops_listener() {
        let mut bridge = PortPublishBridge::new(1500);
        let config = PortMapConfig {
            host_port: 38081,
            container_port: 80,
            target_ip: None,
        };

        bridge.add_port_map(config).expect("bind listener");
        assert_eq!(bridge.port_map_count(), 1);

        bridge.remove_port_map(config);
        assert_eq!(bridge.port_map_count(), 0);
    }
}
