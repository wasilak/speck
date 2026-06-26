//! vminitd — PID 1 inside the speck micro-VM.
//!
//! Parses the kernel cmdline for `vsock_port=PORT` and optionally
//! `dns_vsock_port=PORT`, starts the vsock echo server, and if a DNS
//! port is configured, spawns the DNS forwarder on a background thread.
//!
//! Built as a static musl binary, placed at /init in a cpio initrd.

fn main() {
    let port = parse_cmdline_vsock_port("/proc/cmdline").unwrap_or(1234);
    let dns_port = parse_cmdline_dns_port("/proc/cmdline");

    // If DNS proxy vsock port is configured, spawn the DNS forwarder
    if let Some(dns_vsock_port) = dns_port {
        std::thread::spawn(move || {
            match speck_guest::dns_forwarder::serve(dns_vsock_port) {
                Ok(()) => eprintln!("vminitd: dns forwarder finished (guest shutdown)"),
                Err(e) => eprintln!("vminitd: dns forwarder error: {e}"),
            }
        });
    }

    match speck_guest::vsock_echo::serve(port) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("vminitd: echo server error: {e}");
        }
    }
}

/// Parse `vsock_port=PORT` from the kernel command line.
fn parse_cmdline_vsock_port(path: &str) -> Option<u32> {
    let content = std::fs::read_to_string(path).ok()?;
    for word in content.split_whitespace() {
        if let Some(port_str) = word.strip_prefix("vsock_port=") {
            return port_str.parse::<u32>().ok();
        }
    }
    None
}

/// Parse `dns_vsock_port=PORT` from the kernel command line.
fn parse_cmdline_dns_port(path: &str) -> Option<u32> {
    let content = std::fs::read_to_string(path).ok()?;
    for word in content.split_whitespace() {
        if let Some(port_str) = word.strip_prefix("dns_vsock_port=") {
            return port_str.parse::<u32>().ok();
        }
    }
    None
}
