//! Guest-side DNS forwarder using Linux AF_VSOCK sockets.
//!
//! Binds guest UDP:53, forwards DNS queries length-prefixed over vsock
//! to the host DNS proxy, and sends responses back to the guest client.
//!
//! # Safety
//!
//! This module calls libc functions (socket, bind, listen, accept, poll,
//! recvfrom, sendto, read, write, close) directly with raw file descriptors.
//! All operations are safe under normal operation; invalid fds return IO
//! errors via `io::Error::last_os_error()`.

#![cfg(target_os = "linux")]

use std::io;

const DNS_BUF_SIZE: usize = 512;

/// Run a DNS forwarder on the given vsock port.
///
/// 1. Creates an AF_VSOCK listener on `dns_vsock_port`, accepts one connection.
/// 2. Creates a UDP socket bound to 0.0.0.0:53.
/// 3. Polls both fds: forwards DNS queries from UDP:53 → vsock host proxy,
///    and sends host responses back to the guest UDP client.
pub fn serve(dns_vsock_port: u32) -> io::Result<()> {
    // --- Step 1: Create vsock listener and accept one connection ---
    let listen_fd = unsafe { libc::socket(libc::AF_VSOCK, libc::SOCK_STREAM, 0) };
    if listen_fd < 0 {
        return Err(io::Error::last_os_error());
    }

    let addr = libc::sockaddr_vm {
        svm_family: libc::AF_VSOCK as u16,
        svm_reserved1: 0,
        svm_port: dns_vsock_port,
        svm_cid: libc::VMADDR_CID_ANY,
        svm_zero: [0u8; 4],
    };

    let addr_ptr = &addr as *const libc::sockaddr_vm as *const libc::sockaddr;
    let addr_len = std::mem::size_of::<libc::sockaddr_vm>() as u32;

    let ret = unsafe { libc::bind(listen_fd, addr_ptr, addr_len) };
    if ret < 0 {
        unsafe {
            libc::close(listen_fd);
        }
        return Err(io::Error::last_os_error());
    }

    let ret = unsafe { libc::listen(listen_fd, 1) };
    if ret < 0 {
        unsafe {
            libc::close(listen_fd);
        }
        return Err(io::Error::last_os_error());
    }

    let vsock_fd = unsafe { libc::accept(listen_fd, std::ptr::null_mut(), std::ptr::null_mut()) };
    if vsock_fd < 0 {
        unsafe {
            libc::close(listen_fd);
        }
        return Err(io::Error::last_os_error());
    }

    // Close listen fd — we only handle one vsock connection
    unsafe {
        libc::close(listen_fd);
    }

    // --- Step 2: Create UDP socket bound to 0.0.0.0:53 ---
    let udp_fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) };
    if udp_fd < 0 {
        unsafe {
            libc::close(vsock_fd);
        }
        return Err(io::Error::last_os_error());
    }

    let udp_addr = libc::sockaddr_in {
        sin_family: libc::AF_INET as u16,
        sin_port: 53u16.to_be(),
        sin_addr: libc::in_addr { s_addr: 0u32 },
        sin_zero: [0u8; 8],
    };

    let udp_addr_ptr = &udp_addr as *const libc::sockaddr_in as *const libc::sockaddr;
    let ret = unsafe {
        libc::bind(
            udp_fd,
            udp_addr_ptr,
            std::mem::size_of::<libc::sockaddr_in>() as u32,
        )
    };
    if ret < 0 {
        unsafe {
            libc::close(vsock_fd);
        }
        unsafe {
            libc::close(udp_fd);
        }
        return Err(io::Error::last_os_error());
    }

    // Report each query's destination address (IP_PKTINFO). Containers on
    // user-defined networks (compose) reach this socket via their own bridge
    // gateway path; without replying from the queried address the kernel
    // picks that bridge's address as the source and the client drops the
    // reply.
    let one: libc::c_int = 1;
    unsafe {
        libc::setsockopt(
            udp_fd,
            libc::IPPROTO_IP,
            libc::IP_PKTINFO,
            &one as *const libc::c_int as *const libc::c_void,
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        );
    }

    // TCP:53 for clients that retry over TCP after a truncated UDP answer
    // (RFC 7766). Optional: the forwarder keeps serving UDP if it fails.
    let tcp_fd = match tcp_listener(53) {
        Ok(fd) => fd,
        Err(e) => {
            eprintln!("vminitd: dns forwarder TCP:53 unavailable: {e}");
            -1
        }
    };

    // --- Step 3: Poll loop ---
    let mut buf = [0u8; DNS_BUF_SIZE];
    let mut resp = vec![0u8; u16::MAX as usize];

    loop {
        let mut fds = [
            libc::pollfd {
                fd: udp_fd,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: vsock_fd,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: tcp_fd, // negative fd is ignored by poll
                events: libc::POLLIN,
                revents: 0,
            },
        ];

        let ret = unsafe { libc::poll(fds.as_mut_ptr(), 3, -1) };
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }

        // UDP:53 has data — DNS query from guest app
        if fds[0].revents & libc::POLLIN != 0 {
            // Capture the client address and the address the query was sent
            // to, so the reply can come from that same address.
            let Some((n, client_addr, local_addr)) = recv_query(udp_fd, &mut buf) else {
                continue;
            };

            let resp_len = forward(vsock_fd, &buf[..n], &mut resp)?;
            if resp_len == 0 {
                continue;
            }

            send_reply(udp_fd, &resp[..resp_len], &client_addr, local_addr);
        }

        // TCP:53 client — serve its queries inline (clients send one query
        // and close; a 2 s timeout bounds how long UDP waits behind it).
        if fds[2].revents & libc::POLLIN != 0 {
            let client =
                unsafe { libc::accept(tcp_fd, std::ptr::null_mut(), std::ptr::null_mut()) };
            if client >= 0 {
                let result = serve_tcp_client(client, vsock_fd, &mut resp);
                unsafe { libc::close(client) };
                result?;
            }
        }

        // Vsock data arriving without a prior query (rare/protocol error) — drain
        if fds[1].revents & libc::POLLIN != 0 {
            let mut drain = [0u8; 4];
            let _ = unsafe { libc::read(vsock_fd, drain.as_mut_ptr() as *mut libc::c_void, 4) };
        }
    }
}

