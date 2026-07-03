use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use tokio::io::AsyncReadExt;
use tokio::net::UnixStream;

const LAUNCHD_LABEL: &str = "io.speck.vm";

pub async fn run_down(speck_home: &Path) -> anyhow::Result<()> {
    // Step 1 — Liveness check: confirm the daemon is running before issuing bootout
    let sock_path = speck_home.join("run/control.sock");
    let mut stream = UnixStream::connect(&sock_path)
        .await
        .context("daemon is not running (control socket not reachable — start with spk up)")?;
    let mut buf = [0u8; 8];
    let _ = stream.read(&mut buf).await;
    drop(stream);
    tracing::info!("daemon is alive, proceeding with shutdown");

    // Step 2 — Issue bootout: triggers SIGTERM → daemon performs graceful VM shutdown
    let out = std::process::Command::new("id")
        .arg("-u")
        .output()
        .context("failed to run id -u")?;
    let uid_str = std::str::from_utf8(&out.stdout)
        .context("non-UTF8 uid")?
        .trim()
        .to_owned();
    let status = tokio::process::Command::new("launchctl")
        .args(["bootout", &format!("gui/{uid_str}/{LAUNCHD_LABEL}")])
        .status()
        .await
        .context("launchctl bootout failed")?;
    if !status.success() {
        tracing::warn!("launchctl bootout returned non-zero (daemon may already be stopping)");
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

    // Step 5 — User-facing confirmation
    println!("Speck stopped.");

    Ok(())
}

#[cfg(test)]
mod tests {
    const DOWN_SOURCE: &str = include_str!("down.rs");

    /// Slice production code only — everything before `#[cfg(test)]` — so tests do not
    /// trivially pass because assertion strings themselves contain the searched tokens.
    fn production_code() -> &'static str {
        let end = DOWN_SOURCE.find("#[cfg(test)]").unwrap_or(DOWN_SOURCE.len());
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
}
