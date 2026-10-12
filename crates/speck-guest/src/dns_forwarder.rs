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

use crate::dns_mux::Pending;

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
    // Queries go to the host as soon as they arrive; the host answers them
    // concurrently and `pending` routes each answer back to its client.
    let mut buf = [0u8; DNS_BUF_SIZE];
    let mut resp = vec![0u8; u16::MAX as usize];
    let mut pending: Pending<Client> = Pending::default();
    let mut tcp_clients: Vec<TcpClient> = Vec::new();
    let mut next_client_id = 0u64;

    loop {
        let mut fds: Vec<libc::pollfd> = [udp_fd, vsock_fd, tcp_fd] // negative fd is ignored by poll
            .into_iter()
            .chain(tcp_clients.iter().map(|c| c.fd))
            .map(|fd| libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            })
            .collect();

        let ret = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, 1000) };
        if ret < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        let now = std::time::Instant::now();

        // Host answer: route it to the client that asked.
        if fds[1].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
            let len = read_frame(vsock_fd, &mut resp)?;
            match pending.complete(&mut resp[..len]) {
                Some(Client::Udp { addr, local }) => send_reply(udp_fd, &resp[..len], &addr, local),
                Some(Client::Tcp(id)) => {
                    if let Some(client) = tcp_clients.iter_mut().find(|c| c.id == id) {
                        client.last_active = now;
                        if write_frame(client.fd, &resp[..len]).is_err() {
                            client.closed = true;
                        }
                    }
                }
                None => {}
            }
        }

        // UDP:53 query from a guest app.
        if fds[0].revents & libc::POLLIN != 0
            && let Some((n, addr, local)) = recv_query(udp_fd, &mut buf)
        {
            let mut query = buf[..n].to_vec();
            if pending.register(&mut query, Client::Udp { addr, local }, now) {
                write_frame(vsock_fd, &query)?;
            }
        }

        // New TCP:53 client (retry after a truncated UDP answer).
        if fds[2].revents & libc::POLLIN != 0 {
            let fd = unsafe { libc::accept(tcp_fd, std::ptr::null_mut(), std::ptr::null_mut()) };
            if fd >= 0 {
                if tcp_clients.len() < MAX_TCP_CLIENTS {
                    set_timeouts(fd);
                    tcp_clients.push(TcpClient {
                        id: next_client_id,
                        fd,
                        last_active: now,
                        closed: false,
                    });
                    next_client_id += 1;
                } else {
                    unsafe { libc::close(fd) };
                }
            }
        }

        // Queries from TCP clients.
        for (client, pfd) in tcp_clients.iter_mut().zip(&fds[3..]) {
            if pfd.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) == 0 {
                continue;
            }
            let Ok(len) = read_frame(client.fd, &mut resp) else {
                client.closed = true;
                continue;
            };
            client.last_active = now;
            // A TCP client accepts answers of any size; tell the host so.
            let mut query =
                crate::dns_wire::with_edns(&resp[..len], crate::dns_wire::TCP_EDNS_PAYLOAD);
            if pending.register(&mut query, Client::Tcp(client.id), now) {
                write_frame(vsock_fd, &query)?;
            }
        }

        // Drop closed and idle TCP clients, and the answers they wait for.
        let before = tcp_clients.len();
        tcp_clients.retain(|c| {
            let keep = !c.closed && now.duration_since(c.last_active) < TCP_IDLE;
            if !keep {
                unsafe { libc::close(c.fd) };
            }
            keep
        });
        if tcp_clients.len() != before {
            pending.forget(|client| match client {
                Client::Tcp(id) => !tcp_clients.iter().any(|c| c.id == *id),
                Client::Udp { .. } => false,
            });
        }
    }
}

/// Where a host answer goes.
enum Client {
    Udp {
        addr: libc::sockaddr_in,
        local: Option<libc::in_addr>,
    },
    Tcp(u64),
}

struct TcpClient {
    id: u64,
    fd: libc::c_int,
    last_active: std::time::Instant,
    closed: bool,
}

const MAX_TCP_CLIENTS: usize = 64;
const TCP_IDLE: std::time::Duration = std::time::Duration::from_secs(10);

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

/// Write one length-prefixed DNS message.
fn write_frame(fd: libc::c_int, msg: &[u8]) -> io::Result<()> {
    let mut frame = Vec::with_capacity(msg.len() + 2);
    frame.extend_from_slice(&(msg.len() as u16).to_be_bytes());
    frame.extend_from_slice(msg);
    write_all(fd, &frame)
}

/// Read one whole length-prefixed DNS message into `buf`. Always reads the
/// full message: a partial read would desynchronise the stream.
fn read_frame(fd: libc::c_int, buf: &mut [u8]) -> io::Result<usize> {
    let mut len_buf = [0u8; 2];
    read_exact(fd, &mut len_buf)?;
    let len = u16::from_be_bytes(len_buf) as usize;
    read_exact(fd, &mut buf[..len])?;
    Ok(len)
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

/// Bound blocking reads and writes on a TCP client socket to 2 s.
fn set_timeouts(fd: libc::c_int) {
    let timeout = libc::timeval {
        tv_sec: 2,
        tv_usec: 0,
    };
    for opt in [libc::SO_RCVTIMEO, libc::SO_SNDTIMEO] {
        unsafe {
            libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                opt,
                &timeout as *const libc::timeval as *const libc::c_void,
                std::mem::size_of::<libc::timeval>() as libc::socklen_t,
            );
        }
    }
}
