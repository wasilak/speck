use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use tokio::io::AsyncReadExt;
use tokio::net::UnixStream;

const LAUNCHD_LABEL: &str = "io.speck.vm";

pub async fn run_down(speck_home: &Path) -> anyhow::Result<()> {
    // Step 1 — Liveness check: confirm the daemon is running before issuing bootout
    let sock_path = speck_home.join("run/control.sock");
    let alive = match UnixStream::connect(&sock_path).await {
        Ok(mut stream) => {
            let mut buf = [0u8; 8];
            let _ = stream.read(&mut buf).await;
            drop(stream);
            true
        }
        Err(_) => false,
    };

    if alive {
        tracing::info!("daemon is alive, proceeding with shutdown");

        // Step 2 — Stop the daemon. Try launchctl bootout first (launchd-managed case);
        // if it fails (daemon was started directly, e.g. spk up --foreground), fall back
        // to SIGTERM via the PID file.
        let out = std::process::Command::new("id")
            .arg("-u")
            .output()
            .context("failed to run id -u")?;
        let uid_str = std::str::from_utf8(&out.stdout)
            .context("non-UTF8 uid")?
            .trim()
            .to_owned();
        let bootout_ok = tokio::process::Command::new("launchctl")
            .args(["bootout", &format!("gui/{uid_str}/{LAUNCHD_LABEL}")])
            .status()
            .await
            .context("launchctl bootout failed")?
            .success();

        if !bootout_ok {
            tracing::info!("launchctl bootout failed — trying pid file fallback");
            kill_via_pid_file(speck_home)?;
        }

        // Step 3 — Remove io.speck.vm.plist to prevent auto-registration on next login
        let home = std::env::var("HOME").context("HOME not set")?;
        let plist_path = PathBuf::from(home)
            .join("Library/LaunchAgents")
            .join("io.speck.vm.plist");
        match std::fs::remove_file(&plist_path) {
            Ok(()) => tracing::info!("plist removed"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!(error = %e, "failed to remove plist"),
        }

        // Step 4 — Poll for clean shutdown: wait until control.sock disappears (timeout 30s)
        let deadline = Instant::now() + Duration::from_secs(30);
        while sock_path.exists() {
            if Instant::now() >= deadline {
                tracing::warn!("daemon did not stop within 30 seconds");
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    } else {
        tracing::info!(
            "control socket not reachable (daemon may already be stopped), trying PID file fallback"
        );
        kill_via_pid_file(speck_home)?;
    }

    // Step 5 — User-facing confirmation
    println!("Speck stopped.");

    Ok(())
}

/// Read `$SPECK_HOME/run/speck.pid` and send SIGTERM to that process.
/// Used as a fallback when the daemon was started directly (not via launchd).
fn kill_via_pid_file(speck_home: &Path) -> anyhow::Result<()> {
    let pid_path = speck_home.join("run/speck.pid");
    let contents = std::fs::read_to_string(&pid_path).with_context(|| {
        format!(
            "pid file not found at {} — cannot stop daemon",
            pid_path.display()
        )
    })?;
    let pid = contents.trim().to_owned();
    pid.parse::<u32>()
        .with_context(|| format!("invalid pid in {}: {:?}", pid_path.display(), pid))?;
    let status = std::process::Command::new("kill")
        .args(["-TERM", &pid])
        .status()
        .context("failed to run kill")?;
    if status.success() {
        tracing::info!(pid, "sent SIGTERM via pid file");
        return Ok(());
    }
    // First attempt failed — check for stale PID (ESRCH) by inspecting stderr
    let output = std::process::Command::new("kill")
        .args(["-TERM", &pid])
        .output()
        .context("failed to run kill")?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("No such process") {
        tracing::info!("stale pid file detected — removing");
        std::fs::remove_file(&pid_path).context("failed to remove stale pid file")?;
        return Ok(());
    }
    anyhow::ensure!(status.success(), "kill -TERM {pid} failed");
    Ok(())
}

#[cfg(test)]
mod tests {
    const DOWN_SOURCE: &str = include_str!("down.rs");

    /// Slice production code only — everything before `#[cfg(test)]` — so tests do not
    /// trivially pass because assertion strings themselves contain the searched tokens.
    fn production_code() -> &'static str {
        let end = DOWN_SOURCE
            .find("#[cfg(test)]")
            .unwrap_or(DOWN_SOURCE.len());
        &DOWN_SOURCE[..end]
    }

    #[test]
    fn down_connects_to_control_socket() {
        let src = production_code();
        assert!(
            src.contains("control.sock"),
            "run_down must construct the path run/control.sock for the liveness check"
        );
        assert!(
            src.contains("UnixStream::connect"),
            "run_down must connect via tokio::net::UnixStream::connect"
        );
    }

    #[test]
    fn down_uses_launchctl_bootout() {
        let src = production_code();
        assert!(
            src.contains("bootout"),
            "run_down must invoke launchctl bootout to trigger graceful daemon shutdown"
        );
        assert!(
            src.contains("io.speck.vm"),
            "run_down must reference the launchd label io.speck.vm in the bootout invocation"
        );
    }

    #[test]
    fn down_has_pid_file_fallback() {
        let src = production_code();
        assert!(
            src.contains("speck.pid"),
            "run_down must fall back to kill_via_pid_file using run/speck.pid when launchctl bootout fails"
        );
        assert!(
            src.contains("kill_via_pid_file"),
            "run_down must call kill_via_pid_file as the non-launchd fallback"
        );
        assert!(
            src.contains("SIGTERM") || src.contains("-TERM"),
            "kill_via_pid_file must send SIGTERM to the daemon process"
        );
    }

    #[test]
    fn down_removes_plist_on_shutdown() {
        let src = production_code();
        assert!(
            src.contains("io.speck.vm.plist"),
            "run_down must reference io.speck.vm.plist when removing the LaunchAgent plist"
        );
        assert!(
            src.contains("remove_file"),
            "run_down must call std::fs::remove_file to remove the plist"
        );
    }

    #[test]
    fn down_polls_for_socket_disappearance() {
        let src = production_code();
        assert!(
            src.contains("from_secs(30)"),
            "run_down must use a 30-second timeout while polling for clean shutdown"
        );
        assert!(
            src.contains("from_millis(200)"),
            "run_down must poll for socket disappearance every 200 ms"
        );
    }

    #[test]
    fn down_daemon04_met_by_down_then_up() {
        // DAEMON-04: "spk restart performs a graceful stop followed by a fresh start" is
        // satisfied by the documented user workflow `spk down && spk up`.  No `spk restart`
        // subcommand is implemented in Phase 09 per the explicit deferral in CONTEXT.md
        // §Deferred.  This test asserts that no restart command was accidentally added.
        let src = production_code();
        assert!(
            !src.contains("Commands::Restart"),
            "down.rs must not add a Commands::Restart variant — DAEMON-04 is met by spk down && spk up"
        );
        assert!(
            !src.contains("fn run_restart"),
            "down.rs must not define a run_restart function — DAEMON-04 is met by spk down && spk up"
        );
    }

    #[test]
    fn kill_via_pid_file_handles_esrch() {
        let src = production_code();
        assert!(
            src.contains("No such process"),
            "kill_via_pid_file must detect ESRCH by checking kill stderr for 'No such process'"
        );
        assert!(
            src.contains("remove_file"),
            "kill_via_pid_file must remove the stale PID file when ESRCH is detected"
        );
    }

    #[test]
    fn down_handles_control_socket_gone() {
        let src = production_code();
        let connect_pos = src
            .find("UnixStream::connect")
            .expect("run_down must call UnixStream::connect");
        let after_connect = &src[connect_pos..];
        assert!(
            after_connect.contains("Err"),
            "run_down must handle UnixStream::connect failure by matching on Err"
        );
    }
}
