use smoltcp::iface::SocketHandle;
use smoltcp::iface::SocketSet;
use smoltcp::socket::udp;
use smoltcp::storage::{PacketBuffer, PacketMetadata};
use smoltcp::wire::{IpAddress, IpEndpoint, Ipv4Address};

use crate::config;

const DHCP_SERVER_PORT: u16 = 67;
const DHCP_CLIENT_PORT: u16 = 68;
const MAGIC_COOKIE: [u8; 4] = [0x63, 0x82, 0x53, 0x63];

const OPT_DHCP_MSG_TYPE: u8 = 53;
const OPT_SUBNET_MASK: u8 = 1;
const OPT_ROUTER: u8 = 3;
const OPT_DNS_SERVER: u8 = 6;
const OPT_LEASE_TIME: u8 = 51;
const OPT_SERVER_ID: u8 = 54;
const OPT_END: u8 = 255;

const DHCP_DISCOVER: u8 = 1;
const DHCP_OFFER: u8 = 2;
const DHCP_REQUEST: u8 = 3;
const DHCP_ACK: u8 = 5;

pub(crate) struct DhcpServer {
    handle: Option<SocketHandle>,
    offered_ip: Ipv4Address,
    server_ip: Ipv4Address,
    gateway_ip: Ipv4Address,
    dns_ip: Ipv4Address,
    subnet_mask: Ipv4Address,
    lease_time_secs: u32,
}

impl DhcpServer {
    pub fn new(config: &config::NetworkConfig) -> Self {
        let octets = config.guest_ip.octets();
        let gw_octets = config.gateway.octets();
        Self {
            handle: None,
            offered_ip: Ipv4Address::new(octets[0], octets[1], octets[2], octets[3]),
            server_ip: Ipv4Address::new(gw_octets[0], gw_octets[1], gw_octets[2], gw_octets[3]),
            gateway_ip: Ipv4Address::new(gw_octets[0], gw_octets[1], gw_octets[2], gw_octets[3]),
            dns_ip: Ipv4Address::new(gw_octets[0], gw_octets[1], gw_octets[2], gw_octets[3]),
            subnet_mask: Ipv4Address::new(255, 255, 255, 0),
            lease_time_secs: 3600,
        }
    }

    pub fn add_to_set(&mut self, sockets: &mut SocketSet) {
        let rx_buffer = PacketBuffer::new(vec![PacketMetadata::EMPTY], vec![0u8; 548]);
        let tx_buffer = PacketBuffer::new(vec![PacketMetadata::EMPTY], vec![0u8; 548]);
        let mut socket = udp::Socket::new(rx_buffer, tx_buffer);
        let _ = socket.bind(DHCP_SERVER_PORT);
        let handle = sockets.add(socket);
        self.handle = Some(handle);
    }

    pub fn poll(&mut self, sockets: &mut SocketSet) {
        let handle = match self.handle {
            Some(h) => h,
            None => return,
        };

        let socket = sockets.get_mut::<udp::Socket>(handle);
        if !socket.can_recv() {
            return;
        }

        let mut buf = [0u8; 548];
        let (len, _meta) = match socket.recv_slice(&mut buf) {
            Ok(l) => l,
            Err(_) => return,
        };

        if len < 240 {
            return;
        }

        let data = &buf[..len];

        let msg_type = match parse_dhcp_msg_type(data) {
            Some(t) => t,
            None => return,
        };

        let xid = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
        let chaddr = &data[28..44];

        let response = match msg_type {
            DHCP_DISCOVER => Some(build_dhcp_response(
                xid,
                DHCP_OFFER,
                self.offered_ip,
                self.server_ip,
                self.subnet_mask,
                self.gateway_ip,
                self.dns_ip,
                self.lease_time_secs,
                chaddr,
                data,
            )),
            DHCP_REQUEST => {
                let requested_ip = get_option(data, 50);
                let ip_match = match requested_ip {
                    Some(ip_bytes) if ip_bytes.len() >= 4 => {
                        ip_bytes[0..4] == self.offered_ip.octets()
                    }
                    _ => true,
                };
                if ip_match {
                    Some(build_dhcp_response(
                        xid,
                        DHCP_ACK,
                        self.offered_ip,
                        self.server_ip,
                        self.subnet_mask,
                        self.gateway_ip,
                        self.dns_ip,
                        self.lease_time_secs,
                        chaddr,
                        data,
                    ))
                } else {
                    None
                }
            }
            _ => None,
        };

        if let Some(resp) = response {
            let endpoint = IpEndpoint {
                addr: IpAddress::v4(255, 255, 255, 255),
                port: DHCP_CLIENT_PORT,
            };
            let _ = socket.send_slice(&resp, endpoint);
        }
    }
}

