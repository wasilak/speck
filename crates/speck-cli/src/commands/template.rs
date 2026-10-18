//! `spk template` and `spk start --edit`: edit `<home>/config.yaml` in
//! `$VISUAL`/`$EDITOR`, like `colima template` and `colima start --edit`.

use std::io::{BufRead as _, IsTerminal as _, Write as _};
use std::path::Path;

use anyhow::Context as _;

use crate::config::EffectiveConfig;

/// A commented config.yaml listing every key with its default. The values
/// come from `EffectiveConfig::default()`, so they follow the loader.
pub fn render_template(profile: &str) -> String {
    let d = EffectiveConfig::default();
    let target = if profile == crate::profile::DEFAULT {
        "the default profile".to_owned()
    } else {
        format!("profile \"{profile}\"")
    };
    format!(
        "# Speck configuration for {target}.
# Every key is optional. Commented values are the defaults; uncomment a line
# to change it. Sizes take k/m/g/t units (6g, 6144m, 1.5g).
# Changes apply on the next `spk start` or `spk restart`.
version: 1

# vm:
#   cpus: {cpus}
#   memory_mb: {memory}      # a plain number is MiB
#   disk_gb: {disk}         # a plain number is GiB; the disk only grows

# log_level: {log_level}       # tracing filter, e.g. debug or speck_net=trace

# ca:
#   keychain: {keychain}          # add certificates trusted in the Keychain to the VM
#   inject_into_containers: {inject}  # give containers the same CA bundle
#   extra_certs: []         # extra PEM files, e.g. [/etc/ssl/certs/corp-ca.pem]

# autostart: {autostart}          # false: no launchd agent, no restart at login (CI)
",
        cpus = d.vm.cpus,
        memory = d.vm.memory_mb,
        disk = d.vm.disk_gb,
        log_level = d.log_level,
        keychain = d.ca_from_keychain,
        inject = d.inject_ca_into_containers,
        autostart = d.autostart,
    )
}

/// The editor command: `$VISUAL`, then `$EDITOR`, then `vi`.
fn editor() -> String {
    ["VISUAL", "EDITOR"]
        .iter()
        .filter_map(|var| std::env::var(var).ok())
        .find(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "vi".to_owned())
}

fn open_editor(path: &Path) -> anyhow::Result<()> {
    let editor = editor();
    // Through sh, so `EDITOR="code -w"` works like it does for git.
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} \"$1\""))
        .arg("sh")
        .arg(path)
        .status()
        .with_context(|| format!("failed to run editor `{editor}`"))?;
    anyhow::ensure!(status.success(), "editor `{editor}` exited with {status}");
    Ok(())
}

fn ask_reedit() -> bool {
    if !std::io::stdin().is_terminal() {
        return false;
    }
    eprint!("Open the editor again? [Y/n] ");
    let _ = std::io::stderr().flush();
    let mut answer = String::new();
    if std::io::stdin().lock().read_line(&mut answer).is_err() {
        return false;
    }
    !matches!(answer.trim().to_ascii_lowercase().as_str(), "n" | "no")
}

/// Edit the profile config. Returns whether the file is valid afterwards;
/// an invalid file is kept as the user left it.
pub fn edit_config(home: &Path, profile: &crate::profile::Profile) -> anyhow::Result<bool> {
    std::fs::create_dir_all(home)
        .with_context(|| format!("failed to create {}", home.display()))?;
    let path = home.join("config.yaml");
    if !path.exists() {
        std::fs::write(&path, render_template(&profile.name))
            .with_context(|| format!("failed to write {}", path.display()))?;
    }
    let before = std::fs::read(&path).ok();
    loop {
        open_editor(&path)?;
        let (errors, warnings) = super::config_cmd::problems(home);
        for warning in &warnings {
            eprintln!("warning: {warning}");
        }
        if errors.is_empty() {
            break;
        }
        for error in &errors {
            eprintln!("error: {error}");
        }
        if !ask_reedit() {
            eprintln!(
                "{} is kept as it is; fix it and run `spk config`.",
                path.display()
            );
            return Ok(false);
        }
    }
    let changed = std::fs::read(&path).ok() != before;
    if changed && home.join("run/control.sock").exists() {
        println!(
            "Speck{} is running; run `spk restart` to apply the changes.",
            profile.display_suffix()
        );
    }
    Ok(true)
}

