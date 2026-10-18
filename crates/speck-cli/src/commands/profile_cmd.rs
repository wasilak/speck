//! `spk profile`: list, select, create and remove profiles.

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use clap::Parser as _;

use crate::profile::{self, Profile};

fn user_home() -> anyhow::Result<PathBuf> {
    std::env::var("HOME")
        .map(PathBuf::from)
        .context("HOME not set")
}

fn ensure_profiles_available() -> anyhow::Result<()> {
    anyhow::ensure!(
        !profile::current().raw_home,
        "profiles are not available with SPECK_HOME set; unset it first"
    );
    Ok(())
}

/// Whether `profile` exists: the default always does, a named one once its
/// directory was created.
fn exists(profile: &Profile) -> bool {
    profile.is_default() || profile.home.is_dir()
}

/// `spk profile use <name>`: make `name` the profile bare `spk` commands act on.
pub fn run_use(name: &str) -> anyhow::Result<()> {
    ensure_profiles_available()?;
    let user = user_home()?;
    let target = Profile::named(name, &user)?;
    anyhow::ensure!(
        exists(&target),
        "profile \"{name}\" does not exist; create it with `spk profile create {name}` \
         or `spk start -p {name}`"
    );
    profile::store_current(&user, name)?;
    println!("Current profile: {name}");
    let overridden = matches!(
        profile::selected_source(),
        Some(profile::Source::Env | profile::Source::Flag)
    ) && profile::current().name != name;
    if overridden {
        eprintln!(
            "warning: SPECK_PROFILE or -p is set and takes precedence over `spk profile use`"
        );
    }
    // Like `colima start -p`: point the docker CLI at the selected profile
    // when it is running.
    if target.home.join("run/control.sock").exists() {
        let context = target.docker_context();
        let used = std::process::Command::new("docker")
            .args(["context", "use", &context])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if used {
            println!("Docker context: {context}");
        }
    }
    Ok(())
}

/// `spk profile current`: the profile this invocation resolves to.
pub fn run_current() {
    println!("{}", profile::current().name);
}

/// `spk profile create <name>`: create the profile and its config.yaml
/// without starting it.
pub fn run_create(
    name: &str,
    cpus: Option<u64>,
    memory: Option<u64>,
    disk: Option<u64>,
    no_autostart: bool,
) -> anyhow::Result<()> {
    ensure_profiles_available()?;
    let target = Profile::named(name, &user_home()?)?;
    anyhow::ensure!(
        !target.is_default(),
        "the default profile always exists; set its resources with `spk start --cpus/--memory/--disk`"
    );
    anyhow::ensure!(!target.home.exists(), "profile \"{name}\" already exists");
    create_profile_dir(&target.home, cpus, memory, disk)?;
    if no_autostart {
        crate::config::persist_autostart(&target.home, false)?;
    }
    println!("Profile \"{name}\" created in {}", target.home.display());
    println!("Start it with `spk start -p {name}`.");
    Ok(())
}

fn create_profile_dir(
    home: &Path,
    cpus: Option<u64>,
    memory: Option<u64>,
    disk: Option<u64>,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(home)
        .with_context(|| format!("failed to create {}", home.display()))?;
    let mut argv = vec!["up".to_owned()];
    for (flag, value) in [("--cpus", cpus), ("--memory", memory), ("--disk", disk)] {
        if let Some(value) = value {
            argv.push(flag.to_owned());
            // Plain numbers keep their unit: MiB for memory, GiB for disk.
            argv.push(value.to_string());
        }
    }
    let args = crate::UpArgs::try_parse_from(&argv)?;
    let written = crate::config::persist_vm_overrides(home, &args).and_then(|saved| {
        if saved.is_empty() {
            std::fs::write(home.join("config.yaml"), "version: 1\n")?;
        }
        Ok(())
    });
    if let Err(e) = written {
        let _ = std::fs::remove_dir_all(home);
        return Err(e);
    }
    Ok(())
}

/// `spk profile rm <name>`: `spk -p <name> delete`, run as a child so the
/// whole delete path acts on that profile.
pub async fn run_rm(name: &str, force: bool) -> anyhow::Result<i32> {
    ensure_profiles_available()?;
    let target = Profile::named(name, &user_home()?)?;
    anyhow::ensure!(exists(&target), "profile \"{name}\" does not exist");
    let binary = std::env::current_exe().context("cannot find own binary")?;
    let mut cmd = tokio::process::Command::new(binary);
    cmd.args(["--profile", name, "delete"]);
    if force {
        cmd.arg("--force");
    }
    // The child must not inherit this process's profile selection.
    cmd.env_remove("SPECK_PROFILE");
    let status = cmd.status().await.context("failed to run spk delete")?;
    if status.success() && profile::stored_current(&user_home()?).as_deref() == Some(name) {
        profile::store_current(&user_home()?, profile::DEFAULT)?;
        println!("Current profile: default");
    }
    Ok(status.code().unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_writes_sizes_into_the_profile_config() {
        let dir = std::env::temp_dir().join(format!("speck-profile-create-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        temp_env::with_vars(
            [
                ("SPECK_VM_CPUS", None::<&str>),
                ("SPECK_VM_MEMORY_MB", None),
                ("SPECK_VM_DISK_GB", None),
            ],
            || {
                create_profile_dir(&dir, Some(1), Some(2048), None).unwrap();
            },
        );
        let text = std::fs::read_to_string(dir.join("config.yaml")).unwrap();
        assert!(text.contains("cpus: 1"), "{text}");
        assert!(text.contains("memory_mb: 2048"), "{text}");

        let bare = dir.join("bare");
        create_profile_dir(&bare, None, None, None).unwrap();
        assert_eq!(
            std::fs::read_to_string(bare.join("config.yaml")).unwrap(),
            "version: 1\n"
        );
    }
}
