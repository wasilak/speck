//! `spk config`: validate config.yaml and show the effective settings with
//! where each value comes from (default, config.yaml or an env variable).

use std::path::Path;

use clap::Parser as _;

use crate::config::{self, EffectiveConfig, EffectiveVmConfig};

struct Row {
    key: &'static str,
    value: String,
    source: String,
}

/// Exit code 0 when the configuration is valid, 1 otherwise.
pub fn run_config(speck_home: &Path) -> anyhow::Result<i32> {
    let path = speck_home.join("config.yaml");
    println!("Config: {}", path.display());

    let raw: Option<serde_yaml::Value> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_yaml::from_str(&text).ok());
    let effective = config::load_config_file(speck_home).and_then(|(file, warnings)| {
        for warning in &warnings {
            println!("  warning: {}", warning.message);
        }
        let no_flags = crate::UpArgs::try_parse_from(["up"])?;
        config::resolve_effective_config(file, &no_flags)
    });
    let effective = match effective {
        Ok(effective) => effective,
        Err(e) => {
            println!("  invalid: {e:#}");
            return Ok(1);
        }
    };
    let missing: Vec<&String> = effective
        .extra_certs
        .iter()
        .filter(|cert| !Path::new(cert).is_file())
        .collect();
    for cert in &missing {
        println!("  invalid: ca.extra_certs: {cert} does not exist");
    }
    if !missing.is_empty() {
        return Ok(1);
    }
    println!(
        "  {}\n",
        if path.exists() {
            "valid"
        } else {
            "not found; using defaults"
        }
    );

    let rows = rows(&effective, raw.as_ref());
    let width = rows.iter().map(|r| r.key.len()).max().unwrap_or(0);
    let value_width = rows.iter().map(|r| r.value.len()).max().unwrap_or(0);
    for row in &rows {
        println!(
            "{:width$}  {:value_width$}  {}",
            row.key, row.value, row.source
        );
    }

    if let Some(running) = running_vm(speck_home)
        && running != effective.vm
    {
        println!(
            "\nThe running VM uses {} CPUs, {} MiB memory, {} GiB disk; \
             run `spk restart` to apply the settings above.",
            running.cpus, running.memory_mb, running.disk_gb
        );
    }
    Ok(0)
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
            value: vm.cpus.to_string(),
            source: source("vm.cpus", Some("SPECK_VM_CPUS")),
        },
        Row {
            key: "vm.memory_mb",
            value: format!("{} ({})", vm.memory_mb, human_mib(vm.memory_mb)),
            source: source("vm.memory_mb", Some("SPECK_VM_MEMORY_MB")),
        },
        Row {
            key: "vm.disk_gb",
            value: vm.disk_gb.to_string(),
            source: source("vm.disk_gb", Some("SPECK_VM_DISK_GB")),
        },
        Row {
            key: "log_level",
            value: effective.log_level.clone(),
            source: source("log_level", Some("SPECK_LOG_LEVEL")),
        },
        Row {
            key: "ca.keychain",
            value: effective.ca_from_keychain.to_string(),
            source: source("ca.keychain", None),
        },
        Row {
            key: "ca.inject_into_containers",
            value: effective.inject_ca_into_containers.to_string(),
            source: source("ca.inject_into_containers", None),
        },
        Row {
            key: "ca.extra_certs",
            value: certs,
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
