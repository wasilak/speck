//! `spk ssh`, `spk ssh-config` and the hidden `spk ssh-proxy`: a shell in
//! the VM, like `colima ssh`. The guest runs dropbear on its own loopback;
//! the daemon serves `<home>/ssh.sock`, which reaches it over vsock, and
//! OpenSSH connects through `spk ssh-proxy` as its ProxyCommand.

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use anyhow::Context as _;

use crate::profile::Profile;

/// Guest port dropbear listens on (guest loopback only).
pub const GUEST_SSH_PORT: u16 = 22;

fn ssh_dir(home: &Path) -> PathBuf {
    home.join("ssh")
}

pub fn identity_file(home: &Path) -> PathBuf {
    ssh_dir(home).join("id_ed25519")
}

pub fn ssh_socket(home: &Path) -> PathBuf {
    home.join("ssh.sock")
}

/// Create `<home>/ssh/id_ed25519` if missing and keep
/// `<home>/ssh/authorized_keys` (read by the guest) in step with its public
/// key. The directory is 0700 and the files 0600.
pub fn ensure_keys(home: &Path, profile: &str) -> anyhow::Result<()> {
    let dir = ssh_dir(home);
    std::fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    let key = identity_file(home);
    if !key.exists() {
        let status = std::process::Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", ""])
            .arg("-C")
            .arg(format!("speck-{profile}"))
            .arg("-f")
            .arg(&key)
            .stdin(std::process::Stdio::null())
            .status()
            .context("failed to run ssh-keygen")?;
        anyhow::ensure!(status.success(), "ssh-keygen exited with {status}");
    }
    let public = std::fs::read(key.with_extension("pub"))
        .with_context(|| format!("failed to read {}.pub", key.display()))?;
    let authorized = dir.join("authorized_keys");
    if std::fs::read(&authorized).ok().as_deref() != Some(public.as_slice()) {
        std::fs::write(&authorized, &public)
            .with_context(|| format!("failed to write {}", authorized.display()))?;
    }
    for file in [&key, &authorized] {
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// The spk path for ProxyCommand. A Homebrew Cellar path changes with every
/// upgrade, so use the stable brew symlink instead.
pub fn spk_path() -> anyhow::Result<PathBuf> {
    let exe = std::env::current_exe().context("cannot find own binary")?;
    Ok(stable_spk_path(&exe, Path::new("/opt/homebrew/bin/spk")))
}

fn stable_spk_path(exe: &Path, brew_link: &Path) -> PathBuf {
    let in_cellar = exe.components().any(|c| c.as_os_str() == "Cellar");
    if in_cellar && brew_link.exists() {
        brew_link.to_owned()
    } else {
        exe.to_owned()
    }
}

/// Quote for the `sh -c` OpenSSH runs ProxyCommand with.
fn shell_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+=:@".contains(c))
    {
        s.to_owned()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

fn proxy_command(spk: &Path, profile: &Profile) -> String {
    let mut cmd = format!("{} ssh-proxy", shell_quote(&spk.to_string_lossy()));
    if profile.managed() {
        cmd.push_str(&format!(" --profile {}", profile.name));
    }
    cmd
}

/// The ssh host name: `speck`, or `speck-<name>` for other profiles.
pub fn host_alias(profile: &Profile) -> String {
    if profile.is_default() || !profile.managed() {
        "speck".to_owned()
    } else {
        format!("speck-{}", profile.name)
    }
}

/// `ssh` arguments for `spk ssh [-- command...]`.
pub fn ssh_argv(profile: &Profile, spk: &Path, command: &[String]) -> Vec<String> {
    let home = &profile.home;
    let mut argv = vec![
        "-i".to_owned(),
        identity_file(home).display().to_string(),
        "-o".to_owned(),
        "IdentitiesOnly=yes".to_owned(),
        "-o".to_owned(),
        format!("ProxyCommand={}", proxy_command(spk, profile)),
        "-o".to_owned(),
        format!(
            "UserKnownHostsFile={}",
            ssh_dir(home).join("known_hosts").display()
        ),
        "-o".to_owned(),
        "StrictHostKeyChecking=accept-new".to_owned(),
        // No "Permanently added ... to the list of known hosts" on first use.
        "-o".to_owned(),
        "LogLevel=ERROR".to_owned(),
    ];
    if command.is_empty() {
        argv.push("-t".to_owned());
    }
    argv.push(format!("root@{}", host_alias(profile)));
    if !command.is_empty() {
        argv.push("--".to_owned());
        argv.extend(command.iter().cloned());
    }
    argv
}

fn not_running(profile: &Profile) -> anyhow::Error {
    anyhow::anyhow!(
        "Speck{} is not running; run `spk start`",
        profile.display_suffix()
    )
}

/// `spk ssh [-- command...]`: exec the system ssh into the VM.
pub fn run_ssh(profile: &Profile, command: &[String]) -> anyhow::Result<i32> {
    let home = &profile.home;
    if std::os::unix::net::UnixStream::connect(ssh_socket(home)).is_err() {
        if std::os::unix::net::UnixStream::connect(home.join("run/control.sock")).is_ok() {
            anyhow::bail!(
                "this VM was started by an older spk; run `spk restart` to enable `spk ssh`"
            );
        }
        return Err(not_running(profile));
    }
    ensure_keys(home, &profile.name)?;
    let argv = ssh_argv(profile, &spk_path()?, command);
    use std::os::unix::process::CommandExt as _;
    let err = std::process::Command::new("ssh").args(&argv).exec();
    Err(anyhow::Error::new(err).context("failed to run ssh"))
}

/// `spk ssh-proxy`: copy stdin/stdout to `<home>/ssh.sock` (OpenSSH
/// ProxyCommand). Exits the process when the VM side closes: the blocking
/// stdin reader would otherwise keep the runtime from shutting down.
pub async fn run_ssh_proxy(profile: &Profile) -> anyhow::Result<()> {
    let stream = tokio::net::UnixStream::connect(ssh_socket(&profile.home))
        .await
        .map_err(|_| not_running(profile))?;
    let (mut from_vm, mut to_vm) = stream.into_split();
    tokio::spawn(async move {
        let _ = tokio::io::copy(&mut tokio::io::stdin(), &mut to_vm).await;
        let _ = tokio::io::AsyncWriteExt::shutdown(&mut to_vm).await;
    });
    let mut stdout = tokio::io::stdout();
    let received = tokio::io::copy(&mut from_vm, &mut stdout).await;
    let _ = tokio::io::AsyncWriteExt::flush(&mut stdout).await;
    if matches!(received, Ok(0)) {
        // The guest closed at once: nothing listens on its port 22.
        eprintln!(
            "spk: no SSH server in the VM{}. It needs rootfs 1.4.0 or newer: \
             upgrade spk and run `spk restart`.",
            profile.display_suffix()
        );
        std::process::exit(1);
    }
    std::process::exit(0);
}

/// The `spk ssh-config` block for `~/.ssh/config`.
pub fn ssh_config(profile: &Profile, spk: &Path) -> String {
    let home = &profile.home;
    let alias = host_alias(profile);
    format!(
        "Host {alias}
  User root
  HostName {alias}
  IdentityFile {key}
  IdentitiesOnly yes
  UserKnownHostsFile {known}
  StrictHostKeyChecking accept-new
  LogLevel ERROR
  ProxyCommand {proxy}
",
        key = identity_file(home).display(),
        known = ssh_dir(home).join("known_hosts").display(),
        proxy = proxy_command(spk, profile),
    )
}

/// `spk ssh-config`: print the block; never edits `~/.ssh/config`.
pub fn run_ssh_config(profile: &Profile) -> anyhow::Result<()> {
    ensure_keys(&profile.home, &profile.name)?;
    print!("{}", ssh_config(profile, &spk_path()?));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work() -> Profile {
        Profile::named("work", Path::new("/Users/me")).unwrap()
    }

    #[test]
    fn ssh_argv_for_a_named_profile() {
        let argv = ssh_argv(&work(), Path::new("/opt/homebrew/bin/spk"), &[]);
        let joined = argv.join(" ");
        assert!(
            joined.contains("-i /Users/me/.speck/profiles/work/ssh/id_ed25519"),
            "{joined}"
        );
        assert!(joined.contains("IdentitiesOnly=yes"));
        assert!(
            joined.contains("ProxyCommand=/opt/homebrew/bin/spk ssh-proxy --profile work"),
            "{joined}"
        );
        assert!(
            joined.contains("UserKnownHostsFile=/Users/me/.speck/profiles/work/ssh/known_hosts")
        );
        assert!(joined.contains("StrictHostKeyChecking=accept-new"));
        assert_eq!(argv.last().unwrap(), "root@speck-work");
        assert!(
            argv.contains(&"-t".to_owned()),
            "interactive shell needs a tty"
        );
    }

    #[test]
    fn ssh_argv_passes_the_command_after_the_host() {
        let cmd = vec!["uname".to_owned(), "-a".to_owned()];
        let default = Profile::named("default", Path::new("/Users/me")).unwrap();
        let argv = ssh_argv(&default, Path::new("/usr/local/bin/spk"), &cmd);
        let host = argv.iter().position(|a| a == "root@speck").unwrap();
        assert_eq!(&argv[host + 1..], ["--", "uname", "-a"]);
        assert!(!argv.contains(&"-t".to_owned()));
        assert!(
            argv.join(" ")
                .contains("ProxyCommand=/usr/local/bin/spk ssh-proxy --profile default")
        );
    }

    #[test]
    fn ssh_config_for_default_and_named_profiles() {
        let spk = Path::new("/opt/homebrew/bin/spk");
        let default = Profile::named("default", Path::new("/Users/me")).unwrap();
        assert_eq!(
            ssh_config(&default, spk),
            "Host speck
  User root
  HostName speck
  IdentityFile /Users/me/.speck/ssh/id_ed25519
  IdentitiesOnly yes
  UserKnownHostsFile /Users/me/.speck/ssh/known_hosts
  StrictHostKeyChecking accept-new
  LogLevel ERROR
  ProxyCommand /opt/homebrew/bin/spk ssh-proxy --profile default
"
        );
        let named = ssh_config(&work(), spk);
        assert!(named.starts_with("Host speck-work\n"), "{named}");
        assert!(named.contains("IdentityFile /Users/me/.speck/profiles/work/ssh/id_ed25519"));
        assert!(named.contains("ssh-proxy --profile work"));
    }

    #[test]
    fn cellar_binaries_use_the_brew_symlink() {
        let dir = std::env::temp_dir().join(format!("speck-ssh-link-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let link = dir.join("spk");
        std::fs::write(&link, "").unwrap();
        let cellar = Path::new("/opt/homebrew/Cellar/speck/1.4.9/bin/spk");
        assert_eq!(stable_spk_path(cellar, &link), link);
        let dev = Path::new("/Users/me/git/speck/target/release/spk");
        assert_eq!(stable_spk_path(dev, &link), dev);
        assert_eq!(stable_spk_path(cellar, &dir.join("missing")), cellar);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn proxy_command_quotes_paths_with_spaces() {
        assert_eq!(shell_quote("/a b/spk"), "'/a b/spk'");
        assert_eq!(shell_quote("/usr/local/bin/spk"), "/usr/local/bin/spk");
    }

    #[test]
    fn keys_are_created_private_and_authorized() {
        let dir = std::env::temp_dir().join(format!("speck-ssh-home-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        ensure_keys(&dir, "test").unwrap();
        let key = identity_file(&dir);
        let public = std::fs::read_to_string(key.with_extension("pub")).unwrap();
        assert!(public.starts_with("ssh-ed25519 ") && public.trim_end().ends_with("speck-test"));
        let authorized = std::fs::read_to_string(dir.join("ssh/authorized_keys")).unwrap();
        assert_eq!(authorized, public);
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir.join("ssh")), 0o700);
        assert_eq!(mode(&key), 0o600);
        // A second run keeps the key.
        ensure_keys(&dir, "test").unwrap();
        assert_eq!(
            std::fs::read_to_string(key.with_extension("pub")).unwrap(),
            public
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
