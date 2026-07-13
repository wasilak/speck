//! Guest-side vsock -> TCP forwarder.
//!
//! The host writes a single ASCII target line `<ipv4>:<port>\n` to the vsock
//! stream. The guest root namespace then connects to that TCP target and proxies
//! bytes bidirectionally. This is used for host-side published port forwarding,
//! where the guest can reach Docker bridge container IPs directly but the host
//! cannot.

#![cfg(target_os = "linux")]

use std::io;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddrV4, TcpStream};
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
        let vsock_fd = unsafe { libc::accept(listen_fd, std::ptr::null_mut(), std::ptr::null_mut()) };
        if vsock_fd < 0 {
            eprintln!("tcp_forwarder: accept failed: {}", io::Error::last_os_error());
            continue;
        }

        std::thread::spawn(move || handle_client(vsock_fd));
    }
}

fn handle_client(vsock_fd: libc::c_int) {
    let target = match read_target_line(vsock_fd) {
        Ok(target) => target,
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

fn read_target_line(fd: libc::c_int) -> io::Result<SocketAddrV4> {
    let mut buf = Vec::with_capacity(64);
    let mut byte = [0u8; 1];
    loop {
        let n = unsafe { libc::read(fd, byte.as_mut_ptr() as *mut libc::c_void, 1) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "EOF before target line"));
        }
        if byte[0] == b'\n' {
            break;
        }
        buf.push(byte[0]);
        if buf.len() > 128 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "target line too long"));
        }
    }
    let line = std::str::from_utf8(&buf)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "target line not utf-8"))?;
    SocketAddrV4::from_str(line)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, format!("invalid target: {line}")))
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
