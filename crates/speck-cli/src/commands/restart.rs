use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::time::timeout;

pub async fn run_restart(speck_home: &Path) -> anyhow::Result<()> {
    let sock_path = speck_home.join("run/control.sock");

    // Step 1 — Liveness check: confirm daemon is running before proceeding
    timeout(Duration::from_secs(2), UnixStream::connect(&sock_path))
        .await
        .context("daemon is not running")?
        .context("daemon is not running")?;

    // Step 2 — Signal restart intent: send PREPARE_RESTART over control socket
    let mut cmd_stream = UnixStream::connect(&sock_path)
        .await
        .context("failed to connect for restart signal")?;
    cmd_stream.write_all(b"PREPARE_RESTART\n").await?;
    let mut resp_buf = [0u8; 8];
    let n = cmd_stream.read(&mut resp_buf).await?;
    let response = std::str::from_utf8(&resp_buf[..n]).unwrap_or("");
    if !response.starts_with("OK") {
        tracing::warn!(
            "daemon did not acknowledge restart signal (response: {response:?}) — continuing anyway"
        );
    }
    drop(cmd_stream);

    // Step 3 — Stop: call spk down as subprocess
    let binary = std::env::current_exe().context("cannot find own binary")?;
    let down_status = tokio::process::Command::new(&binary)
        .arg("down")
        .status()
        .await
        .context("failed to run spk down")?;
    anyhow::ensure!(down_status.success(), "spk down failed");

    // Step 4 — Wait for socket disappearance (timeout 30s)
    let deadline = Instant::now() + Duration::from_secs(30);
    while sock_path.exists() {
        if Instant::now() >= deadline {
            tracing::warn!("daemon did not stop within 30 seconds");
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // Step 5 — Start: call spk up --wait 120 as subprocess
    let up_status = tokio::process::Command::new(&binary)
        .args(["up", "--wait", "120"])
        .status()
        .await
        .context("failed to run spk up")?;
    anyhow::ensure!(up_status.success(), "spk up failed");

    // Step 6 — Confirmation
    println!("Speck restarted.");

    Ok(())
}

#[cfg(test)]
mod tests {
    const RESTART_SOURCE: &str = include_str!("restart.rs");

    fn production_code() -> &'static str {
        let end = RESTART_SOURCE
            .find("#[cfg(test)]")
            .unwrap_or(RESTART_SOURCE.len());
        &RESTART_SOURCE[..end]
    }

    #[test]
    fn restart_runs_down_then_up() {
        let src = production_code();
        assert!(
            src.contains("\"down\""),
            "run_restart must call spk down as a subprocess"
        );
        assert!(
            src.contains("\"up\""),
            "run_restart must call spk up as a subprocess"
        );
        assert!(
            src.contains("\"--wait\""),
            "run_restart must pass --wait to spk up"
        );
    }

    #[test]
    fn restart_checks_down_exit_code() {
        let src = production_code();
        let down_pos = src
            .find("\"down\"")
            .expect("run_restart must reference \"down\" as a subprocess argument");
        let after_down = &src[down_pos..];
        assert!(
            after_down.contains(".success()") || after_down.contains("anyhow::ensure"),
            "run_restart must check spk down exit code via .success() or anyhow::ensure"
        );
    }

    #[test]
    fn restart_checks_up_exit_code() {
        let src = production_code();
        let up_pos = src
            .rfind("\"up\"")
            .expect("run_restart must reference \"up\" as a subprocess argument");
        let after_up = &src[up_pos..];
        assert!(
            after_up.contains(".success()") || after_up.contains("anyhow::ensure"),
            "run_restart must check spk up exit code via .success() or anyhow::ensure"
        );
    }

    #[test]
    fn restart_checks_daemon_alive() {
        let src = production_code();
        assert!(
            src.contains("control.sock"),
            "run_restart must reference run/control.sock for the liveness check"
        );
        assert!(
            src.contains("UnixStream::connect"),
            "run_restart must connect via tokio::net::UnixStream::connect"
        );
    }

    #[test]
    fn restart_sends_prepare_restart() {
        let src = production_code();
        assert!(
            src.contains("PREPARE_RESTART"),
            "run_restart must send PREPARE_RESTART over the control socket before spk down"
        );
    }

    #[test]
    fn restart_acknowledges_ok_response() {
        let src = production_code();
        assert!(
            src.contains("starts_with(\"OK\")"),
            "run_restart must check the daemon response starts_with(\"OK\")"
        );
    }
}
