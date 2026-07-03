use std::path::Path;

use anyhow::Context as _;
use serde::Deserialize;

const CONFIG_FILE: &str = "config.yaml";
const CONFIG_VERSION: u64 = 1;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct AppConfig {
    #[serde(default = "default_config_version")]
    pub version: Option<u64>,
    #[serde(default)]
    pub vm: FileVmConfig,
    #[serde(default)]
    pub log_level: Option<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            version: Some(CONFIG_VERSION),
            vm: FileVmConfig::default(),
            log_level: None,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct FileVmConfig {
    #[serde(default)]
    pub cpus: Option<u64>,
    #[serde(default)]
    pub memory_mb: Option<u64>,
    #[serde(default)]
    pub disk_gb: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveConfig {
    pub vm: EffectiveVmConfig,
    pub log_level: String,
}

impl Default for EffectiveConfig {
    fn default() -> Self {
        Self {
            vm: EffectiveVmConfig::default(),
            log_level: "info".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveVmConfig {
    pub cpus: u64,
    pub memory_mb: u64,
    pub disk_gb: u64,
}

impl Default for EffectiveVmConfig {
    fn default() -> Self {
        Self {
            cpus: 2,
            memory_mb: 2048,
            disk_gb: 20,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigWarning {
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigSources {
    pub config_file: bool,
    pub cli: bool,
    pub env: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictAppConfig {
    version: u64,
    #[serde(default)]
    vm: StrictVmConfig,
    #[serde(default)]
    log_level: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictVmConfig {
    #[serde(default)]
    cpus: Option<u64>,
    #[serde(default)]
    memory_mb: Option<u64>,
    #[serde(default)]
    disk_gb: Option<u64>,
}

fn default_config_version() -> Option<u64> {
    Some(CONFIG_VERSION)
}

pub fn load_config_file(speck_home: &Path) -> anyhow::Result<(AppConfig, Vec<ConfigWarning>)> {
    let path = speck_home.join(CONFIG_FILE);
    if !path.exists() {
        return Ok((AppConfig::default(), Vec::new()));
    }

    let contents = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let value: serde_yaml::Value = serde_yaml::from_str(&contents)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    let warnings = collect_unknown_keys(&value);
    let config: AppConfig = serde_yaml::from_value(value)
        .with_context(|| format!("failed to decode {}", path.display()))?;

    anyhow::ensure!(
        config.version == Some(CONFIG_VERSION),
        "unsupported config version {:?}; expected version 1",
        config.version
    );

    Ok((config, warnings))
}

pub fn collect_unknown_keys(value: &serde_yaml::Value) -> Vec<ConfigWarning> {
    let mut warnings = Vec::new();
    let Some(top) = value.as_mapping() else {
        return warnings;
    };

    for key in top.keys().filter_map(serde_yaml::Value::as_str) {
        if !matches!(key, "version" | "vm" | "log_level") {
            warnings.push(ConfigWarning {
                path: key.to_string(),
                message: format!("unknown config key `{key}` ignored"),
            });
        }
    }

    if let Some(vm) = top
        .get(serde_yaml::Value::String("vm".into()))
        .and_then(serde_yaml::Value::as_mapping)
    {
        for key in vm.keys().filter_map(serde_yaml::Value::as_str) {
            if !matches!(key, "cpus" | "memory_mb" | "disk_gb") {
                warnings.push(ConfigWarning {
                    path: format!("vm.{key}"),
                    message: format!("unknown config key `vm.{key}` ignored"),
                });
            }
        }
    }

    warnings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_speck_home(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "speck-config-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn unknown_keys_warn_lenient_strict_rejects() {
        let home = temp_speck_home("unknown-keys");
        std::fs::write(
            home.join("config.yaml"),
            "version: 1\nvm:\n  cpus: 4\n  typo: true\nunexpected: value\n",
        )
        .unwrap();

        let (config, warnings) = load_config_file(&home).unwrap();

        assert_eq!(config.version, Some(1));
        assert_eq!(config.vm.cpus, Some(4));
        assert!(warnings.iter().any(|warning| warning.path == "vm.typo"));
        assert!(warnings.iter().any(|warning| warning.path == "unexpected"));

        let strict = serde_yaml::from_str::<StrictAppConfig>(
            "version: 1\nvm:\n  cpus: 4\n  typo: true\nunexpected: value\n",
        );
        assert!(strict.is_err(), "strict schema must reject unknown keys");
    }
}
