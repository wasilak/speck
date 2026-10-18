//! colima-style docker context management (UX-01).
//!
//! `colima start` creates and selects a `colima` docker context, so plain
//! `docker` commands talk to it without any shell setup. Speck does the same
//! with a `speck` context (`speck-<name>` for a profile). An instance in a
//! raw `SPECK_HOME` (tests) must not take over the user's global docker CLI
//! selection, so it manages no context.

use std::path::Path;
use std::process::Command;

use crate::profile::{self, Profile};

/// Whether this Speck instance owns a docker context.
pub fn manages_context() -> bool {
    profile::current().managed()
}

/// Name of this instance's docker context: `speck` or `speck-<profile>`.
pub fn context_name() -> String {
    profile::current().docker_context()
}

/// Create or update this instance's context for `speck_home`, and select it
/// when `activate` is set. Profiles with `autostart: false` (CI) only create
/// theirs: parallel jobs must not switch the user's docker CLI.
pub fn select(speck_home: &Path, activate: bool) {
    if !manages_context() {
        return;
    }
    let context = context_name();
    let context = context.as_str();
    let host = format!("host=unix://{}", speck_home.join("speck.sock").display());
    let exists = docker(&["context", "inspect", context]).is_some_and(|ok| ok);
    let configured = if exists {
        docker(&["context", "update", context, "--docker", &host])
    } else {
        docker(&[
            "context",
            "create",
            context,
            "--description",
            "Speck",
            "--docker",
            &host,
        ])
    };
    match configured {
        None => tracing::info!("docker CLI not found; skipping docker context setup"),
        Some(false) => tracing::warn!("could not create/update the '{context}' docker context"),
        Some(true) if !activate => tracing::info!("docker context '{context}' ready"),
        Some(true) => {
            if docker(&["context", "use", context]) == Some(true) {
                tracing::info!("docker context '{context}' selected");
            } else {
                tracing::warn!("could not select the '{context}' docker context");
            }
        }
    }
}

/// Name of the currently selected docker context, if the docker CLI is there.
pub fn current() -> Option<String> {
    Command::new("docker")
        .args(["context", "show"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
}

/// Switch the docker CLI back to `default` if `speck` is selected, so docker
/// commands do not hang on a stopped Speck.
pub fn deselect() {
    deselect_profile(&profile::current());
}

/// [`deselect`] for `profile`: only when its context is the selected one.
pub fn deselect_profile(profile: &Profile) {
    if !profile.managed() {
        return;
    }
    if current().as_deref() == Some(profile.docker_context().as_str()) {
        let _ = docker(&["context", "use", fallback_context(profile)]);
    }
}

/// Context to switch to when `profile` stops: the default Speck when it is
/// running, otherwise docker's `default`.
fn fallback_context(profile: &Profile) -> &'static str {
    let default_running = !profile.is_default()
        && std::env::var("HOME").is_ok_and(|home| {
            profile::default_home(Path::new(&home))
                .join("run/control.sock")
                .exists()
        });
    if default_running { "speck" } else { "default" }
}

/// Deselect and delete the `speck` context.
pub fn remove() {
    let profile = profile::current();
    if !profile.managed() {
        return;
    }
    deselect_profile(&profile);
    let _ = docker(&["context", "rm", "--force", &profile.docker_context()]);
}

/// A `DOCKER_HOST` in the environment overrides any docker context; warn
/// when it points somewhere other than Speck.
pub fn docker_host_override_warning(speck_home: &Path) -> Option<String> {
    let host = std::env::var("DOCKER_HOST").ok()?;
    let speck = format!("unix://{}", speck_home.join("speck.sock").display());
    (host != speck).then(|| {
        format!(
            "DOCKER_HOST={host} is set and overrides the '{}' docker context; \
             unset it to use Speck",
            context_name()
        )
    })
}

/// Run `docker <args>` quietly. `None` when the docker CLI is unavailable.
fn docker(args: &[&str]) -> Option<bool> {
    Command::new("docker")
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()
        .map(|s| s.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docker_host_pointing_elsewhere_is_reported() {
        let home = Path::new("/Users/x/.speck");
        temp_env::with_var(
            "DOCKER_HOST",
            Some("unix:///Users/x/.colima/default/docker.sock"),
            || {
                assert!(
                    docker_host_override_warning(home)
                        .unwrap()
                        .contains("colima")
                );
            },
        );
        temp_env::with_var(
            "DOCKER_HOST",
            Some("unix:///Users/x/.speck/speck.sock"),
            || {
                assert!(docker_host_override_warning(home).is_none());
            },
        );
        temp_env::with_var("DOCKER_HOST", None::<&str>, || {
            assert!(docker_host_override_warning(home).is_none());
        });
    }

    #[test]
    fn only_profiles_manage_a_context() {
        temp_env::with_vars(
            [("SPECK_HOME", Some("/tmp/other")), ("SPECK_PROFILE", None)],
            || assert!(!manages_context()),
        );
        temp_env::with_vars(
            [
                ("SPECK_HOME", None::<&str>),
                ("SPECK_PROFILE", Some("work")),
            ],
            || {
                assert!(manages_context());
                assert_eq!(context_name(), "speck-work");
            },
        );
    }
}
