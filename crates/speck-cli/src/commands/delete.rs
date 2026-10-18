use std::io::{BufRead as _, IsTerminal as _, Write as _};
use std::path::Path;

use anstream::println;

use crate::theme::NEON_CYAN;

/// Files and directories that make up the VM instance. Downloaded kernel and
/// initrd files, `config.yaml` and logs are kept, like colima keeps its image
/// cache: the next `spk start` creates a fresh VM without re-downloading them.
const INSTANCE_PATHS: &[&str] = &[
    "data.img",
    "rootfs.img",
    "rootfs.version",
    "ca-certs",
    "run",
    "speck.sock",
    "console.log",
];

/// `spk delete`: stop Speck and remove the VM, including all containers,
/// images and volumes (colima delete). With `whole_profile`, a named profile
/// is removed with its config and logs. Returns `false` when the user aborted.
pub async fn run_delete(
    force: bool,
    speck_home: &Path,
    whole_profile: bool,
) -> anyhow::Result<bool> {
    if !force {
        anyhow::ensure!(
            std::io::stdin().is_terminal(),
            "spk delete removes all containers, images and volumes; re-run with --force to confirm"
        );
        print!(
            "This deletes the Speck VM{} with all containers, images and volumes. Continue? [y/N] ",
            crate::profile::current().display_suffix()
        );
        std::io::stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin().lock().read_line(&mut answer)?;
        if !matches!(answer.trim(), "y" | "Y" | "yes") {
            println!("Aborted.");
            return Ok(false);
        }
    }

    // Never delete disks under a running VM: a failed stop is fatal unless
    // Speck was not running at all (no PID file).
    if let Err(e) = crate::commands::down::run_down(speck_home).await
        && speck_home.join("run/speck.pid").exists()
    {
        return Err(e);
    }
    crate::docker_context::remove();

    // A named profile is removed completely, like `colima delete -p <name>`;
    // its config and logs go with it. Shared downloads stay in ~/.speck.
    let profile = crate::profile::current();
    if whole_profile && profile.managed() && !profile.is_default() {
        if let Ok(plist) = profile.plist_path() {
            let _ = std::fs::remove_file(plist);
        }
        if let Err(e) = std::fs::remove_dir_all(speck_home)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            anyhow::bail!("failed to remove {}: {e}", speck_home.display());
        }
        println!(
            "{NEON_CYAN}Speck profile \"{}\" deleted.\x1b[0m",
            profile.name
        );
        return Ok(true);
    }

    for name in INSTANCE_PATHS {
        let path = speck_home.join(name);
        let removed = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        if let Err(e) = removed
            && e.kind() != std::io::ErrorKind::NotFound
        {
            anyhow::bail!("failed to remove {}: {e}", path.display());
        }
    }

    println!("{NEON_CYAN}Speck VM deleted.\x1b[0m");
    Ok(true)
}
