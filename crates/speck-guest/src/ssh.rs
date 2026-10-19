//! SSH server in the guest (`spk ssh`, `spk ssh-config`).
//!
//! vminitd runs dropbear on guest `127.0.0.1:22` only: containers and the
//! guest network cannot reach it. The host reaches it through the vsock TCP
//! forwarder (target line `127.0.0.1:22`). Public-key auth only. The host
//! key lives on the data disk, so it stays the same across restarts, and
//! `authorized_keys` comes from the Speck home on the Mac.

use std::io;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::time::Duration;

/// dropbear in the rootfs (since rootfs 1.4.0).
pub const DROPBEAR: &str = "/usr/sbin/dropbear";
const DROPBEARKEY: &str = "/usr/bin/dropbearkey";
/// On the data disk (`/dev/vdb`), which survives restarts and upgrades.
pub const HOST_KEY: &str = "/var/lib/containerd/.speck/ssh/dropbear_ed25519_host_key";
const ROOT_SSH_DIR: &str = "/root/.ssh";
const AUTHORIZED_KEYS: &str = "/root/.ssh/authorized_keys";
pub const LISTEN: &str = "127.0.0.1:22";

/// Where `<home>/ssh/authorized_keys` is visible in the guest. The Mac's
/// /Users and /private/tmp are mounted at the same paths (identity roots),
/// so the home path from the kernel cmdline works as is; `/tmp` and `/var`
/// are symlinks into `/private` on the Mac.
pub fn authorized_keys_candidates(speck_home: &str) -> Vec<std::path::PathBuf> {
    let home = speck_home.trim_end_matches('/');
    let mut homes = vec![home.to_owned()];
    if home.starts_with("/tmp/") || home.starts_with("/var/") {
        homes.push(format!("/private{home}"));
    }
    homes
        .into_iter()
        .map(|h| Path::new(&h).join("ssh/authorized_keys"))
        .collect()
}

/// dropbear arguments: foreground, log to stderr (the console log), key
/// auth only, loopback only.
pub fn dropbear_args(host_key: &str) -> Vec<String> {
    [
        "-F", // foreground, so the supervisor can wait for it
        "-E", // log to stderr
        "-s", // no password logins
        // Port forwarding stays on: VS Code Remote-SSH needs it.
        "-p", LISTEN, "-r", host_key,
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect()
}

/// Copy `authorized_keys` from the VirtioFS home into root's home. dropbear
/// refuses key files not owned by the user (the VirtioFS file belongs to the
/// Mac user's uid), so it gets a root-owned 0600 copy. Returns whether the
/// copy changed.
pub fn sync_authorized_keys(source: &Path, ssh_dir: &Path, target: &Path) -> io::Result<bool> {
    let keys = match std::fs::read(source) {
        Ok(keys) => keys,
        Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e),
    };
    if std::fs::read(target).ok().as_deref() == Some(keys.as_slice()) {
        return Ok(false);
    }
    std::fs::create_dir_all(ssh_dir)?;
    std::fs::set_permissions(ssh_dir, std::fs::Permissions::from_mode(0o700))?;
    let tmp = target.with_extension("tmp");
    std::fs::write(&tmp, &keys)?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    std::fs::rename(&tmp, target)?;
    Ok(true)
}