/// `spk template`: exit code 0 when the edited config is valid.
pub fn run_template(home: &Path, profile: &crate::profile::Profile) -> anyhow::Result<i32> {
    let valid = edit_config(home, profile)?;
    if valid {
        println!("{}", home.join("config.yaml").display());
    }
    Ok(if valid { 0 } else { 1 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AppConfig, collect_unknown_keys};

    #[test]
    fn template_parses_back_into_the_default_config() {
        for profile in ["default", "work"] {
            let text = render_template(profile);
            let value: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
            assert!(collect_unknown_keys(&value).is_empty());
            let parsed: AppConfig = serde_yaml::from_value(value).unwrap();
            assert_eq!(parsed, AppConfig::default(), "{profile}");
        }
    }

    #[test]
    fn uncommented_template_lines_are_valid_keys() {
        // Uncommenting every key must give a config the loader accepts,
        // with the same values as the defaults.
        let text: String = render_template("work")
            .lines()
            .filter_map(|line| {
                let Some(body) = line.strip_prefix("# ") else {
                    return (!line.starts_with('#')).then(|| line.to_owned());
                };
                let key = body.split(':').next().unwrap_or_default();
                let is_key = body.starts_with("  ")
                    || (body.contains(':')
                        && key.chars().all(|c| c.is_ascii_lowercase() || c == '_'));
                is_key.then(|| body.to_owned())
            })
            .map(|line| line + "\n")
            .collect();
        let value: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
        assert!(collect_unknown_keys(&value).is_empty(), "{text}");
        let parsed: AppConfig = serde_yaml::from_value(value).unwrap();
        let d = EffectiveConfig::default();
        assert_eq!(parsed.vm.cpus, Some(d.vm.cpus), "{text}");
        assert_eq!(parsed.vm.memory_mb, Some(d.vm.memory_mb));
        assert_eq!(parsed.vm.disk_gb, Some(d.vm.disk_gb));
        assert_eq!(parsed.log_level.as_deref(), Some(d.log_level.as_str()));
        assert_eq!(parsed.ca.keychain, Some(d.ca_from_keychain));
        assert_eq!(parsed.autostart, Some(d.autostart));
    }

    #[test]
    fn editor_falls_back_to_vi() {
        temp_env::with_vars([("VISUAL", None::<&str>), ("EDITOR", Some(" "))], || {
            assert_eq!(editor(), "vi");
        });
        temp_env::with_vars(
            [("VISUAL", Some("code -w")), ("EDITOR", Some("nano"))],
            || {
                assert_eq!(editor(), "code -w");
            },
        );
    }

    #[test]
    fn edit_with_a_noop_editor_creates_a_valid_config() {
        let dir = std::env::temp_dir().join(format!("speck-template-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let profile = crate::profile::Profile::raw(dir.clone());
        temp_env::with_vars(
            [
                ("VISUAL", Some("true")),
                ("SPECK_VM_CPUS", None),
                ("SPECK_VM_MEMORY_MB", None),
                ("SPECK_VM_DISK_GB", None),
                ("SPECK_LOG_LEVEL", None),
            ],
            || {
                assert!(edit_config(&dir, &profile).unwrap());
            },
        );
        assert!(
            std::fs::read_to_string(dir.join("config.yaml"))
                .unwrap()
                .contains("# vm:")
        );

        // An invalid edit is reported and the file is kept.
        std::fs::write(dir.join("config.yaml"), "version: 1\nvm: {cpus: abc}\n").unwrap();
        temp_env::with_var("VISUAL", Some("true"), || {
            assert!(!edit_config(&dir, &profile).unwrap());
        });
        assert!(
            std::fs::read_to_string(dir.join("config.yaml"))
                .unwrap()
                .contains("cpus: abc")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
