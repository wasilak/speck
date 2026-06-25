//! vminitd — PID 1 inside the speck micro-VM.
//!
//! Parses the kernel cmdline for `vsock_port=PORT`,
//! starts the vsock echo server, echoes one connection, then exits.
//!
//! Built as a static musl binary, placed at /init in a cpio initrd.

fn main() {
    let port = parse_cmdline_vsock_port("/proc/cmdline").unwrap_or(1234);
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
