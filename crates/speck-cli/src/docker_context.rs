//! colima-style docker context management (UX-01).
//!
//! `colima start` creates and selects a `colima` docker context, so plain
//! `docker` commands talk to it without any shell setup. Speck does the same
//! with a `speck` context. Only the default `SPECK_HOME` manages the context:
//! extra instances (tests, a custom SPECK_HOME) must not take over the
//! user's global docker CLI selection.

use std::path::Path;
use std::process::Command;

pub const CONTEXT: &str = "speck";

/// Whether this Speck instance owns the global `speck` docker context.
pub fn manages_context() -> bool {
    std::env::var_os("SPECK_HOME").is_none()
}

/// Create or update the `speck` context for `speck_home` and select it.
pub fn select(speck_home: &Path) {
    if !manages_context() {
        return;
    }
    let host = format!("host=unix://{}", speck_home.join("speck.sock").display());
    let exists = docker(&["context", "inspect", CONTEXT]).is_some_and(|ok| ok);
    let configured = if exists {
        docker(&["context", "update", CONTEXT, "--docker", &host])
    } else {
        docker(&[
            "context",
            "create",
            CONTEXT,
            "--description",
            "Speck",
            "--docker",
            &host,
        ])
    };
    match configured {
        None => tracing::info!("docker CLI not found; skipping docker context setup"),
        Some(false) => tracing::warn!("could not create/update the '{CONTEXT}' docker context"),
        Some(true) => {
            if docker(&["context", "use", CONTEXT]) == Some(true) {
                tracing::info!("docker context '{CONTEXT}' selected");
            } else {
                tracing::warn!("could not select the '{CONTEXT}' docker context");
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
    if !manages_context() {
        return;
    }
    if current().as_deref() == Some(CONTEXT) {
        let _ = docker(&["context", "use", "default"]);
    }
}

/// Deselect and delete the `speck` context.
pub fn remove() {
    if !manages_context() {
        return;
    }
    deselect();
    let _ = docker(&["context", "rm", "--force", CONTEXT]);
}

/// A `DOCKER_HOST` in the environment overrides any docker context; warn
/// when it points somewhere other than Speck.
pub fn docker_host_override_warning(speck_home: &Path) -> Option<String> {
    let host = std::env::var("DOCKER_HOST").ok()?;
    let speck = format!("unix://{}", speck_home.join("speck.sock").display());
    (host != speck).then(|| {
        format!(
            "DOCKER_HOST={host} is set and overrides the '{CONTEXT}' docker context; \
             unset it to use Speck"
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
    fn only_the_default_home_manages_the_context() {
        temp_env::with_var("SPECK_HOME", Some("/tmp/other"), || {
            assert!(!manages_context())
        });
        temp_env::with_var("SPECK_HOME", None::<&str>, || assert!(manages_context()));
    }
}
