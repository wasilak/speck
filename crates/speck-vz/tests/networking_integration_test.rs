//! Integration test: boot a micro-VM with network device, exercise the netstack.
//!
//! This test:
//! 1. Creates GuestConfig with NetworkConfig (subnet, MTU, MAC)
//! 2. Starts the VM (which wires VZFileHandleNetworkDeviceConfiguration)
//! 3. Retrieves the host-side netstack fd
//! 4. Sends a synthetic ARP request through the fd
//! 5. Creates FdDevice + SmoltcpInterface from the host fd
//! 6. Polls the interface — verifies the netstack responds with a valid ARP reply
//! 7. Cleans up: stops the VM
//!
//! Run with: cargo test -p speck-vz --test networking_integration_test -- --ignored

use speck_vz::{Guest, GuestConfig};
use std::os::unix::io::{AsRawFd, RawFd};
use std::path::PathBuf;
use std::time::Duration;

fn kernel_path() -> PathBuf {
    std::env::var("SPEK_KERNEL")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            base.join("../../target/kernel/vmlinux")
        })
}

fn initrd_path() -> PathBuf {
    std::env::var("SPEK_INITRD")
        .map(PathBuf::from)
        .unwrap_or_else(|_| "/tmp/speck-initrd.cpio.gz".into())
}

/// Build a minimal valid ARP request targeting the netstack.
///
/// ARP request: who-has 172.16.0.1? tell 172.16.0.2 (guest MAC)
/// Wraps in Ethernet II frame.
fn build_arp_request() -> Vec<u8> {
    let mut frame = Vec::with_capacity(42);

    // Ethernet header
    frame.extend_from_slice(&[0x02, 0x00, 0x00, 0x00, 0x00, 0x01]); // dst MAC: netstack
    frame.extend_from_slice(&[0x02, 0x00, 0x00, 0x00, 0x00, 0x02]); // src MAC: guest
    frame.extend_from_slice(&[0x08, 0x06]); // EtherType: ARP

    // ARP header (28 bytes)
    frame.extend_from_slice(&[0x00, 0x01]); // HTYPE: Ethernet
    frame.extend_from_slice(&[0x08, 0x00]); // PTYPE: IPv4
    frame.extend_from_slice(&[0x06]); // HLEN: 6
    frame.extend_from_slice(&[0x04]); // PLEN: 4
    frame.extend_from_slice(&[0x00, 0x01]); // OPER: Request

    frame.extend_from_slice(&[0x02, 0x00, 0x00, 0x00, 0x00, 0x02]); // sender MAC: guest
    frame.extend_from_slice(&[172, 16, 0, 2]); // sender IP: guest
    frame.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00]); // target MAC: unknown
    frame.extend_from_slice(&[172, 16, 0, 1]); // target IP: gateway

    frame
}

/// Check if a frame is a valid ARP reply from 172.16.0.1 to 172.16.0.2.
fn is_arp_reply(frame: &[u8]) -> bool {
    if frame.len() < 42 {
        return false;
    }
    // EtherType = ARP (0x0806)
    if frame[12] != 0x08 || frame[13] != 0x06 {
        return false;
    }
    // OPER = Reply (2)
    if frame[20] != 0x00 || frame[21] != 0x02 {
        return false;
    }
    // sender IP = 172.16.0.1
    if frame[28] != 172 || frame[29] != 16 || frame[30] != 0 || frame[31] != 1 {
        return false;
    }
    // target IP = 172.16.0.2
    if frame[38] != 172 || frame[39] != 16 || frame[40] != 0 || frame[41] != 2 {
        return false;
    }
    true
}

#[test]
#[ignore = "requires VM entitlement + kernel + initrd artifacts"]
fn test_netstack_arp() {
    let config = GuestConfig::builder()
        .kernel_path(kernel_path())
        .initrd_path(initrd_path())
        .cmdline("console=hvc0".to_string())
        .cpu_count(1)
        .memory_size_bytes(512 * 1024 * 1024)
        .vsock_port(1234)
        .network(speck_net::config::NetworkConfig {
            subnet_prefix: 24,
            gateway: std::net::Ipv4Addr::new(172, 16, 0, 1),
            guest_ip: std::net::Ipv4Addr::new(172, 16, 0, 2),
            mtu: 1500,
            mac: [0x02, 0x00, 0x00, 0x00, 0x00, 0x02],
        })
        .stop_timeout(Duration::from_secs(10))
        .build();

    let guest = Guest::new(config);
    guest.start().expect("guest start");

    let host_fd: RawFd = guest.netstack_fd().expect("netstack_fd");

    std::thread::sleep(Duration::from_secs(1));

    let device = speck_net::device::FdDevice::new(host_fd, 1500);
    let mut net = speck_net::interface::SmoltcpInterface::new(
        device,
        &speck_net::config::NetworkConfig {
            subnet_prefix: 24,
            gateway: std::net::Ipv4Addr::new(172, 16, 0, 1),
            guest_ip: std::net::Ipv4Addr::new(172, 16, 0, 2),
            mtu: 1500,
            mac: [0x02, 0x00, 0x00, 0x00, 0x00, 0x01],
        },
    );

    let arp_request = build_arp_request();
    unsafe {
        libc::write(
            host_fd,
            arp_request.as_ptr() as *const libc::c_void,
            arp_request.len(),
        );
    }

    let timestamp = smoltcp::time::Instant::now();
    net.poll(timestamp);

    let mut resp_buf = vec![0u8; 1514];
    let n = unsafe {
        libc::read(
            host_fd,
            resp_buf.as_mut_ptr() as *mut libc::c_void,
            resp_buf.len(),
        )
    };

    assert!(n > 0, "Expected ARP reply, got empty read (n={})", n);
    resp_buf.truncate(n as usize);

    assert!(
        is_arp_reply(&resp_buf),
        "Expected ARP reply from 172.16.0.1, got {}-byte frame: {:02x?}",
        resp_buf.len(),
        &resp_buf[..resp_buf.len().min(60)]
    );

    guest.stop().expect("guest stop");
}