/// Receive one query. Returns its length, the client address and the local
/// address the query was sent to (from IP_PKTINFO), if the kernel reported it.
fn recv_query(
    udp_fd: libc::c_int,
    buf: &mut [u8],
) -> Option<(usize, libc::sockaddr_in, Option<libc::in_addr>)> {
    let mut client: libc::sockaddr_in = unsafe { std::mem::zeroed() };
    let mut iov = libc::iovec {
        iov_base: buf.as_mut_ptr() as *mut libc::c_void,
        iov_len: buf.len(),
    };
    let mut control = [0u8; 64];
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_name = &mut client as *mut libc::sockaddr_in as *mut libc::c_void;
    msg.msg_namelen = std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr() as *mut libc::c_void;
    msg.msg_controllen = control.len() as _;

    let n = unsafe { libc::recvmsg(udp_fd, &mut msg, 0) };
    if n <= 0 {
        return None;
    }

    let mut local = None;
    let mut cmsg = unsafe { libc::CMSG_FIRSTHDR(&msg) };
    while !cmsg.is_null() {
        let hdr = unsafe { &*cmsg };
        if hdr.cmsg_level == libc::IPPROTO_IP && hdr.cmsg_type == libc::IP_PKTINFO {
            let info: libc::in_pktinfo = unsafe {
                std::ptr::read_unaligned(libc::CMSG_DATA(cmsg) as *const libc::in_pktinfo)
            };
            local = Some(info.ipi_addr);
        }
        cmsg = unsafe { libc::CMSG_NXTHDR(&msg, cmsg) };
    }
    Some((n as usize, client, local))
}

/// Send a reply to `client`, from `local` when known.
fn send_reply(
    udp_fd: libc::c_int,
    data: &[u8],
    client: &libc::sockaddr_in,
    local: Option<libc::in_addr>,
) {
    let mut iov = libc::iovec {
        iov_base: data.as_ptr() as *mut libc::c_void,
        iov_len: data.len(),
    };
    let mut control = [0u8; 64];
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_name = client as *const libc::sockaddr_in as *mut libc::c_void;
    msg.msg_namelen = std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;

    if let Some(addr) = local {
        let space = unsafe { libc::CMSG_SPACE(std::mem::size_of::<libc::in_pktinfo>() as u32) };
        msg.msg_control = control.as_mut_ptr() as *mut libc::c_void;
        msg.msg_controllen = space as _;
        let cmsg = unsafe { libc::CMSG_FIRSTHDR(&msg) };
        unsafe {
            (*cmsg).cmsg_level = libc::IPPROTO_IP;
            (*cmsg).cmsg_type = libc::IP_PKTINFO;
            (*cmsg).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<libc::in_pktinfo>() as u32) as _;
            let info = libc::in_pktinfo {
                ipi_ifindex: 0,
                ipi_spec_dst: addr,
                ipi_addr: addr,
            };
            std::ptr::write_unaligned(libc::CMSG_DATA(cmsg) as *mut libc::in_pktinfo, info);
        }
    }

    unsafe {
        libc::sendmsg(udp_fd, &msg, 0);
    }
}