fn ensure_host_key() -> io::Result<()> {
    if Path::new(HOST_KEY).exists() {
        return Ok(());
    }
    if let Some(dir) = Path::new(HOST_KEY).parent() {
        std::fs::create_dir_all(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let status = std::process::Command::new(DROPBEARKEY)
        .args(["-t", "ed25519", "-f", HOST_KEY])
        .stdout(std::process::Stdio::null())
        .status()?;
    if !status.success() {
        return Err(io::Error::other(format!("dropbearkey exited {status}")));
    }
    Ok(())
}

/// Start dropbear under a restart-with-backoff supervisor. With an old
/// rootfs that has no dropbear, log one line and do nothing else.
pub fn spawn_supervised(speck_home: Option<String>) {
    if !Path::new(DROPBEAR).exists() {
        eprintln!("vminitd: no dropbear in the rootfs; `spk ssh` is not available");
        return;
    }
    if let Err(e) = ensure_host_key() {
        eprintln!("vminitd: cannot create the ssh host key: {e}");
        return;
    }

    let candidates = speck_home
        .as_deref()
        .map(authorized_keys_candidates)
        .unwrap_or_default();
    if candidates.is_empty() {
        eprintln!("vminitd: no speck_home_path on the kernel cmdline; ssh logins will fail");
    }
    // Keys added on the Mac (a new profile key) work without a restart.
    std::thread::spawn(move || {
        loop {
            // The first candidate that exists, else the first one (missing
            // source: an empty key list).
            let source = candidates
                .iter()
                .find(|p| p.exists())
                .or(candidates.first())
                .cloned()
                .unwrap_or_default();
            if let Err(e) =
                sync_authorized_keys(&source, Path::new(ROOT_SSH_DIR), Path::new(AUTHORIZED_KEYS))
            {
                eprintln!("vminitd: cannot copy ssh authorized_keys: {e}");
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    });

    std::thread::spawn(|| {
        let mut backoff = Duration::from_secs(1);
        loop {
            let started = std::time::Instant::now();
            match std::process::Command::new(DROPBEAR)
                .args(dropbear_args(HOST_KEY))
                .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
                .spawn()
            {
                Ok(mut child) => {
                    eprintln!("vminitd: started dropbear on {LISTEN} (pid {})", child.id());
                    match child.wait() {
                        Ok(status) => eprintln!("vminitd: dropbear exited {status}; restarting"),
                        Err(e) => eprintln!("vminitd: dropbear wait error: {e}; restarting"),
                    }
                }
                Err(e) => eprintln!("vminitd: cannot start dropbear: {e}"),
            }
            if started.elapsed() > Duration::from_secs(60) {
                backoff = Duration::from_secs(1);
            }
            std::thread::sleep(backoff);
            backoff = (backoff * 2).min(Duration::from_secs(30));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropbear_listens_on_loopback_with_keys_only() {
        let args = dropbear_args("/k");
        let joined = args.join(" ");
        assert!(joined.contains("-p 127.0.0.1:22"), "{joined}");
        assert!(joined.contains("-r /k"), "{joined}");
        assert!(
            args.iter().any(|a| a == "-s"),
            "password logins must be off"
        );
        assert!(
            args.iter().any(|a| a == "-F"),
            "must stay in the foreground"
        );
    }

    #[test]
    fn authorized_keys_come_from_the_mac_home_path() {
        assert_eq!(
            authorized_keys_candidates("/Users/me/.speck"),
            [Path::new("/Users/me/.speck/ssh/authorized_keys")]
        );
        assert_eq!(
            authorized_keys_candidates("/tmp/spkt/"),
            [
                Path::new("/tmp/spkt/ssh/authorized_keys"),
                Path::new("/private/tmp/spkt/ssh/authorized_keys")
            ]
        );
    }

    #[test]
    fn authorized_keys_copy_is_private_and_only_rewritten_on_change() {
        let dir = std::env::temp_dir().join(format!("speck-ssh-keys-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("source");
        let ssh_dir = dir.join("root/.ssh");
        let target = ssh_dir.join("authorized_keys");

        // No source yet: an empty file, so dropbear rejects every key.
        assert!(sync_authorized_keys(&source, &ssh_dir, &target).unwrap());
        assert_eq!(std::fs::read(&target).unwrap(), b"");

        std::fs::write(&source, "ssh-ed25519 AAAA speck-default\n").unwrap();
        assert!(sync_authorized_keys(&source, &ssh_dir, &target).unwrap());
        assert!(!sync_authorized_keys(&source, &ssh_dir, &target).unwrap());
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "ssh-ed25519 AAAA speck-default\n"
        );
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&target), 0o600);
        assert_eq!(mode(&ssh_dir), 0o700);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
