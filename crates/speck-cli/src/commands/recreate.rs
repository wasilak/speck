use std::path::Path;

use anyhow::Context as _;

/// `spk recreate`: delete the VM and start a fresh one (colima delete +
/// colima start). Downloaded guest assets and `config.yaml` are kept.
pub async fn run_recreate(force: bool, speck_home: &Path) -> anyhow::Result<()> {
    if !crate::commands::delete::run_delete(force, speck_home).await? {
        return Ok(());
    }
    let binary = std::env::current_exe().context("cannot find own binary")?;
    let status = tokio::process::Command::new(binary)
        .args(["up", "--wait", "--timeout", "300"])
        .status()
        .await
        .context("failed to run spk start")?;
    anyhow::ensure!(status.success(), "spk start failed");
    Ok(())
}