fn write_all(fd: libc::c_int, data: &[u8]) -> io::Result<()> {
    let mut written = 0usize;
    while written < data.len() {
        let w = unsafe {
            libc::write(
                fd,
                data.as_ptr().add(written) as *const libc::c_void,
                data.len() - written,
            )
        };
        if w < 0 {
            return Err(io::Error::last_os_error());
        }
        written += w as usize;
    }
    Ok(())
}

fn read_exact(fd: libc::c_int, buf: &mut [u8]) -> io::Result<()> {
    let mut r = 0usize;
    while r < buf.len() {
        let n = unsafe {
            libc::read(
                fd,
                buf.as_mut_ptr().add(r) as *mut libc::c_void,
                buf.len() - r,
            )
        };
        if n <= 0 {
            return Err(io::Error::last_os_error());
        }
        r += n as usize;
    }
    Ok(())
}

/// Send one length-prefixed query to the host over vsock and read the whole
/// response into `resp`. Always reads the full response, whatever its size:
/// a partial read would desynchronise the stream for every later query.
fn forward(vsock_fd: libc::c_int, query: &[u8], resp: &mut [u8]) -> io::Result<usize> {
    write_all(vsock_fd, &(query.len() as u16).to_be_bytes())?;
    write_all(vsock_fd, query)?;
    let mut len_buf = [0u8; 2];
    read_exact(vsock_fd, &mut len_buf)?;
    let resp_len = u16::from_be_bytes(len_buf) as usize;
    read_exact(vsock_fd, &mut resp[..resp_len])?;
    Ok(resp_len)
}

fn tcp_listener(port: u16) -> io::Result<libc::c_int> {
    let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let one: libc::c_int = 1;
    unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_REUSEADDR,
            &one as *const libc::c_int as *const libc::c_void,
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        );
    }
    let addr = libc::sockaddr_in {
        sin_family: libc::AF_INET as u16,
        sin_port: port.to_be(),
        sin_addr: libc::in_addr { s_addr: 0 },
        sin_zero: [0u8; 8],
    };
    let ok = unsafe {
        libc::bind(
            fd,
            &addr as *const libc::sockaddr_in as *const libc::sockaddr,
            std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
        ) == 0
            && libc::listen(fd, 16) == 0
    };
    if !ok {
        let err = io::Error::last_os_error();
        unsafe { libc::close(fd) };
        return Err(err);
    }
    Ok(fd)
}

/// Serve DNS-over-TCP queries from one client until it closes or idles.
/// Client-side failures end the connection; only vsock failures are errors.
fn serve_tcp_client(client: libc::c_int, vsock_fd: libc::c_int, resp: &mut [u8]) -> io::Result<()> {
    let timeout = libc::timeval {
        tv_sec: 2,
        tv_usec: 0,
    };
    for opt in [libc::SO_RCVTIMEO, libc::SO_SNDTIMEO] {
        unsafe {
            libc::setsockopt(
                client,
                libc::SOL_SOCKET,
                opt,
                &timeout as *const libc::timeval as *const libc::c_void,
                std::mem::size_of::<libc::timeval>() as libc::socklen_t,
            );
        }
    }
    let mut query = vec![0u8; u16::MAX as usize];
    loop {
        let mut len_buf = [0u8; 2];
        if read_exact(client, &mut len_buf).is_err() {
            return Ok(());
        }
        let len = u16::from_be_bytes(len_buf) as usize;
        if read_exact(client, &mut query[..len]).is_err() {
            return Ok(());
        }
        // A TCP client accepts answers of any size; tell the host so.
        let q = crate::dns_wire::with_edns(&query[..len], crate::dns_wire::TCP_EDNS_PAYLOAD);
        let resp_len = forward(vsock_fd, &q, resp)?;
        if write_all(client, &(resp_len as u16).to_be_bytes()).is_err()
            || write_all(client, &resp[..resp_len]).is_err()
        {
            return Ok(());
        }
    }
}
