use std::path::Path;

pub async fn run_down(_speck_home: &Path) -> anyhow::Result<()> {
    println!("spk down: send SIGTERM to running 'spk up' process to shut down the VM.");
    println!("Alternatively, Ctrl-C the 'spk up' process.");
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
