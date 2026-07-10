use std::path::Path;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::time::timeout;

use anstream::println;

use crate::theme;

pub async fn run_status(speck_home: &Path) -> anyhow::Result<()> {
    let sock_path = speck_home.join("run/control.sock");
    let status = match timeout(Duration::from_secs(2), UnixStream::connect(&sock_path)).await {
        Ok(Ok(mut stream)) => {
            // The daemon's control server reads first (PREPARE_RESTART protocol),
            // so write PING — any unknown command elicits PONG — then drain the
            // reply under a 2s timeout. Liveness is defined by connect success.
            let _ = stream.write_all(b"PING\n").await;
            let mut buf = [0u8; 8];
            let _ = timeout(Duration::from_secs(2), stream.read(&mut buf)).await;
            "running"
        }
        _ => "stopped",
    };

    println!("Speck daemon: {}", theme::format_status(status));

    let pid_path = speck_home.join("run/speck.pid");
    if let Ok(pid_str) = std::fs::read_to_string(&pid_path)
        && let Ok(pid) = pid_str.trim().parse::<u32>()
    {
        println!("  PID: {pid}");
    }

    let resources = crate::commands::up::read_vm_resource_snapshot(speck_home)?;
    if let Some(cfg) = resources {
        println!(
            "  CPUs: {}  Memory: {} MiB  Disk: {} GiB",
            cfg.cpus, cfg.memory_mb, cfg.disk_gb
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    const STATUS_SOURCE: &str = include_str!("status.rs");

    /// Slice production code only — everything before `#[cfg(test)]` — so tests do not
    /// trivially pass because assertion strings themselves contain the searched tokens.
    fn production_code() -> &'static str {
        let end = STATUS_SOURCE
            .find("#[cfg(test)]")
            .unwrap_or(STATUS_SOURCE.len());
        &STATUS_SOURCE[..end]
    }

    #[test]
    fn status_reads_pid_from_run_directory() {
        assert!(
            production_code().contains("run/speck.pid"),
            "status must read PID from speck_home/run/speck.pid (consistent with up.rs write location)"
        );
    }

    #[test]
    fn status_probe_writes_ping_before_read() {
        let src = production_code();
        let ping_pos = src
            .find("write_all(b\"PING")
            .expect("run_status probe must write PING before reading (daemon reads first)");
        let read_pos = src
            .find("stream.read")
            .expect("run_status probe must read the PONG reply");
        assert!(
            ping_pos < read_pos,
            "PING write must precede the read — the daemon's control server reads first"
        );
        assert!(
            src.contains("timeout(Duration::from_secs(2), stream.read"),
            "run_status probe read must be bounded by a 2-second timeout"
        );
    }
}
