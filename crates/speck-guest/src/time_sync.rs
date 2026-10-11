//! Guest clock sync from the host over vsock (RES-03).
//!
//! While the Mac sleeps the VM is paused and the guest clock falls behind,
//! which breaks TLS validity checks and AWS request signing after wake. The
//! host pushes its wall-clock time every few seconds; the guest steps its
//! clock when it drifted further than `MAX_DRIFT`.

/// vsock port of the time sync listener (host connects, writes 12 bytes:
/// seconds since the Unix epoch as i64 BE, then nanoseconds as u32 BE).
pub const TIME_SYNC_VSOCK_PORT: u32 = 9007;

/// Drift tolerated before the guest clock is stepped.
pub const MAX_DRIFT_NANOS: i128 = 1_000_000_000;

/// Decode the 12-byte host timestamp into (seconds, nanoseconds).
pub fn decode(msg: &[u8; 12]) -> (i64, u32) {
    let secs = i64::from_be_bytes(msg[..8].try_into().expect("8 bytes"));
    let nanos = u32::from_be_bytes(msg[8..].try_into().expect("4 bytes"));
    (secs, nanos)
}

/// Whether the guest clock is far enough from the host clock to step it.
pub fn needs_step(guest: (i64, u32), host: (i64, u32)) -> bool {
    let to_nanos = |(s, n): (i64, u32)| i128::from(s) * 1_000_000_000 + i128::from(n);
    (to_nanos(host) - to_nanos(guest)).abs() > MAX_DRIFT_NANOS
}

/// Accept host connections forever and step the clock when needed.
#[cfg(target_os = "linux")]
pub fn serve(port: u32) -> std::io::Result<()> {
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
    if unsafe { libc::bind(fd, addr_ptr, addr_len) } < 0 || unsafe { libc::listen(fd, 4) } < 0 {
        let err = io::Error::last_os_error();
        unsafe { libc::close(fd) };
        return Err(err);
    }

    loop {
        let client = unsafe { libc::accept(fd, std::ptr::null_mut(), std::ptr::null_mut()) };
        if client < 0 {
            continue;
        }
        let mut msg = [0u8; 12];
        let mut got = 0usize;
        while got < msg.len() {
            let n = unsafe {
                libc::read(
                    client,
                    msg.as_mut_ptr().add(got) as *mut libc::c_void,
                    msg.len() - got,
                )
            };
            if n <= 0 {
                break;
            }
            got += n as usize;
        }
        unsafe { libc::close(client) };
        if got == msg.len() {
            step_if_drifted(decode(&msg));
        }
    }
}

#[cfg(target_os = "linux")]
fn step_if_drifted(host: (i64, u32)) {
    let mut now: libc::timespec = unsafe { std::mem::zeroed() };
    if unsafe { libc::clock_gettime(libc::CLOCK_REALTIME, &mut now) } != 0 {
        return;
    }
    let guest = (now.tv_sec, now.tv_nsec as u32);
    if !needs_step(guest, host) {
        return;
    }
    let ts = libc::timespec {
        tv_sec: host.0,
        tv_nsec: i64::from(host.1),
    };
    if unsafe { libc::clock_settime(libc::CLOCK_REALTIME, &ts) } == 0 {
        eprintln!(
            "vminitd: guest clock stepped by {}s to match host",
            host.0 - guest.0
        );
    } else {
        eprintln!(
            "vminitd: clock_settime failed: {}",
            std::io::Error::last_os_error()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_round_trips_the_wire_format() {
        let mut msg = [0u8; 12];
        msg[..8].copy_from_slice(&1_791_712_000i64.to_be_bytes());
        msg[8..].copy_from_slice(&123_456_789u32.to_be_bytes());
        assert_eq!(decode(&msg), (1_791_712_000, 123_456_789));
    }

    #[test]
    fn only_drift_over_one_second_is_stepped() {
        let host = (1000, 500_000_000);
        assert!(!needs_step((1000, 0), host));
        assert!(!needs_step((999, 600_000_000), host));
        assert!(needs_step((998, 0), host)); // guest behind after sleep
        assert!(needs_step((1002, 0), host)); // guest ahead
        assert!(needs_step((1000 - 3600, 0), host));
    }
}
