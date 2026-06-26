use std::net::ToSocketAddrs;
use std::os::fd::FromRawFd;
use std::os::unix::io::RawFd;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::error;

/// Spawn a tokio task that reads length-prefixed DNS queries from a vsock fd,
/// resolves them via the macOS system resolver (getaddrinfo), and writes responses.
pub fn spawn_dns_proxy(
    vsock_fd: RawFd,
) -> tokio::task::JoinHandle<std::result::Result<(), error::Error>> {
    tokio::task::spawn(async move {
        let mut stream = unsafe { tokio::fs::File::from_raw_fd(vsock_fd) };
        let mut buf = vec![0u8; 512];

        loop {
            // Read u16 BE length prefix
            let mut len_buf = [0u8; 2];
            match stream.read_exact(&mut len_buf).await {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    return Ok(());
                }
                Err(e) => return Err(error::Error::Io(e)),
            }

            let query_len = u16::from_be_bytes(len_buf) as usize;
            if query_len == 0 || query_len > buf.len() {
                continue;
            }

            stream
                .read_exact(&mut buf[..query_len])
                .await
                .map_err(|e| error::Error::Io(e))?;

            let qname = extract_qname(&buf[..query_len]);
            let response = if let Some(domain) = qname {
                resolve_dns(&domain, &buf[..query_len])
            } else {
                None
            };

            if let Some(resp_bytes) = response {
                let resp_len = resp_bytes.len() as u16;
                stream
                    .write_all(&resp_len.to_be_bytes())
                    .await
                    .map_err(|e| error::Error::Io(e))?;
                stream
                    .write_all(&resp_bytes)
                    .await
                    .map_err(|e| error::Error::Io(e))?;
            } else {
                let servfail = build_servfail_response(&buf[..query_len]);
                let sf_len = servfail.len() as u16;
                stream
                    .write_all(&sf_len.to_be_bytes())
                    .await
                    .map_err(|e| error::Error::Io(e))?;
                stream
                    .write_all(&servfail)
                    .await
                    .map_err(|e| error::Error::Io(e))?;
            }
        }
    })
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
        labels.push(
            std::str::from_utf8(label)
                .ok()?
                .to_lowercase(),
        );
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
    let question_end = qname_end + 4; // null terminator + QTYPE(2) + QCLASS(2)
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

/// Build a minimal SERVFAIL response (12 bytes).
fn build_servfail_response(query_header: &[u8]) -> Vec<u8> {
    let id = query_header.get(..2).unwrap_or(&[0, 0]);
    vec![
        id[0],
        id[1],
        0x81, // flags: QR, RD, RA
        0x82, // flags: RCODE=SERVFAIL(2)
        0x00, 0x00, // QDCOUNT = 0
        0x00, 0x00, // ANCOUNT = 0
        0x00, 0x00, // NSCOUNT = 0
        0x00, 0x00, // ARCOUNT = 0
    ]
}
