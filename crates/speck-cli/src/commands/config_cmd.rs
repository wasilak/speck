//! `spk config`: validate config.yaml and show the effective settings with
//! where each value comes from (default, config.yaml or an env variable).

use std::path::Path;

use clap::Parser as _;
use serde::Serialize;

use crate::config::{self, EffectiveConfig, EffectiveVmConfig};

use crate::output::{self, Output};

#[derive(Serialize)]
struct Row {
    key: &'static str,
    /// Typed value for `-o json|yaml`.
    value: serde_json::Value,
    /// Text for the plain table, e.g. "6144 (6 GiB)".
    #[serde(skip)]
    text: String,
    source: String,
}

/// `spk config -o json|yaml` document.
#[derive(Serialize, Default)]
struct ConfigDoc {
    path: String,
    exists: bool,
    valid: bool,
    errors: Vec<String>,
    warnings: Vec<String>,
    settings: Vec<Row>,
    /// Resources of the running VM when they differ from the settings.
    #[serde(skip_serializing_if = "Option::is_none")]
    running_vm: Option<EffectiveVmConfig>,
}

fn build(speck_home: &Path) -> ConfigDoc {
    let path = speck_home.join("config.yaml");
    let mut doc = ConfigDoc {
        path: path.display().to_string(),
        exists: path.exists(),
        ..ConfigDoc::default()
    };
    let raw: Option<serde_yaml::Value> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_yaml::from_str(&text).ok());
    let effective = config::load_config_file(speck_home).and_then(|(file, warnings)| {
        doc.warnings = warnings.into_iter().map(|w| w.message).collect();
        let no_flags = crate::UpArgs::try_parse_from(["up"])?;
        config::resolve_effective_config(file, &no_flags)
    });
    let effective = match effective {
        Ok(effective) => effective,
        Err(e) => {
            doc.errors.push(format!("{e:#}"));
            return doc;
        }
    };
    doc.errors = effective
        .extra_certs
        .iter()
        .filter(|cert| !Path::new(cert).is_file())
        .map(|cert| format!("ca.extra_certs: {cert} does not exist"))
        .collect();
    if !doc.errors.is_empty() {
        return doc;
    }
    doc.valid = true;
    doc.settings = rows(&effective, raw.as_ref());
    doc.running_vm = running_vm(speck_home).filter(|running| *running != effective.vm);
    doc
}

/// Errors and warnings for `<home>/config.yaml`, as `spk config` reports them.
pub fn problems(speck_home: &Path) -> (Vec<String>, Vec<String>) {
    let doc = build(speck_home);
    (doc.errors, doc.warnings)
}

/// Exit code 0 when the configuration is valid, 1 otherwise.
pub fn run_config(speck_home: &Path, format: Output) -> anyhow::Result<i32> {
    let doc = build(speck_home);
    let code = if doc.valid { 0 } else { 1 };
    if !format.is_plain() {
        output::emit(format, &doc)?;
        return Ok(code);
    }
    println!("Config: {}", doc.path);
    for warning in &doc.warnings {
        println!("  warning: {warning}");
    }
    for error in &doc.errors {
        println!("  invalid: {error}");
    }
    if !doc.valid {
        return Ok(code);
    }
    println!(
        "  {}\n",
        if doc.exists {
            "valid"
        } else {
            "not found; using defaults"
        }
    );

    let width = doc.settings.iter().map(|r| r.key.len()).max().unwrap_or(0);
    let value_width = doc.settings.iter().map(|r| r.text.len()).max().unwrap_or(0);
    for row in &doc.settings {
        println!(
            "{:width$}  {:value_width$}  {}",
            row.key, row.text, row.source
        );
    }

    if let Some(running) = &doc.running_vm {
        println!(
            "\nThe running VM uses {} CPUs, {} MiB memory, {} GiB disk; \
             run `spk restart` to apply the settings above.",
            running.cpus, running.memory_mb, running.disk_gb
        );
    }
    Ok(code)
}

