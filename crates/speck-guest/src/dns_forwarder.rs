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

    // --- Step 3: Poll loop ---
    let mut buf = [0u8; DNS_BUF_SIZE];

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
        ];

        let ret = unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) };
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }

        // UDP:53 has data — DNS query from guest app
        if fds[0].revents & libc::POLLIN != 0 {
            let n = unsafe {
                libc::recvfrom(
                    udp_fd,
                    buf.as_mut_ptr() as *mut libc::c_void,
                    DNS_BUF_SIZE,
                    0,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            if n <= 0 {
                continue;
            }

            // Send length-prefixed DNS query over vsock (u16 BE + bytes)
            let query_len = (n as u16).to_be_bytes();
            let mut written = 0usize;
            while written < 2 {
                let w = unsafe {
                    libc::write(
                        vsock_fd,
                        query_len.as_ptr().add(written) as *const libc::c_void,
                        2 - written,
                    )
                };
                if w < 0 {
                    return Err(io::Error::last_os_error());
                }
                written += w as usize;
            }

            written = 0;
            while written < n as usize {
                let w = unsafe {
                    libc::write(
                        vsock_fd,
                        buf.as_ptr().add(written) as *const libc::c_void,
                        n as usize - written,
                    )
                };
                if w < 0 {
                    return Err(io::Error::last_os_error());
                }
                written += w as usize;
            }

            // Read response from vsock: u16 BE length prefix + DNS response bytes
            let mut len_buf = [0u8; 2];
            let mut r = 0usize;
            while r < 2 {
                let n = unsafe {
                    libc::read(
                        vsock_fd,
                        len_buf.as_mut_ptr().add(r) as *mut libc::c_void,
                        2 - r,
                    )
                };
                if n <= 0 {
                    return Err(io::Error::last_os_error());
                }
                r += n as usize;
            }

            let resp_len = u16::from_be_bytes(len_buf) as usize;
            if resp_len == 0 || resp_len > DNS_BUF_SIZE {
                continue;
            }

            r = 0;
            while r < resp_len {
                let n = unsafe {
                    libc::read(
                        vsock_fd,
                        buf.as_mut_ptr().add(r) as *mut libc::c_void,
                        resp_len - r,
                    )
                };
                if n <= 0 {
                    return Err(io::Error::last_os_error());
                }
                r += n as usize;
            }

            // Send DNS response to UDP client
            unsafe {
                libc::sendto(
                    udp_fd,
                    buf.as_ptr() as *const libc::c_void,
                    resp_len,
                    0,
                    std::ptr::null_mut(),
                    0,
                );
            }
        }

        // Vsock data arriving without a prior query (rare/protocol error) — drain
        if fds[1].revents & libc::POLLIN != 0 {
            let mut drain = [0u8; 4];
            let _ = unsafe { libc::read(vsock_fd, drain.as_mut_ptr() as *mut libc::c_void, 4) };
        }
    }
}
