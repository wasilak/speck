use std::net::ToSocketAddrs;
use std::os::unix::io::RawFd;

use crate::error;

/// Spawn a thread that reads length-prefixed DNS queries from a vsock fd,
/// resolves them via the macOS system resolver (getaddrinfo), and writes responses.
///
/// Queries matching a VPN-scoped suffix in `resolver_rx` are forwarded directly
/// to the VPN nameserver via UDP (DNS-04). All other queries fall through to
/// `resolve_dns` (getaddrinfo — DNS-02, DNS-03).
///
/// Runs in a dedicated std::thread with blocking I/O to avoid tokio::fs::File
/// compatibility issues with vsock socket fds (which may be non-blocking).
/// The fd is set to blocking mode on entry.
pub fn spawn_dns_proxy(
    vsock_fd: RawFd,
    mut resolver_rx: tokio::sync::watch::Receiver<crate::resolver_table::ResolverTable>,
) -> tokio::task::JoinHandle<std::result::Result<(), error::Error>> {
    tokio::task::spawn_blocking(move || {
        // Ensure the fd is in blocking mode — VZ framework may return non-blocking fds.
        let fl_before = unsafe { libc::fcntl(vsock_fd, libc::F_GETFL, 0) };
        unsafe { libc::fcntl(vsock_fd, libc::F_SETFL, 0) };
        let fl_after = unsafe { libc::fcntl(vsock_fd, libc::F_GETFL, 0) };
        tracing::debug!(fd = vsock_fd, flags_before = fl_before, flags_after = fl_after, "dns-proxy started");

        let mut buf = vec![0u8; 4096];

        loop {
            // Read u16 BE length prefix
            let mut len_buf = [0u8; 2];
            tracing::debug!(fd = vsock_fd, "dns-proxy waiting for query");
            if !read_exact_fd(vsock_fd, &mut len_buf) {
                tracing::debug!(fd = vsock_fd, "dns-proxy EOF, exiting");
                return Ok(());
            }

            let query_len = u16::from_be_bytes(len_buf) as usize;
            tracing::debug!(fd = vsock_fd, len = query_len, "dns-proxy query received");
            if query_len == 0 || query_len > buf.len() {
                continue;
            }

            if !read_exact_fd(vsock_fd, &mut buf[..query_len]) {
                return Ok(());
            }

            let qname = extract_qname(&buf[..query_len]);
            tracing::debug!(domain = ?qname, "dns-proxy resolving");
            let current_table = resolver_rx.borrow_and_update().clone();
            let response = if let Some(domain) = qname {
                if let Some(servers) = current_table.find_resolver(&domain) {
                    // VPN-scoped path: direct UDP to first VPN nameserver (DNS-04)
                    let mut resp = direct_dns_query(servers[0], &buf[..query_len])
                        .unwrap_or_else(|| build_servfail_response(&buf[..query_len]));
                    translate_nxdomain_to_servfail(&mut resp);
                    Some(resp)
                } else {
                    // Default path: macOS system resolver via getaddrinfo (DNS-02, DNS-03)
                    resolve_dns(&domain, &buf[..query_len])
                }
            } else {
                None
            };

            let to_send = if let Some(resp_bytes) = response {
                tracing::debug!(bytes = resp_bytes.len(), "dns-proxy resolved ok");
                resp_bytes
            } else {
                tracing::debug!(fd = vsock_fd, "dns-proxy resolve failed, sending SERVFAIL");
                build_servfail_response(&buf[..query_len])
            };

            let len_bytes = (to_send.len() as u16).to_be_bytes();
            if !write_exact_fd(vsock_fd, &len_bytes) || !write_exact_fd(vsock_fd, &to_send) {
                return Ok(());
            }
        }
    })
}

fn read_exact_fd(fd: RawFd, buf: &mut [u8]) -> bool {
    let mut pos = 0;
    while pos < buf.len() {
        let n = unsafe {
            libc::read(fd, buf.as_mut_ptr().add(pos) as *mut libc::c_void, buf.len() - pos)
        };
        if n <= 0 {
            return false;
        }
        pos += n as usize;
    }
    true
}

fn write_exact_fd(fd: RawFd, buf: &[u8]) -> bool {
    let mut pos = 0;
    while pos < buf.len() {
        let n = unsafe {
            libc::write(fd, buf.as_ptr().add(pos) as *const libc::c_void, buf.len() - pos)
        };
        if n <= 0 {
            return false;
        }
        pos += n as usize;
    }
    true
}

/// Extract the QNAME (domain name) from a DNS query.
/// DNS header is 12 bytes, question starts at offset 12.
/// Returns None if parsing fails or if compression pointers are used.
fn extract_qname(data: &[u8]) -> Option<String> {
    if data.len() < 12 {
        return None;
    }
    let mut pos = 12;
    let mut labels = Vec::new();
    loop {
        let label_len = data.get(pos).copied()?;
        if label_len == 0 {
            break;
        }
        // Check for compression pointer (top 2 bits set)
        if label_len & 0xc0 == 0xc0 {
            return None;
        }
        if pos + 1 + label_len as usize > data.len() {
            return None;
        }
        let label = data.get(pos + 1..pos + 1 + label_len as usize)?;
        labels.push(std::str::from_utf8(label).ok()?.to_lowercase());
        pos += 1 + label_len as usize;
    }
    if labels.is_empty() {
        None
    } else {
        Some(labels.join("."))
    }
}

