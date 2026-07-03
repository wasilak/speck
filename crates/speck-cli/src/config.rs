use std::path::Path;

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
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

pub fn resolve_effective_config(
    file: AppConfig,
    cli: &crate::UpArgs,
) -> anyhow::Result<EffectiveConfig> {
    let physical_cores = physical_core_count()?;
    resolve_effective_config_with_physical_cores(file, cli, physical_cores)
}

fn resolve_effective_config_with_physical_cores(
    file: AppConfig,
    cli: &crate::UpArgs,
    physical_cores: u64,
) -> anyhow::Result<EffectiveConfig> {
    let mut effective = EffectiveConfig::default();

    if let Some(cpus) = file.vm.cpus {
        effective.vm.cpus = cpus;
    }
    if let Some(memory_mb) = file.vm.memory_mb {
        effective.vm.memory_mb = memory_mb;
    }
    if let Some(disk_gb) = file.vm.disk_gb {
        effective.vm.disk_gb = disk_gb;
    }
    if let Some(log_level) = file.log_level {
        effective.log_level = log_level;
    }

    if let Some(cpus) = cli.cpus {
        effective.vm.cpus = cpus;
    }
    if let Some(memory_mb) = cli.memory {
        effective.vm.memory_mb = memory_mb;
    }
    if let Some(disk_gb) = cli.disk {
        effective.vm.disk_gb = disk_gb;
    }

    if let Some(cpus) = parse_env_u64("SPECK_VM_CPUS", "vCPU count")? {
        effective.vm.cpus = cpus;
    }
    if let Some(memory_mb) = parse_env_u64("SPECK_VM_MEMORY_MB", "MiB")? {
        effective.vm.memory_mb = memory_mb;
    }
    if let Some(disk_gb) = parse_env_u64("SPECK_VM_DISK_GB", "GiB")? {
        effective.vm.disk_gb = disk_gb;
    }
    if let Ok(log_level) = std::env::var("SPECK_LOG_LEVEL") {
        effective.log_level = log_level;
        validate_log_level("SPECK_LOG_LEVEL", &effective.log_level)?;
    } else {
        validate_log_level("log_level", &effective.log_level)?;
    }

    validate_effective_vm_config_with_physical_cores(&effective.vm, physical_cores)?;
    Ok(effective)
}

pub fn parse_env_u64(var_name: &str, unit: &str) -> anyhow::Result<Option<u64>> {
    let value = match std::env::var(var_name) {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => return Ok(None),
        Err(err) => anyhow::bail!("{var_name} must be valid UTF-8 {unit}: {err}"),
    };

    let parsed = value.parse::<u64>().with_context(|| {
        format!("{var_name} must be an unsigned integer {unit}; got `{value}`")
    })?;
    Ok(Some(parsed))
}

pub fn validate_effective_vm_config(config: &EffectiveVmConfig) -> anyhow::Result<()> {
    let physical_cores = physical_core_count()?;
    validate_effective_vm_config_with_physical_cores(config, physical_cores)
}

fn validate_effective_vm_config_with_physical_cores(
    config: &EffectiveVmConfig,
    physical_cores: u64,
) -> anyhow::Result<()> {
    anyhow::ensure!(config.cpus > 0, "vm.cpus must be at least 1");
    anyhow::ensure!(
        config.cpus <= physical_cores,
        "vm.cpus ({}) exceeds host physical core count ({})",
        config.cpus,
        physical_cores
    );
    anyhow::ensure!(
        config.memory_mb >= 512,
        "vm.memory_mb must be at least 512 MiB"
    );
    anyhow::ensure!(config.disk_gb > 0, "vm.disk_gb must be at least 1 GiB");
    Ok(())
}

pub fn physical_core_count() -> anyhow::Result<u64> {
    if let Some(count) = sysinfo::System::physical_core_count() {
        return u64::try_from(count).context("physical core count overflow");
    }

    let output = std::process::Command::new("sysctl")
        .args(["-n", "hw.physicalcpu"])
        .output()
        .context("failed to determine host physical core count with sysctl")?;
    anyhow::ensure!(
        output.status.success(),
        "sysctl hw.physicalcpu failed while determining host physical core count"
    );
    let stdout = String::from_utf8(output.stdout).context("sysctl output was not UTF-8")?;
    let count = stdout
        .trim()
        .parse::<u64>()
        .context("sysctl hw.physicalcpu did not return an integer")?;
    anyhow::ensure!(count > 0, "host physical core count must be greater than 0");
    Ok(count)
}

