//! Guest-side vsock→Unix socket bidirectional byte forwarder.
//!
//! Listens on a vsock port and proxies raw bytes bidirectionally between
//! the vsock connection and a Unix domain socket path. Used by vminitd
//! to expose containerd and buildkitd gRPC sockets to the host over vsock.
//!
//! # Safety
//!
//! This module calls libc functions (socket, bind, listen, accept, dup,
//! connect, read, write, close) directly with raw file descriptors.
//! All operations are safe under normal operation; invalid fds return IO
//! errors via `io::Error::last_os_error()`.

#![cfg(target_os = "linux")]

use std::io;

/// Listen on `vsock_port` and proxy each accepted connection to the Unix
/// domain socket at `unix_path`. Runs indefinitely, handling reconnects.
///
/// For each accepted vsock connection:
/// 1. Connects to `unix_path` (AF_UNIX SOCK_STREAM).
/// 2. Spawns two threads — one copying vsock→unix, the other unix→vsock.
/// 3. Re-accepts the next vsock connection.
///
/// If `unix_connect` fails (e.g., the socket doesn't exist yet), the vsock
/// connection is closed and the loop continues.
pub fn serve(vsock_port: u32, unix_path: &'static str) -> io::Result<()> {
    // --- Create vsock listener ---
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

    // --- Accept loop ---
    loop {
        let vsock_fd =
            unsafe { libc::accept(listen_fd, std::ptr::null_mut(), std::ptr::null_mut()) };
        if vsock_fd < 0 {
            let err = io::Error::last_os_error();
            eprintln!("sock_forwarder: accept failed: {err}");
            continue;
        }

        let unix_fd = match unix_connect(unix_path) {
            Ok(fd) => fd,
            Err(e) => {
                eprintln!("sock_forwarder: unix_connect({unix_path}) failed: {e}");
                unsafe {
                    libc::close(vsock_fd);
                }
                continue;
            }
        };

        // Dup both fds so each thread owns an independent pair.
        let vsock_a = unsafe { libc::dup(vsock_fd) };
        let unix_a = unsafe { libc::dup(unix_fd) };
        let vsock_b = unsafe { libc::dup(vsock_fd) };
        let unix_b = unsafe { libc::dup(unix_fd) };

        // Close originals — threads have their copies.
        unsafe {
            libc::close(vsock_fd);
        }
        unsafe {
            libc::close(unix_fd);
        }

        // Thread A: vsock → unix
        std::thread::spawn(move || {
            proxy_copy(vsock_a, unix_a);
            // Half-close: signal dockerd that no more data is coming from the host.
            unsafe { libc::shutdown(unix_a, libc::SHUT_WR) };
            unsafe { libc::close(vsock_a) };
            unsafe { libc::close(unix_a) };
        });

        // Thread B: unix → vsock
        std::thread::spawn(move || {
            proxy_copy(unix_b, vsock_b);
            // Half-close: send FIN to host so host v→u thread unblocks and sees EOF.
            // shutdown() on a dup'd fd still affects the underlying socket on Linux.
            unsafe { libc::shutdown(vsock_b, libc::SHUT_WR) };
            unsafe { libc::close(unix_b) };
            unsafe { libc::close(vsock_b) };
        });
    }
}

/// Copy bytes from `read_fd` to `write_fd` until EOF or error.
fn proxy_copy(read_fd: libc::c_int, write_fd: libc::c_int) {
    let mut buf = [0u8; 16384];
    loop {
        let n = unsafe { libc::read(read_fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
        if n <= 0 {
            break; // EOF or error
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
                return; // write error
            }
            written += w as usize;
        }
    }
}

/// Connect to a Unix domain socket at `path`.
///
/// Returns a SOCK_STREAM fd on success, or an IO error on failure.
fn unix_connect(path: &str) -> io::Result<libc::c_int> {
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }

    let path_bytes = path.as_bytes();
    if path_bytes.len() >= 108 {
        unsafe {
            libc::close(fd);
        }
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unix socket path too long",
        ));
    }

    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    addr.sun_family = libc::AF_UNIX as u16;

    // Copy path bytes into sun_path.
    for (i, &b) in path_bytes.iter().enumerate() {
        addr.sun_path[i] = b as _;
    }

    let addr_ptr = &addr as *const libc::sockaddr_un as *const libc::sockaddr;
    let addr_len = std::mem::size_of::<libc::sockaddr_un>() as u32;

    let ret = unsafe { libc::connect(fd, addr_ptr, addr_len) };
    if ret < 0 {
        unsafe {
            libc::close(fd);
        }
        return Err(io::Error::last_os_error());
    }

    Ok(fd)
}