/// Resolve a domain via getaddrinfo and build a minimal DNS response.
/// Uses the original query header for ID and echoes the question section.
fn resolve_dns(domain: &str, query: &[u8]) -> Option<Vec<u8>> {
    let addr_str = format!("{domain}:0");
    let addrs: Vec<std::net::Ipv4Addr> = addr_str
        .to_socket_addrs()
        .ok()?
        .filter_map(|sa| match sa {
            std::net::SocketAddr::V4(v4) => Some(*v4.ip()),
            _ => None,
        })
        .collect();

    if addrs.is_empty() {
        return None;
    }

    let id = [query[0], query[1]];

    // Build DNS response
    let mut resp = Vec::with_capacity(512);

    // Header
    resp.extend_from_slice(&id); // ID
    resp.extend_from_slice(&[0x81, 0x80]); // flags: QR, RD, RA, no error
    resp.extend_from_slice(&[0x00, 0x01]); // QDCOUNT = 1
    let ancount = (addrs.len() as u16).to_be_bytes();
    resp.extend_from_slice(&ancount); // ANCOUNT
    resp.extend_from_slice(&[0x00, 0x00]); // NSCOUNT = 0
    resp.extend_from_slice(&[0x00, 0x00]); // ARCOUNT = 0

    // Echo the question section from the query
    // Find where the question ends (null terminator + 4 bytes for QTYPE/QCLASS)
    let qname_end = find_qname_end(query, 12)?;
    // qname_end is the index of the null byte; question section continues with
    // the null byte itself (1) + QTYPE (2) + QCLASS (2) = 5 more bytes.
    let question_end = qname_end + 5;
    if question_end > query.len() {
        return None;
    }
    resp.extend_from_slice(&query[12..question_end]);

    // Answer section: one A record per resolved IP
    for addr in &addrs {
        // NAME pointer (pointing to the question QNAME in the question section)
        resp.extend_from_slice(&[0xc0, 0x0c]);
        // TYPE = A (1)
        resp.extend_from_slice(&[0x00, 0x01]);
        // CLASS = IN (1)
        resp.extend_from_slice(&[0x00, 0x01]);
        // TTL = 60 seconds
        resp.extend_from_slice(&[0x00, 0x00, 0x00, 0x3c]);
        // RDLENGTH = 4
        resp.extend_from_slice(&[0x00, 0x04]);
        // RDATA = IP bytes
        resp.extend_from_slice(&addr.octets());
    }

    Some(resp)
}

/// Find the end offset of the QNAME in a DNS question starting at `start`.
/// Returns the offset of the null terminator byte.
fn find_qname_end(data: &[u8], mut start: usize) -> Option<usize> {
    loop {
        let byte = data.get(start).copied()?;
        if byte == 0 {
            return Some(start);
        }
        if byte & 0xc0 == 0xc0 {
            // Compression pointer - question ends after pointer (2 bytes)
            return Some(start + 1);
        }
        start += 1 + byte as usize;
    }
}

/// Forward a DNS query directly to a VPN nameserver via UDP (DNS-04).
/// Uses a 500ms read timeout to avoid blocking the proxy loop indefinitely.
/// Returns None on any socket error, send failure, or receive timeout.
fn direct_dns_query(nameserver: std::net::IpAddr, query: &[u8]) -> Option<Vec<u8>> {
    use std::net::{SocketAddr, UdpSocket};
    use std::time::Duration;
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.set_read_timeout(Some(Duration::from_millis(500))).ok()?;
    let ns_addr = SocketAddr::new(nameserver, 53);
    socket.send_to(query, ns_addr).ok()?;
    let mut resp_buf = vec![0u8; 4096];
    match socket.recv_from(&mut resp_buf) {
        Ok((n, _)) => Some(resp_buf[..n].to_vec()),
        Err(_) => None,
    }
}

/// Rewrite RCODE NXDOMAIN (3) → SERVFAIL (2) per DNS-06.
/// RFC 1035: RCODE is the lower 4 bits of byte 3 in the DNS header.
pub(crate) fn translate_nxdomain_to_servfail(response: &mut [u8]) {
    if response.len() < 4 {
        return;
    }
    if response[3] & 0x0f == 3 {
        response[3] = (response[3] & 0xf0) | 0x02;
    }
}

/// Build a minimal SERVFAIL response (12 bytes).
fn build_servfail_response(query_header: &[u8]) -> Vec<u8> {
    let id = query_header.get(..2).unwrap_or(&[0, 0]);
    vec![
        id[0], id[1], 0x81, // flags: QR, RD, RA
        0x82, // flags: RCODE=SERVFAIL(2)
        0x00, 0x00, // QDCOUNT = 0
        0x00, 0x00, // ANCOUNT = 0
        0x00, 0x00, // NSCOUNT = 0
        0x00, 0x00, // ARCOUNT = 0
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nxdomain_to_servfail_rewrites_rcode() {
        let mut buf = vec![0u8; 12];
        buf[3] = 0x03; // RCODE = NXDOMAIN
        translate_nxdomain_to_servfail(&mut buf);
        assert_eq!(buf[3] & 0x0f, 2);
    }

    #[test]
    fn servfail_passthrough_unchanged() {
        let mut buf = vec![0u8; 12];
        buf[3] = 0x02; // RCODE = SERVFAIL
        translate_nxdomain_to_servfail(&mut buf);
        assert_eq!(buf[3] & 0x0f, 2);
    }

    #[test]
    fn noerror_not_touched() {
        let mut buf = vec![0u8; 12];
        buf[3] = 0x00; // RCODE = NOERROR
        translate_nxdomain_to_servfail(&mut buf);
        assert_eq!(buf[3] & 0x0f, 0);
    }

    #[test]
    fn too_short_does_not_panic() {
        let mut buf = vec![0u8; 3];
        translate_nxdomain_to_servfail(&mut buf);
        // No panic — early return guard prevents access to buf[3]
    }
}
