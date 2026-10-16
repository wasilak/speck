//! Guest-side vsock -> TCP forwarder.
//!
//! The host writes a single ASCII target line `<ipv4>:<port>\n` to the vsock
//! stream. The guest root namespace then connects to that TCP target and proxies
//! bytes bidirectionally. This is used for host-side published port forwarding,
//! where the guest can reach Docker bridge container IPs directly but the host
//! cannot.
//!
//! A target line `udp <ipv4>:<port>\n` forwards one UDP flow instead: each
//! datagram travels over the vsock stream as a 2-byte big-endian length plus
//! payload, in both directions. The host closes the stream when the flow
//! goes idle.

#![cfg(target_os = "linux")]

use std::io;
use std::net::{SocketAddrV4, TcpStream, UdpSocket};
use std::os::fd::IntoRawFd;
use std::str::FromStr;

pub fn serve(vsock_port: u32) -> io::Result<()> {
    let listen_fd = unsafe { libc::socket(libc::AF_VSOCK, libc::SOCK_STREAM, 0) };
    if listen_fd < 0 {
        return Err(io::Error::last_os_error());
    }

    let addr = libc::sockaddr_vm {
        svm_family: libc::AF_VSOCK as u16,
        svm_reserved1: 0,
        svm_port: vsock_port,
        svm_cid: libc::VMADDR_CID_ANY,
        svm_zero: [0u8; 4],
    };

    let addr_ptr = &addr as *const libc::sockaddr_vm as *const libc::sockaddr;
    let addr_len = std::mem::size_of::<libc::sockaddr_vm>() as u32;

    if unsafe { libc::bind(listen_fd, addr_ptr, addr_len) } < 0 {
        unsafe { libc::close(listen_fd) };
        return Err(io::Error::last_os_error());
    }

    if unsafe { libc::listen(listen_fd, 16) } < 0 {
        unsafe { libc::close(listen_fd) };
        return Err(io::Error::last_os_error());
    }

    loop {
        let vsock_fd =
            unsafe { libc::accept(listen_fd, std::ptr::null_mut(), std::ptr::null_mut()) };
        if vsock_fd < 0 {
            eprintln!(
                "tcp_forwarder: accept failed: {}",
                io::Error::last_os_error()
            );
            continue;
        }

        std::thread::spawn(move || handle_client(vsock_fd));
    }
}

fn handle_client(vsock_fd: libc::c_int) {
    let target = match read_target_line(vsock_fd) {
        Ok(Target::Udp(target)) => {
            handle_udp_client(vsock_fd, target);
            return;
        }
        Ok(Target::Tcp(target)) => target,
        Err(err) => {
            eprintln!("tcp_forwarder: failed to read target: {err}");
            unsafe { libc::close(vsock_fd) };
            return;
        }
    };

    let tcp = match TcpStream::connect(target) {
        Ok(stream) => stream,
        Err(err) => {
            eprintln!("tcp_forwarder: connect({target}) failed: {err}");
            unsafe { libc::close(vsock_fd) };
            return;
        }
    };

    let tcp_fd = tcp.into_raw_fd();
    let vsock_a = unsafe { libc::dup(vsock_fd) };
    let tcp_a = unsafe { libc::dup(tcp_fd) };
    let vsock_b = unsafe { libc::dup(vsock_fd) };
    let tcp_b = unsafe { libc::dup(tcp_fd) };

    unsafe { libc::close(vsock_fd) };
    unsafe { libc::close(tcp_fd) };

    std::thread::spawn(move || {
        proxy_copy(vsock_a, tcp_a);
        unsafe { libc::shutdown(tcp_a, libc::SHUT_WR) };
        unsafe { libc::close(vsock_a) };
        unsafe { libc::close(tcp_a) };
    });

    std::thread::spawn(move || {
        proxy_copy(tcp_b, vsock_b);
        unsafe { libc::shutdown(vsock_b, libc::SHUT_WR) };
        unsafe { libc::close(tcp_b) };
        unsafe { libc::close(vsock_b) };
    });
}

enum Target {
    Tcp(SocketAddrV4),
    Udp(SocketAddrV4),
}

