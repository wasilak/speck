use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

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
    #[serde(default)]
    pub ca: CaConfig,
    /// `false`: run without a launchd agent, so the instance does not come
    /// back after a reboot (CI profiles). `None` means the default: on.
    #[serde(default)]
    pub autostart: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct CaConfig {
    #[serde(default)]
    pub extra_certs: Vec<String>,
    /// Opt-in: bind the guest CA bundle into every new container (TLS-03).
    #[serde(default)]
    pub inject_into_containers: bool,
    /// Export admin-trusted Keychain certificates into the guest (TLS-02).
    /// `None` means the default: on.
    #[serde(default)]
    pub keychain: Option<bool>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            version: Some(CONFIG_VERSION),
            vm: FileVmConfig::default(),
            log_level: None,
            ca: CaConfig::default(),
            autostart: None,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct FileVmConfig {
    #[serde(default)]
    pub cpus: Option<u64>,
    #[serde(default, deserialize_with = "crate::size::serde_size::mib")]
    pub memory_mb: Option<u64>,
    #[serde(default, deserialize_with = "crate::size::serde_size::gib")]
    pub disk_gb: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveConfig {
    pub vm: EffectiveVmConfig,
    pub log_level: String,
    pub extra_certs: Vec<String>,
    pub inject_ca_into_containers: bool,
    pub ca_from_keychain: bool,
    pub autostart: bool,
}

impl Default for EffectiveConfig {
    fn default() -> Self {
        Self {
            vm: EffectiveVmConfig::default(),
            log_level: "info".into(),
            extra_certs: Vec::new(),
            inject_ca_into_containers: false,
            ca_from_keychain: true,
            autostart: true,
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
            disk_gb: 100,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigWarning {
    pub path: String,
    pub message: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigSources {
    pub config_file: bool,
    pub cli: bool,
    pub env: bool,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictAppConfig {
    version: u64,
    #[serde(default)]
    vm: StrictVmConfig,
    #[serde(default)]
    log_level: Option<String>,
    #[serde(default)]
    ca: CaConfig,
    #[serde(default)]
    autostart: Option<bool>,
}

#[allow(dead_code)]
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictVmConfig {
    #[serde(default)]
    cpus: Option<u64>,
    #[serde(default, deserialize_with = "crate::size::serde_size::mib")]
    memory_mb: Option<u64>,
    #[serde(default, deserialize_with = "crate::size::serde_size::gib")]
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

/// Save `autostart: <value>` into config.yaml, keeping the other keys.
pub fn persist_autostart(speck_home: &Path, value: bool) -> anyhow::Result<()> {
    let path = speck_home.join(CONFIG_FILE);
    let mut root = match std::fs::read_to_string(&path) {
        Ok(contents) => serde_yaml::from_str(&contents)
            .with_context(|| format!("failed to parse {}", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_yaml::Value::Null,
        Err(e) => return Err(e).with_context(|| format!("failed to read {}", path.display())),
    };
    if !root.is_mapping() {
        let mut fresh = serde_yaml::Mapping::new();
        fresh.insert("version".into(), CONFIG_VERSION.into());
        root = fresh.into();
    }
    root.as_mapping_mut()
        .expect("mapping")
        .insert("autostart".into(), value.into());
    std::fs::write(&path, serde_yaml::to_string(&root)?)
        .with_context(|| format!("failed to write {}", path.display()))
}

/// Save `spk up --cpus/--memory/--disk` into config.yaml, like `colima start`
/// does. The launchd daemon reads only the config file, so without this the
/// flags would be ignored. Returns the `vm.<key>=<value>` pairs written.
pub fn persist_vm_overrides(speck_home: &Path, cli: &crate::UpArgs) -> anyhow::Result<Vec<String>> {
    let overrides = [
        ("cpus", cli.cpus),
        ("memory_mb", cli.memory),
        ("disk_gb", cli.disk),
    ];
    if overrides.iter().all(|(_, v)| v.is_none()) {
        return Ok(Vec::new());
    }
    let (file_config, _) = load_config_file(speck_home)?;
    resolve_effective_config(file_config, cli)?;

    let path = speck_home.join(CONFIG_FILE);
    let mut root = match std::fs::read_to_string(&path) {
        Ok(contents) => serde_yaml::from_str(&contents)
            .with_context(|| format!("failed to parse {}", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_yaml::Value::Null,
        Err(e) => return Err(e).with_context(|| format!("failed to read {}", path.display())),
    };
    if !root.is_mapping() {
        let mut fresh = serde_yaml::Mapping::new();
        fresh.insert("version".into(), CONFIG_VERSION.into());
        root = fresh.into();
    }
    let top = root.as_mapping_mut().expect("mapping");
    let vm = top
        .entry("vm".into())
        .or_insert_with(|| serde_yaml::Mapping::new().into());
    if !vm.is_mapping() {
        *vm = serde_yaml::Mapping::new().into();
    }
    let vm = vm.as_mapping_mut().expect("mapping");
    let mut written = Vec::new();
    for (key, value) in overrides {
        if let Some(value) = value {
            vm.insert(key.into(), value.into());
            written.push(format!("vm.{key}={value}"));
        }
    }
    std::fs::write(&path, serde_yaml::to_string(&root)?)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(written)
}

pub fn collect_unknown_keys(value: &serde_yaml::Value) -> Vec<ConfigWarning> {
    let mut warnings = Vec::new();
    let Some(top) = value.as_mapping() else {
        return warnings;
    };

    for key in top.keys().filter_map(serde_yaml::Value::as_str) {
        if !matches!(key, "version" | "vm" | "log_level" | "ca" | "autostart") {
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

    if let Some(ca) = top
        .get(serde_yaml::Value::String("ca".into()))
        .and_then(serde_yaml::Value::as_mapping)
    {
        for key in ca.keys().filter_map(serde_yaml::Value::as_str) {
            if !matches!(key, "extra_certs" | "inject_into_containers" | "keychain") {
                warnings.push(ConfigWarning {
                    path: format!("ca.{key}"),
                    message: format!("unknown config key `ca.{key}` ignored"),
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
    effective.extra_certs = file.ca.extra_certs;
    effective.inject_ca_into_containers = file.ca.inject_into_containers;
    effective.ca_from_keychain = file.ca.keychain.unwrap_or(true);
    effective.autostart = file.autostart.unwrap_or(true);

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
    if let Some(memory_mb) = parse_env_size("SPECK_VM_MEMORY_MB", crate::size::parse_mib)? {
        effective.vm.memory_mb = memory_mb;
    }
    if let Some(disk_gb) = parse_env_size("SPECK_VM_DISK_GB", crate::size::parse_gib)? {
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

/// A size env var (`6g`, `6144`); see `crate::size`.
fn parse_env_size(
    var_name: &str,
    parse: fn(&str) -> Result<u64, String>,
) -> anyhow::Result<Option<u64>> {
    match std::env::var(var_name) {
        Ok(value) => parse(&value)
            .map(Some)
            .map_err(|e| anyhow::anyhow!("{var_name}: {e}")),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(err) => anyhow::bail!("{var_name} must be valid UTF-8: {err}"),
    }
}

pub fn parse_env_u64(var_name: &str, unit: &str) -> anyhow::Result<Option<u64>> {
    let value = match std::env::var(var_name) {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => return Ok(None),
        Err(err) => anyhow::bail!("{var_name} must be valid UTF-8 {unit}: {err}"),
    };

    let parsed = value
        .parse::<u64>()
        .with_context(|| format!("{var_name} must be an unsigned integer {unit}; got `{value}`"))?;
    Ok(Some(parsed))
}

#[allow(dead_code)]
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

/// Validate CA certificate PEM files and copy them to `{speck_home}/ca-certs/{sha256}.pem`,
/// together with `keychain_pems` (TLS-02).
///
/// The directory is synced: `.pem` files from earlier runs that are not in the
/// current set are removed, because the whole directory is shared with the
/// guest and a CA dropped from the config must stop being trusted.
///
/// Returns `Ok(vec![])` when there is nothing to install.
/// Returns `Err` on first failed validation (missing file or invalid PEM).
/// Returns `Ok(Vec<PathBuf>)` listing the cert files on success.
///
/// Validation fails fast at config-load time, before the VM starts (D-12-04).
pub fn validate_and_prepare_ca_certs(
    speck_home: &Path,
    extra_certs: &[String],
    keychain_pems: &[String],
) -> anyhow::Result<Vec<PathBuf>> {
    let ca_certs_dir = speck_home.join("ca-certs");
    if extra_certs.is_empty() && keychain_pems.is_empty() {
        remove_stale_certs(&ca_certs_dir, &HashSet::new());
        return Ok(Vec::new());
    }

    std::fs::create_dir_all(&ca_certs_dir).with_context(|| {
        format!(
            "failed to create ca-certs directory at {}",
            ca_certs_dir.display()
        )
    })?;

    let mut copied_paths = Vec::new();
    let mut seen_hashes = HashSet::new();

    for path_str in extra_certs {
        let path = Path::new(path_str);

        let content = std::fs::read(path).with_context(|| format!("file not found: {path_str}"))?;

        // Validate PEM structure (must have proper BEGIN / END markers).
        // We avoid the full `pem` crate parse here because its base64 validation
        // rejects some commonly-distributed PEM files. The critical check for
        // correctness is that the file looks like PEM — the guest system's CA
        // store will do the full cryptographic verification at boot.
        let pem_text =
            std::str::from_utf8(&content).with_context(|| format!("not valid PEM: {path_str}"))?;
        let trimmed = pem_text.trim();
        if !trimmed.starts_with("-----BEGIN ") || !trimmed.ends_with("-----") {
            anyhow::bail!("not valid PEM: {path_str}");
        }

        let hash = Sha256::digest(&content);
        let hash_hex = format!("{hash:x}");

        if seen_hashes.insert(hash_hex.clone()) {
            let dest_path = ca_certs_dir.join(format!("{hash_hex}.pem"));
            if !dest_path.exists() {
                std::fs::copy(path, &dest_path)
                    .with_context(|| format!("failed to copy cert to {}", dest_path.display()))?;
            }
            copied_paths.push(dest_path);
        }
    }

    for pem in keychain_pems {
        let hash_hex = format!("{:x}", Sha256::digest(pem.as_bytes()));
        if seen_hashes.insert(hash_hex.clone()) {
            let dest_path = ca_certs_dir.join(format!("{hash_hex}.pem"));
            if !dest_path.exists() {
                std::fs::write(&dest_path, pem).with_context(|| {
                    format!("failed to write keychain cert to {}", dest_path.display())
                })?;
            }
            copied_paths.push(dest_path);
        }
    }

    remove_stale_certs(&ca_certs_dir, &seen_hashes);
    Ok(copied_paths)
}

/// Delete `{hash}.pem` files in `dir` whose hash is not in `keep`.
fn remove_stale_certs(dir: &Path, keep: &HashSet<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let stale = path.extension().and_then(|e| e.to_str()) == Some("pem")
            && path
                .file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|stem| !keep.contains(stem));
        if stale {
            let _ = std::fs::remove_file(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_ca_certs_syncs_dir_and_adds_keychain_pems() {
        let dir = temp_speck_home("ca-sync");
        let ca_dir = dir.join("ca-certs");
        std::fs::create_dir_all(&ca_dir).unwrap();
        std::fs::write(ca_dir.join("deadbeef.pem"), "stale").unwrap();
        std::fs::write(ca_dir.join("notes.txt"), "kept").unwrap();

        let pem = "-----BEGIN CERTIFICATE-----\nMIIK\n-----END CERTIFICATE-----\n".to_string();
        let paths = validate_and_prepare_ca_certs(&dir, &[], &[pem.clone(), pem]).unwrap();

        assert_eq!(paths.len(), 1); // deduplicated
        assert!(paths[0].is_file());
        assert!(!ca_dir.join("deadbeef.pem").exists());
        assert!(ca_dir.join("notes.txt").exists());

        // Nothing configured any more: previously installed certs go away.
        assert!(
            validate_and_prepare_ca_certs(&dir, &[], &[])
                .unwrap()
                .is_empty()
        );
        assert!(!paths[0].exists());
    }

    fn temp_speck_home(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("speck-config-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // --- Environment-safety guard ---

    #[test]
    fn no_unsafe_env_mutation() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/config.rs");
        let src = std::fs::read_to_string(path).unwrap();
        let test_section = src.find("#[cfg(test)]").unwrap_or(0);
        let test_code = &src[test_section..];

        for op in ["set_var", "remove_var"] {
            let needle = format!("{}::{}", "std::env", op);
            assert!(
                !test_code.contains(&needle),
                "test code must use temp_env::with_var(s), not raw unsafe {}::{}",
                "std::env",
                op,
            );
        }
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
        temp_env::with_var("SPECK_VM_CPUS", Some("8"), || {
            let cli = crate::UpArgs {
                kernel: None,
                initrd: None,
                rootfs: None,
                data_disk: None,
                cpus: Some(2),
                memory: None,
                disk: None,
                foreground: false,
                pull: false,
                wait: false,
                timeout: 300,
                no_autostart: false,
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
            assert_eq!(effective.vm.disk_gb, 100);
        });
    }

    #[test]
    fn env_overrides_all_config_fields() {
        temp_env::with_vars(
            [
                ("SPECK_VM_CPUS", Some("6")),
                ("SPECK_VM_MEMORY_MB", Some("4096")),
                ("SPECK_VM_DISK_GB", Some("40")),
                ("SPECK_LOG_LEVEL", Some("debug")),
            ],
            || {
                let cli = crate::UpArgs {
                    kernel: None,
                    initrd: None,
                    rootfs: None,
                    data_disk: None,
                    cpus: Some(2),
                    memory: Some(2048),
                    disk: Some(20),
                    foreground: false,
                    pull: false,
                    wait: false,
                    timeout: 300,
                    no_autostart: false,
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

                let effective =
                    resolve_effective_config_with_physical_cores(file, &cli, 16).unwrap();

                assert_eq!(effective.vm.cpus, 6);
                assert_eq!(effective.vm.memory_mb, 4096);
                assert_eq!(effective.vm.disk_gb, 40);
                assert_eq!(effective.log_level, "debug");
            },
        );
    }

    #[test]
    fn invalid_env_value_names_variable_and_unit() {
        temp_env::with_var("SPECK_VM_MEMORY_MB", Some("abc"), || {
            let cli = empty_cli_args();

            let err = resolve_effective_config_with_physical_cores(AppConfig::default(), &cli, 16)
                .unwrap_err()
                .to_string();

            assert!(err.contains("SPECK_VM_MEMORY_MB"));
            assert!(
                err.contains("6144m"),
                "error must show a valid example: {err}"
            );
        });
    }

    #[test]
    fn sizes_work_in_env_and_config_file() {
        temp_env::with_vars(
            [
                ("SPECK_VM_MEMORY_MB", Some("6G")),
                ("SPECK_VM_DISK_GB", Some("1t")),
            ],
            || {
                let effective = resolve_effective_config_with_physical_cores(
                    AppConfig::default(),
                    &empty_cli_args(),
                    16,
                )
                .unwrap();
                assert_eq!(effective.vm.memory_mb, 6144);
                assert_eq!(effective.vm.disk_gb, 1024);
            },
        );
        let home = temp_speck_home("sizes");
        std::fs::write(
            home.join(CONFIG_FILE),
            "version: 1\nvm:\n  memory_mb: 6g\n  disk_gb: 120\n",
        )
        .unwrap();
        let (config, warnings) = load_config_file(&home).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(config.vm.memory_mb, Some(6144));
        assert_eq!(config.vm.disk_gb, Some(120));
        std::fs::write(
            home.join(CONFIG_FILE),
            "version: 1\nvm:\n  memory_mb: lots\n",
        )
        .unwrap();
        assert!(load_config_file(&home).is_err());
    }

    #[test]
    fn invalid_log_level_names_env_var() {
        temp_env::with_var("SPECK_LOG_LEVEL", Some("not a valid filter"), || {
            let cli = empty_cli_args();

            let err = resolve_effective_config_with_physical_cores(AppConfig::default(), &cli, 16)
                .unwrap_err()
                .to_string();

            assert!(err.contains("SPECK_LOG_LEVEL"));
        });
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

    #[test]
    fn persist_vm_overrides_writes_flags_and_keeps_other_keys() {
        without_vm_env(|| {
            let home = temp_speck_home("persist");
            let mut cli = empty_cli_args();
            assert!(persist_vm_overrides(&home, &cli).unwrap().is_empty());
            assert!(!home.join(CONFIG_FILE).exists());

            std::fs::write(
                home.join(CONFIG_FILE),
                "version: 1\nvm:\n  cpus: 2\nca:\n  keychain: false\n",
            )
            .unwrap();
            cli.memory = Some(8192);
            assert_eq!(
                persist_vm_overrides(&home, &cli).unwrap(),
                vec!["vm.memory_mb=8192"]
            );
            let (config, warnings) = load_config_file(&home).unwrap();
            assert!(warnings.is_empty());
            assert_eq!(config.vm.memory_mb, Some(8192));
            assert_eq!(config.vm.cpus, Some(2));
            assert_eq!(config.ca.keychain, Some(false));
        });
    }

    #[test]
    fn persist_vm_overrides_creates_config_and_rejects_invalid_values() {
        without_vm_env(|| {
            let home = temp_speck_home("persist-new");
            let mut cli = empty_cli_args();
            cli.memory = Some(256);
            assert!(persist_vm_overrides(&home, &cli).is_err());
            assert!(!home.join(CONFIG_FILE).exists());

            cli.memory = Some(4096);
            persist_vm_overrides(&home, &cli).unwrap();
            let (config, _) = load_config_file(&home).unwrap();
            assert_eq!(config.vm.memory_mb, Some(4096));
        });
    }

    /// Env overrides from parallel tests must not leak into resolution.
    fn without_vm_env(f: impl FnOnce()) {
        temp_env::with_vars(
            [
                ("SPECK_VM_CPUS", None::<&str>),
                ("SPECK_VM_MEMORY_MB", None),
                ("SPECK_VM_DISK_GB", None),
                ("SPECK_LOG_LEVEL", None),
            ],
            f,
        );
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
            foreground: false,
            pull: false,
            wait: false,
            timeout: 300,
            no_autostart: false,
        }
    }

    // ── CA cert validation tests (Phase 12 — RED; function does not exist yet) ──

    fn ca_test_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("speck-ca-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_pem(path: &std::path::Path, content: &[u8]) {
        std::fs::write(path, content).unwrap();
    }

    const VALID_PEM: &[u8] = b"-----BEGIN CERTIFICATE-----\nMIIDXTCCAkWgAwIBAgIJAKlSvOqYQm0lMA0GCSqGSIb3DQEBCwUAMEUxCzAJBgNV\nBAYTAlVTMRMwEQYDVQQIDApDYWxpZm9ybmlhMQswCQYDVQQHDAJTRjEUMBIGA1UE\nCgwLRXhhbXBsZSBDQTAeFw0yNTAxMDEwMDAwMDBaFw0zNTAxMDEwMDAwMDBaMEUx\nCzAJBgNVBAYTAlVTMRMwEQYDVQQIDApDYWxpZm9ybmlhMQswCQYDVQQHDAJTRjEU\nMBIGA1UECgwLRXhhbXBsZSBDQTCCASIwDQYJKoZIhvcNAQEBBQADggEPADCCAQoC\nggEBAK0A/2J/LPU2n9Y/z8XzqM/GnJqFk5yYtY6NUKQjMHgqJp1sL7tgGKz6bPqA\n1h0u5Yb4dLlQwAIBRAc4hYkGnQL+A2J2gVBX/sa6O7BYKeQ8wO1tBmUj2o3lQmYB\nvJ0X7CAwIDAQABo4GJMIGGMAkGA1UdEwQCMAAwHQYDVR0OBBYEFKCFp8L0S0pU\n7UjKq6L5n0x3WvTAMAkGA1UdEwQCMAAwCwYDVR0PBAQDAgEGMA8GA1UdEwEB/wQF\nMAMBAf8wHQYDVR0OBBYEFKCFp8L0S0pU7UjKq6L5n0x3WvTAMBgNVHRIEATAHMAUG\nA1UdIwEB/zAFBgNVHSQBAf8wDQYJKoZIhvcNAQELBQADggEBAGVPQ3VpRL0K3VGR\nLm0YH1Zz7n8cL0Jp6L5n0x3WvTAMBgNVHRIEATAHMAUGA1UdIwEB/zAFBgNVHSQ=\n-----END CERTIFICATE-----\n";

    const OTHER_VALID_PEM: &[u8] = b"-----BEGIN CERTIFICATE-----\nMIIDXjCCAkWgAwIBAgIJAKlSvOqYQm0mMA0GCSqGSIb3DQEBCwUAMEUxCzAJBgNV\nBAYTAlVTMRMwEQYDVQQIDApDYWxpZm9ybmlhMQswCQYDVQQHDAJTRjEUMBIGA1UE\nCgwLRXhhbXBsZSBDQTAeFw0yNTAxMDEwMDAwMDBaFw0zNTAxMDEwMDAwMDBaMEUx\nCzAJBgNVBAYTAlVTMRMwEQYDVQQIDApDYWxpZm9ybmlhMQswCQYDVQQHDAJTRjEU\nMBIGA1UECgwLRXhhbXBsZSBDQTCCASIwDQYJKoZIhvcNAQEBBQADggEPADCCAQoC\nggEBAK0A/2J/LPU2n9Y/z8XzqM/GnJqFk5yYtY6NUKQjMHgqJp1sL7tgGKz6bPqA\n1h0u5Yb4dLlQwAIBRAc4hYkGnQL+A2J2gVBX/sa6O7BYKeQ8wO1tBmUj2o3lQmYB\nvJ0X7CAwIDAQABo4GJMIGGMAkGA1UdEwQCMAAwHQYDVR0OBBYEFKCFp8L0S0pU\n7UjKq6L5n0x3WvTAMAkGA1UdEwQCMAAwCwYDVR0PBAQDAgEGMA8GA1UdEwEB/wQF\nMAMBAf8wHQYDVR0OBBYEFKCFp8L0S0pU7UjKq6L5n0x3WvTAMBgNVHRIEATAHMAUG\nA1UdIwEB/zAFBgNVHSQBAf8wDQYJKoZIhvcNAQELBQADggEBAGVPQ3VpRL0K3VGR\nLm0YH1Zz7n8cL0Jp6L5n0x3WvTAMBgNVHRIEATAHMAUGA1UdIwEB/zAFBgNVHSQ=\n-----END CERTIFICATE-----\n";

    const NOT_PEM: &[u8] = b"not a certificate";

    #[test]
    fn empty_cert_list_returns_ok_empty_vec() {
        let dir = ca_test_dir("empty-list");
        let result = validate_and_prepare_ca_certs(&dir, &[], &[]);
        assert!(result.is_ok(), "empty list should succeed, got: {result:?}");
        let paths = result.unwrap();
        assert!(paths.is_empty(), "empty list should yield no output paths");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_returns_err_file_not_found() {
        let dir = ca_test_dir("missing-file");
        let paths = vec![String::from("/nonexistent/ca.pem")];
        let result = validate_and_prepare_ca_certs(&dir, &paths, &[]);
        assert!(result.is_err(), "missing file should fail");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("not found"),
            "error should mention 'not found', got: {err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn non_pem_content_returns_err_not_valid_pem() {
        let dir = ca_test_dir("bad-pem");
        let bad_file = dir.join("bad.pem");
        write_pem(&bad_file, NOT_PEM);
        let paths = vec![bad_file.to_string_lossy().to_string()];
        let result = validate_and_prepare_ca_certs(&dir, &paths, &[]);
        assert!(result.is_err(), "non-PEM should fail");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("not valid PEM") || err.contains("PEM"),
            "error should mention PEM validation, got: {err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn valid_pem_returns_ok_with_dedup_filename() {
        let dir = ca_test_dir("valid-pem");
        let cert_file = dir.join("my-cert.pem");
        write_pem(&cert_file, VALID_PEM);
        let paths = vec![cert_file.to_string_lossy().to_string()];
        let result = validate_and_prepare_ca_certs(&dir, &paths, &[]);
        assert!(result.is_ok(), "valid PEM should succeed, got: {result:?}");
        let output_paths = result.unwrap();
        assert!(!output_paths.is_empty(), "should return at least one path");
        // Each output path should be under the ca-certs/ subdirectory
        for p in &output_paths {
            let rel = p.strip_prefix(&dir).unwrap();
            assert!(
                rel.starts_with("ca-certs/") || rel.starts_with("ca-certs"),
                "output path {p:?} should be under ca-certs/ dir, relative: {rel:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_identical_certs_deduplicate() {
        let dir = ca_test_dir("dedup");
        let cert_a = dir.join("a.pem");
        let cert_b = dir.join("b.pem");
        write_pem(&cert_a, VALID_PEM);
        write_pem(&cert_b, VALID_PEM); // same content
        let paths = vec![
            cert_a.to_string_lossy().to_string(),
            cert_b.to_string_lossy().to_string(),
        ];
        let result = validate_and_prepare_ca_certs(&dir, &paths, &[]);
        assert!(result.is_ok(), "dedup test should succeed");
        let output_paths = result.unwrap();
        // Two identical certs should produce one output file (same SHA256)
        assert_eq!(
            output_paths.len(),
            1,
            "two identical certs should deduplicate to one output, got {}",
            output_paths.len()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn output_paths_under_ca_certs_dir() {
        let dir = ca_test_dir("output-path");
        let cert_a = dir.join("root.pem");
        write_pem(&cert_a, VALID_PEM);
        let cert_b = dir.join("intermediate.pem");
        write_pem(&cert_b, OTHER_VALID_PEM);
        let paths = vec![
            cert_a.to_string_lossy().to_string(),
            cert_b.to_string_lossy().to_string(),
        ];
        let result = validate_and_prepare_ca_certs(&dir, &paths, &[]);
        assert!(result.is_ok(), "output path test should succeed");
        let output_paths = result.unwrap();
        assert!(!output_paths.is_empty(), "should have output paths");
        let ca_certs_dir = dir.join("ca-certs");
        for p in &output_paths {
            assert!(
                p.starts_with(&ca_certs_dir),
                "path {p:?} should be under {ca_certs_dir:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