fn rows(effective: &EffectiveConfig, raw: Option<&serde_yaml::Value>) -> Vec<Row> {
    let source = |key: &'static str, env: Option<&str>| -> String {
        if let Some(env) = env.filter(|e| std::env::var_os(e).is_some()) {
            return format!("env {env}");
        }
        let mut node = raw;
        for part in key.split('.') {
            node = node.and_then(|n| n.get(part));
        }
        if node.is_some() {
            "config.yaml".into()
        } else {
            "default".into()
        }
    };
    let vm = &effective.vm;
    let certs = if effective.extra_certs.is_empty() {
        "-".to_owned()
    } else {
        effective.extra_certs.join(", ")
    };
    vec![
        Row {
            key: "vm.cpus",
            value: vm.cpus.into(),
            text: vm.cpus.to_string(),
            source: source("vm.cpus", Some("SPECK_VM_CPUS")),
        },
        Row {
            key: "vm.memory_mb",
            value: vm.memory_mb.into(),
            text: format!("{} ({})", vm.memory_mb, human_mib(vm.memory_mb)),
            source: source("vm.memory_mb", Some("SPECK_VM_MEMORY_MB")),
        },
        Row {
            key: "vm.disk_gb",
            value: vm.disk_gb.into(),
            text: vm.disk_gb.to_string(),
            source: source("vm.disk_gb", Some("SPECK_VM_DISK_GB")),
        },
        Row {
            key: "autostart",
            value: effective.autostart.into(),
            text: effective.autostart.to_string(),
            source: source("autostart", None),
        },
        Row {
            key: "log_level",
            value: effective.log_level.clone().into(),
            text: effective.log_level.clone(),
            source: source("log_level", Some("SPECK_LOG_LEVEL")),
        },
        Row {
            key: "ca.keychain",
            value: effective.ca_from_keychain.into(),
            text: effective.ca_from_keychain.to_string(),
            source: source("ca.keychain", None),
        },
        Row {
            key: "ca.inject_into_containers",
            value: effective.inject_ca_into_containers.into(),
            text: effective.inject_ca_into_containers.to_string(),
            source: source("ca.inject_into_containers", None),
        },
        Row {
            key: "ca.extra_certs",
            value: effective.extra_certs.clone().into(),
            text: certs,
            source: source("ca.extra_certs", None),
        },
    ]
}

/// "6 GiB", "1.5 GiB" or "512 MiB".
fn human_mib(mib: u64) -> String {
    if mib >= 1024 {
        let gib = mib as f64 / 1024.0;
        if gib.fract() == 0.0 {
            format!("{gib} GiB")
        } else {
            format!("{gib:.1} GiB")
        }
    } else {
        format!("{mib} MiB")
    }
}

/// Resources of the running VM, when Speck is running.
fn running_vm(speck_home: &Path) -> Option<EffectiveVmConfig> {
    if !speck_home.join("run/control.sock").exists() {
        return None;
    }
    let text = std::fs::read_to_string(speck_home.join("run/vm-config.json")).ok()?;
    serde_json::from_str(&text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_mib_rounds_to_gib() {
        assert_eq!(human_mib(6144), "6 GiB");
        assert_eq!(human_mib(1536), "1.5 GiB");
        assert_eq!(human_mib(512), "512 MiB");
    }

    #[test]
    fn invalid_config_is_a_document_with_the_error() {
        let dir = std::env::temp_dir().join(format!("speck-config-doc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("config.yaml"),
            "version: 1\nvm:\n  memory_mb: 6x\n",
        )
        .unwrap();
        let doc = build(&dir);
        assert!(!doc.valid && doc.settings.is_empty());
        let v: serde_json::Value =
            serde_json::from_str(&output::render(Output::Json, &doc).unwrap()).unwrap();
        assert_eq!(v["valid"], false);
        assert!(v["errors"][0].as_str().unwrap().contains("vm.memory_mb"));
    }

    #[test]
    fn settings_are_typed_in_the_document() {
        let rows = temp_env::with_vars(
            [
                ("SPECK_VM_CPUS", None::<&str>),
                ("SPECK_VM_MEMORY_MB", None),
                ("SPECK_LOG_LEVEL", None),
            ],
            || rows(&EffectiveConfig::default(), None),
        );
        let v: serde_json::Value =
            serde_json::from_str(&output::render(Output::Json, &rows).unwrap()).unwrap();
        let get = |key: &str| {
            v.as_array()
                .unwrap()
                .iter()
                .find(|r| r["key"] == key)
                .unwrap()
                .clone()
        };
        assert_eq!(get("vm.memory_mb")["value"], 2048);
        assert_eq!(get("autostart")["value"], true);
        assert_eq!(get("ca.extra_certs")["value"], serde_json::json!([]));
        assert_eq!(get("vm.cpus")["source"], "default");
        assert!(get("vm.memory_mb").get("text").is_none());
    }

    #[test]
    fn sources_come_from_env_then_file_then_default() {
        let raw: serde_yaml::Value =
            serde_yaml::from_str("version: 1\nvm:\n  memory_mb: 6g\nca:\n  keychain: false\n")
                .unwrap();
        let effective = EffectiveConfig::default();
        temp_env::with_var("SPECK_VM_CPUS", Some("4"), || {
            let rows = rows(&effective, Some(&raw));
            let source = |key: &str| {
                rows.iter()
                    .find(|r| r.key == key)
                    .map(|r| r.source.clone())
                    .unwrap()
            };
            assert_eq!(source("vm.cpus"), "env SPECK_VM_CPUS");
            assert_eq!(source("vm.memory_mb"), "config.yaml");
            assert_eq!(source("ca.keychain"), "config.yaml");
            assert_eq!(source("vm.disk_gb"), "default");
        });
    }
}