fn parse_dhcp_msg_type(data: &[u8]) -> Option<u8> {
    let options = data.get(240..)?;
    if options.len() < 4 {
        return None;
    }
    if options[..4] != MAGIC_COOKIE {
        return None;
    }
    let mut i = 4;
    while i < options.len() {
        match options[i] {
            OPT_END => break,
            0 => i += 1,
            tag => {
                if i + 1 >= options.len() {
                    break;
                }
                let len = options[i + 1] as usize;
                if tag == OPT_DHCP_MSG_TYPE && len >= 1 && i + 2 < options.len() {
                    return Some(options[i + 2]);
                }
                i += 2 + len;
            }
        }
    }
    None
}

fn get_option(data: &[u8], opt: u8) -> Option<&[u8]> {
    let options = data.get(240..)?;
    if options.len() < 4 || options[..4] != MAGIC_COOKIE {
        return None;
    }
    let mut i = 4;
    while i < options.len() {
        match options[i] {
            OPT_END => break,
            0 => i += 1,
            tag => {
                if i + 1 >= options.len() {
                    break;
                }
                let len = options[i + 1] as usize;
                if tag == opt && i + 2 + len <= options.len() {
                    return Some(&options[i + 2..i + 2 + len]);
                }
                i += 2 + len;
            }
        }
    }
    None
}

fn build_dhcp_response(
    xid: u32,
    msg_type: u8,
    yiaddr: Ipv4Address,
    siaddr: Ipv4Address,
    subnet_mask: Ipv4Address,
    gateway: Ipv4Address,
    dns: Ipv4Address,
    lease_time: u32,
    chaddr: &[u8],
    _request: &[u8],
) -> Vec<u8> {
    let mut pkt = Vec::with_capacity(300);

    pkt.push(2);
    pkt.push(1);
    pkt.push(6);
    pkt.push(0);
    pkt.extend_from_slice(&xid.to_be_bytes());
    pkt.extend_from_slice(&[0, 0]);
    pkt.extend_from_slice(&[0x80, 0x00]);
    pkt.extend_from_slice(&[0, 0, 0, 0]);
    pkt.extend_from_slice(&yiaddr.octets());
    pkt.extend_from_slice(&siaddr.octets());
    pkt.extend_from_slice(&[0, 0, 0, 0]);
    pkt.extend_from_slice(&chaddr[..16.min(chaddr.len())]);
    while pkt.len() < 44 {
        pkt.push(0);
    }
    while pkt.len() < 108 {
        pkt.push(0);
    }
    while pkt.len() < 236 {
        pkt.push(0);
    }
    pkt.extend_from_slice(&MAGIC_COOKIE);
    pkt.push(OPT_DHCP_MSG_TYPE);
    pkt.push(1);
    pkt.push(msg_type);
    pkt.push(OPT_SUBNET_MASK);
    pkt.push(4);
    pkt.extend_from_slice(&subnet_mask.octets());
    pkt.push(OPT_ROUTER);
    pkt.push(4);
    pkt.extend_from_slice(&gateway.octets());
    pkt.push(OPT_DNS_SERVER);
    pkt.push(4);
    pkt.extend_from_slice(&dns.octets());
    pkt.push(OPT_LEASE_TIME);
    pkt.push(4);
    pkt.extend_from_slice(&lease_time.to_be_bytes());
    pkt.push(OPT_SERVER_ID);
    pkt.push(4);
    pkt.extend_from_slice(&siaddr.octets());
    pkt.push(OPT_END);

    pkt
}