/// Relay one UDP flow between the vsock stream and `target`.
fn handle_udp_client(vsock_fd: libc::c_int, target: SocketAddrV4) {
    let socket = match UdpSocket::bind("0.0.0.0:0").and_then(|s| s.connect(target).map(|()| s)) {
        Ok(socket) => socket,
        Err(err) => {
            eprintln!("tcp_forwarder: udp socket for {target} failed: {err}");
            unsafe { libc::close(vsock_fd) };
            return;
        }
    };
    let _ = socket.set_read_timeout(Some(std::time::Duration::from_secs(1)));
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

    let replies = {
        let socket = match socket.try_clone() {
            Ok(socket) => socket,
            Err(_) => {
                unsafe { libc::close(vsock_fd) };
                return;
            }
        };
        let done = std::sync::Arc::clone(&done);
        std::thread::spawn(move || {
            let mut buf = [0u8; 65535];
            while !done.load(std::sync::atomic::Ordering::SeqCst) {
                let Ok(n) = socket.recv(&mut buf) else {
                    continue; // timeout: re-check `done`
                };
                let mut frame = Vec::with_capacity(n + 2);
                frame.extend_from_slice(&(n as u16).to_be_bytes());
                frame.extend_from_slice(&buf[..n]);
                if !write_all(vsock_fd, &frame) {
                    break;
                }
            }
        })
    };

    let mut buf = vec![0u8; 65535];
    loop {
        let mut len = [0u8; 2];
        if !read_exact(vsock_fd, &mut len) {
            break;
        }
        let len = u16::from_be_bytes(len) as usize;
        if !read_exact(vsock_fd, &mut buf[..len]) {
            break;
        }
        let _ = socket.send(&buf[..len]);
    }
    done.store(true, std::sync::atomic::Ordering::SeqCst);
    let _ = replies.join();
    unsafe { libc::close(vsock_fd) };
}

fn read_exact(fd: libc::c_int, buf: &mut [u8]) -> bool {
    let mut off = 0;
    while off < buf.len() {
        let n = unsafe {
            libc::read(
                fd,
                buf[off..].as_mut_ptr() as *mut libc::c_void,
                buf.len() - off,
            )
        };
        if n <= 0 {
            return false;
        }
        off += n as usize;
    }
    true
}

fn write_all(fd: libc::c_int, buf: &[u8]) -> bool {
    let mut off = 0;
    while off < buf.len() {
        let n = unsafe {
            libc::write(
                fd,
                buf[off..].as_ptr() as *const libc::c_void,
                buf.len() - off,
            )
        };
        if n <= 0 {
            return false;
        }
        off += n as usize;
    }
    true
}

fn read_target_line(fd: libc::c_int) -> io::Result<Target> {
    let mut buf = Vec::with_capacity(64);
    let mut byte = [0u8; 1];
    loop {
        let n = unsafe { libc::read(fd, byte.as_mut_ptr() as *mut libc::c_void, 1) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "EOF before target line",
            ));
        }
        if byte[0] == b'\n' {
            break;
        }
        buf.push(byte[0]);
        if buf.len() > 128 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "target line too long",
            ));
        }
    }
    let line = std::str::from_utf8(&buf)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "target line not utf-8"))?;
    let (udp, addr) = match line.strip_prefix("udp ") {
        Some(addr) => (true, addr),
        None => (false, line),
    };
    let addr = SocketAddrV4::from_str(addr).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid target: {line}"),
        )
    })?;
    Ok(if udp {
        Target::Udp(addr)
    } else {
        Target::Tcp(addr)
    })
}

fn proxy_copy(read_fd: libc::c_int, write_fd: libc::c_int) {
    let mut buf = [0u8; 16384];
    loop {
        let n = unsafe { libc::read(read_fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
        if n <= 0 {
            break;
        }
        let mut written = 0usize;
        while written < n as usize {
            let w = unsafe {
                libc::write(
                    write_fd,
                    buf.as_ptr().add(written) as *const libc::c_void,
                    n as usize - written,
                )
            };
            if w < 0 {
                return;
            }
            written += w as usize;
        }
    }
}