#[test]
#[ignore = "requires VM entitlement"]
fn test_netstack_no_fd_before_start() {
    let config = GuestConfig::builder().kernel_path(kernel_path()).build();
    let guest = Guest::new(config);
    let result = guest.netstack_fd();
    assert!(
        result.is_err(),
        "expected error when calling netstack_fd before start"
    );
}

/// Build a minimal DNS A query for the given domain.
fn build_dns_query(domain: &str, id: u16) -> Vec<u8> {
    let mut msg = Vec::with_capacity(512);

    msg.extend_from_slice(&id.to_be_bytes()); // ID
    msg.extend_from_slice(&[0x01, 0x00]); // flags: RD=1
    msg.extend_from_slice(&[0x00, 0x01]); // QDCOUNT = 1
    msg.extend_from_slice(&[0x00, 0x00]); // ANCOUNT = 0
    msg.extend_from_slice(&[0x00, 0x00]); // NSCOUNT = 0
    msg.extend_from_slice(&[0x00, 0x00]); // ARCOUNT = 0

    for label in domain.split('.') {
        msg.push(label.len() as u8);
        msg.extend_from_slice(label.as_bytes());
    }
    msg.push(0x00); // null terminator

    msg.extend_from_slice(&[0x00, 0x01]); // QTYPE: A
    msg.extend_from_slice(&[0x00, 0x01]); // QCLASS: IN

    msg
}

fn is_valid_dns_response(data: &[u8]) -> bool {
    if data.len() < 12 {
        return false;
    }
    // QR flag = 1 (response)
    if data[2] & 0x80 == 0 {
        return false;
    }
    // RCODE = 0 (no error)
    if data[3] & 0x0f != 0 {
        return false;
    }
    // Answer count > 0
    let ancount = u16::from_be_bytes([data[6], data[7]]);
    ancount > 0
}

#[test]
#[ignore = "requires VM entitlement + kernel + initrd artifacts"]
fn test_dns_proxy_vsock() {
    let config = GuestConfig::builder()
        .kernel_path(kernel_path())
        .initrd_path(initrd_path())
        .cmdline("console=hvc0".to_string())
        .cpu_count(1)
        .memory_size_bytes(512 * 1024 * 1024)
        .vsock_port(1234)
        .dns_vsock_port(1235)
        .network(speck_net::config::NetworkConfig {
            subnet_prefix: 24,
            gateway: std::net::Ipv4Addr::new(172, 16, 0, 1),
            guest_ip: std::net::Ipv4Addr::new(172, 16, 0, 2),
            mtu: 1500,
            mac: [0x02, 0x00, 0x00, 0x00, 0x00, 0x02],
        })
        .stop_timeout(Duration::from_secs(10))
        .build();

    let guest = Guest::new(config);
    guest.start().expect("guest start");

    let vsock = guest
        .vsock_connect(1235)
        .expect("vsock_connect to DNS proxy port");

    let query = build_dns_query("google.com", 0x1234);
    let query_len = query.len() as u16;

    let mut wire = Vec::with_capacity(2 + query.len());
    wire.extend_from_slice(&query_len.to_be_bytes());
    wire.extend_from_slice(&query);

    unsafe {
        libc::write(
            vsock.as_raw_fd(),
            wire.as_ptr() as *const libc::c_void,
            wire.len(),
        );
    }

    let mut len_buf = [0u8; 2];
    let n = unsafe {
        libc::read(
            vsock.as_raw_fd(),
            len_buf.as_mut_ptr() as *mut libc::c_void,
            2,
        )
    };
    assert_eq!(n, 2, "should read 2-byte length prefix");

    let resp_len = u16::from_be_bytes(len_buf) as usize;
    assert!(
        resp_len > 0 && resp_len <= 512,
        "DNS response length {} out of range",
        resp_len
    );

    let mut resp_buf = vec![0u8; resp_len];
    let n = unsafe {
        libc::read(
            vsock.as_raw_fd(),
            resp_buf.as_mut_ptr() as *mut libc::c_void,
            resp_len,
        )
    };
    assert_eq!(
        n as usize, resp_len,
        "should read {} DNS response bytes, got {}",
        resp_len, n
    );

    assert!(
        is_valid_dns_response(&resp_buf),
        "expected valid DNS response for google.com, got {} bytes: {:02x?}",
        resp_buf.len(),
        &resp_buf[..resp_buf.len().min(40)]
    );

    guest.stop().expect("guest stop");
}
