use std::path::Path;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::net::UnixStream;
use tokio::time::timeout;

use crate::theme;

pub async fn run_status(speck_home: &Path) -> anyhow::Result<()> {
    let sock_path = speck_home.join("run/control.sock");
    let status = match timeout(Duration::from_secs(2), UnixStream::connect(&sock_path)).await {
        Ok(Ok(mut stream)) => {
            let mut buf = [0u8; 8];
            let _ = stream.read(&mut buf).await;
            "running"
        }
        _ => "stopped",
    };

    println!("Speck daemon: {}", theme::format_status(status));

    let pid_path = speck_home.join("speck.pid");
    if let Ok(pid_str) = std::fs::read_to_string(&pid_path) {
        if let Ok(pid) = pid_str.trim().parse::<u32>() {
            println!("  PID: {pid}");
        }
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
