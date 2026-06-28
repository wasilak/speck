//! Guest-side vsock echo server using Linux AF_VSOCK sockets.
//!
//! # Safety
//!
//! This module calls libc functions (socket, bind, listen, accept, read, write, close)
//! directly with raw file descriptors. All operations are safe under normal operation;
//! invalid fds return IO errors via `io::Error::last_os_error()`.

#![cfg(target_os = "linux")]

use std::io;

const BUF_SIZE: usize = 4096;

/// Run a blocking echo server on the given vsock port.
///
/// Binds to `VMADDR_CID_ANY`, listens with backlog 1, accepts exactly one
/// connection, and echoes back all received data. Returns `Ok(())` when the
/// peer disconnects (read returns 0).
pub fn serve(port: u32) -> io::Result<()> {
    let fd = unsafe { libc::socket(libc::AF_VSOCK, libc::SOCK_STREAM, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }

    let addr = libc::sockaddr_vm {
        svm_family: libc::AF_VSOCK as u16,
        svm_reserved1: 0,
        svm_port: port,
        svm_cid: libc::VMADDR_CID_ANY,
        svm_zero: [0u8; 4],
    };

    let addr_ptr = &addr as *const libc::sockaddr_vm as *const libc::sockaddr;
    let addr_len = std::mem::size_of::<libc::sockaddr_vm>() as u32;

    let ret = unsafe { libc::bind(fd, addr_ptr, addr_len) };
    if ret < 0 {
        unsafe {
            libc::close(fd);
        }
        return Err(io::Error::last_os_error());
    }

    let ret = unsafe { libc::listen(fd, 1) };
    if ret < 0 {
        unsafe {
            libc::close(fd);
        }
        return Err(io::Error::last_os_error());
    }

    let client = unsafe { libc::accept(fd, std::ptr::null_mut(), std::ptr::null_mut()) };
    if client < 0 {
        unsafe {
            libc::close(fd);
        }
        return Err(io::Error::last_os_error());
    }

    // Close listen fd — we only handle one connection
    unsafe {
        libc::close(fd);
    }

    let mut buf = [0u8; BUF_SIZE];
    loop {
        let n = unsafe { libc::read(client, buf.as_mut_ptr() as *mut libc::c_void, BUF_SIZE) };
        if n <= 0 {
            break;
        }
        let mut written = 0usize;
        while written < n as usize {
            let w = unsafe {
                libc::write(
                    client,
                    buf.as_ptr().add(written) as *const libc::c_void,
                    (n as usize) - written,
                )
            };
            if w < 0 {
                unsafe {
                    libc::close(client);
                }
                return Err(io::Error::last_os_error());
            }
            written += w as usize;
        }
    }

    unsafe {
        libc::close(client);
    }
    Ok(())
}
