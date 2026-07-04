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
        if fl_before >= 0 {
            // Mask only O_NONBLOCK; preserve any other file-status flags on the fd.
            unsafe { libc::fcntl(vsock_fd, libc::F_SETFL, fl_before & !libc::O_NONBLOCK) };
        }
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
            if query_len == 0 {
                continue;
            }
            if query_len > buf.len() {
                // Consume and discard the oversized payload so the length-prefixed
                // stream stays in sync for subsequent queries (CR-01).
                let mut discard = vec![0u8; query_len];
                if !read_exact_fd(vsock_fd, &mut discard) {
                    return Ok(());
                }
                tracing::warn!(fd = vsock_fd, query_len, "dns-proxy: oversized query discarded");
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
                    let mut resp = servers
                        .iter()
                        .find_map(|&ns| direct_dns_query(ns, &buf[..query_len]))
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

/// Resolve a domain via the macOS system resolver (`getaddrinfo`) and build a
/// DNS response that honours the query QTYPE.
///
/// Returns:
/// * `Some(resp)` — a complete DNS response: NOERROR with A/AAAA answers, or a
///   negative response (NXDOMAIN / NODATA) with the correct RCODE.
/// * `None` — transient resolution failure; the caller should emit SERVFAIL.
///
/// NXDOMAIN is preserved for non-existent domains on the default path so guest
/// resolvers can negatively cache (DNS-06 applies only to the VPN-scoped path).
fn resolve_dns(domain: &str, query: &[u8]) -> Option<Vec<u8>> {
    let id = [query[0], query[1]];

    // Locate the question section and extract QTYPE.
    let qname_end = find_qname_end(query, 12)?;
    let qtype_off = qname_end + 1;
    if qtype_off + 2 > query.len() {
        return None;
    }
    let qtype = u16::from_be_bytes([query[qtype_off], query[qtype_off + 1]]);
    let question_end = qname_end + 5; // null terminator (1) + QTYPE (2) + QCLASS (2)
    if question_end > query.len() {
        return None;
    }
    let question = &query[12..question_end];

    let addr_str = format!("{domain}:0");
    match addr_str.to_socket_addrs() {
        Err(e) => {
            // EAI_NONAME ("not known") → NXDOMAIN so guests can negatively cache.
            // Transient failures (EAI_AGAIN/EAI_FAIL) → SERVFAIL via None.
            let nxdomain = e.kind() == std::io::ErrorKind::NotFound
                || e.to_string().contains("not known");
            if nxdomain {
                Some(build_dns_response(id, question, 0, &[], 3))
            } else {
                None
            }
        }
        Ok(iter) => match qtype {
            1 => {
                let addrs: Vec<std::net::Ipv4Addr> = iter
                    .filter_map(|sa| match sa {
                        std::net::SocketAddr::V4(v4) => Some(*v4.ip()),
                        _ => None,
                    })
                    .collect();
                let answers = build_a_answers(&addrs);
                Some(build_dns_response(id, question, addrs.len() as u16, &answers, 0))
            }
            28 => {
                let addrs: Vec<std::net::Ipv6Addr> = iter
                    .filter_map(|sa| match sa {
                        std::net::SocketAddr::V6(v6) => Some(*v6.ip()),
                        _ => None,
                    })
                    .collect();
                let answers = build_aaaa_answers(&addrs);
                Some(build_dns_response(id, question, addrs.len() as u16, &answers, 0))
            }
            _ => {
                // Unsupported QTYPE via getaddrinfo: return NODATA (NOERROR, 0 answers).
                Some(build_dns_response(id, question, 0, &[], 0))
            }
        },
    }
}

/// Build a DNS response: header (QR|RD|RA, `rcode`), QDCOUNT=1, the echoed
/// `question` section, and `ancount` answer records taken from `answers`.
fn build_dns_response(
    id: [u8; 2],
    question: &[u8],
    ancount: u16,
    answers: &[u8],
    rcode: u8,
) -> Vec<u8> {
    let mut resp = Vec::with_capacity(512);
    resp.extend_from_slice(&id); // ID
    resp.extend_from_slice(&[0x81, 0x80 | (rcode & 0x0f)]); // flags: QR=1, RD=1, RA=1, RCODE
    resp.extend_from_slice(&[0x00, 0x01]); // QDCOUNT = 1
    resp.extend_from_slice(&ancount.to_be_bytes()); // ANCOUNT
    resp.extend_from_slice(&[0x00, 0x00]); // NSCOUNT = 0
    resp.extend_from_slice(&[0x00, 0x00]); // ARCOUNT = 0
    resp.extend_from_slice(question); // echoed question (QNAME + null + QTYPE + QCLASS)
    resp.extend_from_slice(answers); // answer records
    resp
}

/// Encode one A (TYPE=1) answer record per IPv4 address. NAME is a compression
/// pointer (0xc0 0x0c) to the question QNAME at offset 12.
fn build_a_answers(addrs: &[std::net::Ipv4Addr]) -> Vec<u8> {
    let mut out = Vec::with_capacity(addrs.len() * 16);
    for addr in addrs {
        out.extend_from_slice(&[0xc0, 0x0c]); // NAME pointer
        out.extend_from_slice(&[0x00, 0x01]); // TYPE = A
        out.extend_from_slice(&[0x00, 0x01]); // CLASS = IN
        out.extend_from_slice(&[0x00, 0x00, 0x00, 0x3c]); // TTL = 60
        out.extend_from_slice(&[0x00, 0x04]); // RDLENGTH = 4
        out.extend_from_slice(&addr.octets());
    }
    out
}

/// Encode one AAAA (TYPE=28) answer record per IPv6 address.
fn build_aaaa_answers(addrs: &[std::net::Ipv6Addr]) -> Vec<u8> {
    let mut out = Vec::with_capacity(addrs.len() * 28);
    for addr in addrs {
        out.extend_from_slice(&[0xc0, 0x0c]); // NAME pointer
        out.extend_from_slice(&[0x00, 0x1c]); // TYPE = AAAA (28)
        out.extend_from_slice(&[0x00, 0x01]); // CLASS = IN
        out.extend_from_slice(&[0x00, 0x00, 0x00, 0x3c]); // TTL = 60
        out.extend_from_slice(&[0x00, 0x10]); // RDLENGTH = 16
        out.extend_from_slice(&addr.octets());
    }
    out
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

    #[test]
    fn build_dns_response_encodes_nxdomain_rcode() {
        // ID=0x1234, one question, rcode=3 (NXDOMAIN), no answers.
        let resp = build_dns_response([0x12, 0x34], &[0x07, b'e', b'x', b'a', b'm', b'p', b'l', b'e', 0x00, 0x00, 0x01, 0x00, 0x01], 0, &[], 3);
        assert_eq!(&resp[0..2], &[0x12, 0x34]); // ID echoed
        assert_eq!(resp[2], 0x81); // QR=1, RD=1
        assert_eq!(resp[3], 0x83); // RA=1, RCODE=3
        assert_eq!(&resp[4..6], &[0x00, 0x01]); // QDCOUNT
        assert_eq!(&resp[6..8], &[0x00, 0x00]); // ANCOUNT=0
    }

    #[test]
    fn build_dns_response_encodes_noerror_rcode() {
        let resp = build_dns_response([0x00, 0x00], &[], 0, &[], 0);
        assert_eq!(resp[3], 0x80); // RA=1, RCODE=0
    }

    #[test]
    fn build_a_answers_record_format() {
        let addrs = vec!["1.2.3.4".parse().unwrap()];
        let out = build_a_answers(&addrs);
        // 2 NAME pointer + 2 TYPE + 2 CLASS + 4 TTL + 2 RDLENGTH + 4 RDATA = 16
        assert_eq!(out.len(), 16);
        assert_eq!(&out[0..2], &[0xc0, 0x0c]); // NAME pointer
        assert_eq!(&out[2..4], &[0x00, 0x01]); // TYPE = A
        assert_eq!(&out[4..6], &[0x00, 0x01]); // CLASS = IN
        assert_eq!(&out[6..10], &[0x00, 0x00, 0x00, 0x3c]); // TTL = 60
        assert_eq!(&out[10..12], &[0x00, 0x04]); // RDLENGTH = 4
        assert_eq!(&out[12..16], &[1, 2, 3, 4]); // RDATA
    }

    #[test]
    fn build_aaaa_answers_record_format() {
        let addr: std::net::Ipv6Addr = "2001:db8::1".parse().unwrap();
        let out = build_aaaa_answers(&[addr]);
        // 2 NAME + 2 TYPE + 2 CLASS + 4 TTL + 2 RDLENGTH + 16 RDATA = 28
        assert_eq!(out.len(), 28);
        assert_eq!(&out[2..4], &[0x00, 0x1c]); // TYPE = AAAA (28)
        assert_eq!(&out[10..12], &[0x00, 0x10]); // RDLENGTH = 16
        // 2001:db8::1 -> 2001:0db8:0000:...:0001
        assert_eq!(&out[12..16], &[0x20, 0x01, 0x0d, 0xb8]);
        assert_eq!(&out[24..28], &[0x00, 0x00, 0x00, 0x01]);
    }
}