fn validate_log_level(source: &str, value: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !value.chars().any(char::is_whitespace),
        "{source} must be a valid tracing EnvFilter directive"
    );
    tracing_subscriber::EnvFilter::try_new(value)
        .with_context(|| format!("{source} must be a valid tracing EnvFilter directive"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

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

    #[test]
    fn precedence_env_cli_file_default() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_speck_env();
        unsafe {
            std::env::set_var("SPECK_VM_CPUS", "8");
        }
        let cli = crate::UpArgs {
            kernel: None,
            initrd: None,
            rootfs: None,
            data_disk: None,
            cpus: Some(2),
            memory: None,
            disk: None,
        };
        let file = AppConfig {
            vm: FileVmConfig {
                cpus: Some(4),
                ..Default::default()
            },
            ..Default::default()
        };

        let effective = resolve_effective_config_with_physical_cores(file, &cli, 16).unwrap();

        assert_eq!(effective.vm.cpus, 8);
        assert_eq!(effective.vm.memory_mb, 2048);
        assert_eq!(effective.vm.disk_gb, 20);
        clear_speck_env();
    }

    #[test]
    fn env_overrides_all_config_fields() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_speck_env();
        unsafe {
            std::env::set_var("SPECK_VM_CPUS", "6");
            std::env::set_var("SPECK_VM_MEMORY_MB", "4096");
            std::env::set_var("SPECK_VM_DISK_GB", "40");
            std::env::set_var("SPECK_LOG_LEVEL", "debug");
        }
        let cli = crate::UpArgs {
            kernel: None,
            initrd: None,
            rootfs: None,
            data_disk: None,
            cpus: Some(2),
            memory: Some(2048),
            disk: Some(20),
        };
        let file = AppConfig {
            vm: FileVmConfig {
                cpus: Some(4),
                memory_mb: Some(3072),
                disk_gb: Some(30),
            },
            log_level: Some("info".into()),
            ..Default::default()
        };

        let effective = resolve_effective_config_with_physical_cores(file, &cli, 16).unwrap();

        assert_eq!(effective.vm.cpus, 6);
        assert_eq!(effective.vm.memory_mb, 4096);
        assert_eq!(effective.vm.disk_gb, 40);
        assert_eq!(effective.log_level, "debug");
        clear_speck_env();
    }

    #[test]
    fn invalid_env_value_names_variable_and_unit() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_speck_env();
        unsafe {
            std::env::set_var("SPECK_VM_MEMORY_MB", "abc");
        }
        let cli = empty_cli_args();

        let err = resolve_effective_config_with_physical_cores(AppConfig::default(), &cli, 16)
            .unwrap_err()
            .to_string();

        assert!(err.contains("SPECK_VM_MEMORY_MB"));
        assert!(err.contains("MiB"));
        clear_speck_env();
    }

    #[test]
    fn invalid_log_level_names_env_var() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_speck_env();
        unsafe {
            std::env::set_var("SPECK_LOG_LEVEL", "not a valid filter");
        }
        let cli = empty_cli_args();

        let err = resolve_effective_config_with_physical_cores(AppConfig::default(), &cli, 16)
            .unwrap_err()
            .to_string();

        assert!(err.contains("SPECK_LOG_LEVEL"));
        clear_speck_env();
    }

    #[test]
    fn rejects_cpu_count_above_physical_cores() {
        let config = EffectiveVmConfig {
            cpus: 9,
            memory_mb: 2048,
            disk_gb: 20,
        };

        let err = validate_effective_vm_config_with_physical_cores(&config, 8)
            .unwrap_err()
            .to_string();

        assert!(err.contains("physical core"));
    }

    #[test]
    fn rejects_memory_below_512_mib() {
        let config = EffectiveVmConfig {
            cpus: 2,
            memory_mb: 511,
            disk_gb: 20,
        };

        let err = validate_effective_vm_config_with_physical_cores(&config, 8)
            .unwrap_err()
            .to_string();

        assert!(err.contains("512 MiB"));
    }

    fn empty_cli_args() -> crate::UpArgs {
        crate::UpArgs {
            kernel: None,
            initrd: None,
            rootfs: None,
            data_disk: None,
            cpus: None,
            memory: None,
            disk: None,
        }
    }

    fn clear_speck_env() {
        unsafe {
            std::env::remove_var("SPECK_VM_CPUS");
            std::env::remove_var("SPECK_VM_MEMORY_MB");
            std::env::remove_var("SPECK_VM_DISK_GB");
            std::env::remove_var("SPECK_LOG_LEVEL");
        }
    }
}
