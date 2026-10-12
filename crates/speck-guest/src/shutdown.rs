//! Graceful guest shutdown requested by the host over vsock.
//!
//! The host powers the VM off without warning the guest. Before doing so it
//! connects here and sends one byte; vminitd stops dockerd (which stops the
//! containers and records their state, so restart policies apply on the
//! next start), flushes the disks and answers with one byte.

/// vsock port of the shutdown listener.
pub const SHUTDOWN_VSOCK_PORT: u32 = 9008;

/// Request byte the host sends, and the acknowledgement the guest returns.
pub const REQUEST: u8 = b'S';
pub const ACK: u8 = b'K';

/// Accept host requests forever; run `shut_down` for each and acknowledge.
#[cfg(target_os = "linux")]
pub fn serve(port: u32, shut_down: impl Fn()) -> std::io::Result<()> {
    use std::io;

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
    if unsafe { libc::bind(fd, addr_ptr, addr_len) } < 0 || unsafe { libc::listen(fd, 1) } < 0 {
        let err = io::Error::last_os_error();
        unsafe { libc::close(fd) };
        return Err(err);
    }

    loop {
        let client = unsafe { libc::accept(fd, std::ptr::null_mut(), std::ptr::null_mut()) };
        if client < 0 {
            continue;
        }
        let mut request = 0u8;
        let n = unsafe { libc::read(client, &mut request as *mut u8 as *mut libc::c_void, 1) };
        if n == 1 && request == REQUEST {
            shut_down();
            unsafe { libc::write(client, &ACK as *const u8 as *const libc::c_void, 1) };
        }
        unsafe { libc::close(client) };
    }
}
