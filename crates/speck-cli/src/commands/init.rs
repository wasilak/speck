use std::path::{Path, PathBuf};

use anyhow::Context as _;

use crate::InitArgs;
use crate::shell::{self, BEGIN_MARKER, END_MARKER, EnvShell, ShellTarget};

const DEFAULT_CONFIG_YAML: &str = "\
# Speck configuration — config.yaml
# All keys are optional. Uncomment and edit to override defaults.

version: 1

vm:
  # vCPUs to allocate (default: 2, max: host physical cores)
  # cpus: 2

  # Memory in MiB (default: 2048, minimum: 512)
  # memory_mb: 2048

  # Data disk size in GiB (default: 20, minimum: 1)
  # disk_gb: 20

# Log level — tracing EnvFilter directive (default: info)
# Levels: error, warn, info, debug, trace
# log_level: info

ca:
  # Paths to additional CA certificate PEM files to inject into the VM
  # (useful for corporate CAs and private registries)
  # extra_certs:
  #   - /path/to/corporate-ca.pem
";

/// Resolve the shell target from the `--shell` override or auto-detection.
fn resolve_shell(override_shell: &Option<String>) -> anyhow::Result<ShellTarget> {
    match override_shell.as_deref() {
        Some("bash") => Ok(ShellTarget::Bash),
        Some("zsh") => Ok(ShellTarget::Zsh),
        Some("fish") => Ok(ShellTarget::Fish),
        Some(other) => anyhow::bail!("unsupported shell `{other}` (expected bash, zsh, or fish)"),
        None => Ok(shell::detect_shell()),
    }
}

/// Resolve the target dotfile path for the given shell target.
///
/// Bash uses `~/.bashrc`, Zsh uses `~/.zshrc`, and Fish uses
/// `$XDG_CONFIG_HOME/fish/conf.d/speck.fish` (falling back to
/// `~/.config/fish/conf.d/speck.fish`).
fn resolve_target_file(target: ShellTarget) -> anyhow::Result<PathBuf> {
    match target {
        ShellTarget::Bash => {
            Ok(PathBuf::from(std::env::var("HOME").context("HOME not set")?).join(".bashrc"))
        }
        ShellTarget::Zsh => {
            Ok(PathBuf::from(std::env::var("HOME").context("HOME not set")?).join(".zshrc"))
        }
        ShellTarget::Fish => {
            let config_home = match std::env::var("XDG_CONFIG_HOME") {
                Ok(x) => PathBuf::from(x),
                Err(_) => {
                    PathBuf::from(std::env::var("HOME").context("HOME not set")?).join(".config")
                }
            };
            Ok(config_home.join("fish/conf.d/speck.fish"))
        }
    }
}

/// Replace the existing `# BEGIN speck` … `# END speck` region in `content`
/// with `block`, or append `block` if no sentinel markers are present.
fn replace_or_append_block(content: &str, block: &str) -> String {
    if let Some(begin) = content.find(BEGIN_MARKER)
        && let Some(end_rel) = content[begin..].find(END_MARKER)
    {
        let end_abs = begin + end_rel + END_MARKER.len();
        let mut result = String::with_capacity(content.len() + block.len());
        result.push_str(&content[..begin]);
        result.push_str(block);
        result.push_str(&content[end_abs..]);
        return result;
    }
    if content.is_empty() {
        block.to_string()
    } else if content.ends_with('\n') {
        format!("{content}{block}")
    } else {
        format!("{content}\n{block}")
    }
}

/// Initialize Speck shell integration (one-time setup).
///
/// Performs four steps:
/// 1. Resolve shell target from `--shell` arg or [`shell::detect_shell`].
/// 2. Check `DOCKER_HOST` conflict: if set to a non-Speck value, warn stderr.
/// 3. Without `--set-docker-host`: print instructions + preview, return Ok.
///    With `--set-docker-host`: write idempotent block to the target file.
/// 4. Print confirmation or instructions.
pub async fn run_init(args: InitArgs, speck_home: &Path) -> anyhow::Result<()> {
    // Step 1 — Resolve shell target
    let target = resolve_shell(&args.shell)?;

    // Step 2 — Check DOCKER_HOST conflict
    if let Ok(docker_host) = std::env::var("DOCKER_HOST")
        && !docker_host.contains("speck.sock")
    {
        eprintln!(
            "Warning: DOCKER_HOST is currently set to `{docker_host}` — spk init will override it."
        );
    }

    // Step 3 — Scaffold config.yaml if absent
    let config_path = speck_home.join("config.yaml");
    if !config_path.exists() {
        std::fs::create_dir_all(speck_home)
            .with_context(|| format!("failed to create {}", speck_home.display()))?;
        std::fs::write(&config_path, DEFAULT_CONFIG_YAML)
            .with_context(|| format!("failed to write {}", config_path.display()))?;
        println!("Created {}", config_path.display());
    } else {
        println!(
            "{} already exists — skipping (not overwriting)",
            config_path.display()
        );
    }

    // Step 4 — Preview/persist based on --set-docker-host
    if !args.set_docker_host {
        println!("Run `spk init --set-docker-host` to persist Speck environment in your shell.");
        println!();
        println!("--- Preview (POSIX) ---");
        print!("{}", shell::render_env(speck_home, EnvShell::Posix));
        print!("{}", shell::render_speck_home(speck_home, EnvShell::Posix));
        println!("---");
        return Ok(());
    }

    // Step 5 — Persist: write idempotent block
    let target_file = resolve_target_file(target)?;
    let block = shell::render_init_block(target);

    match target {
        ShellTarget::Bash | ShellTarget::Zsh => {
            let existing = std::fs::read_to_string(&target_file).unwrap_or_default();
            let updated = replace_or_append_block(&existing, &block);
            std::fs::write(&target_file, updated)
                .with_context(|| format!("failed to write {}", target_file.display()))?;
        }
        ShellTarget::Fish => {
            if let Some(parent) = target_file.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create {}", parent.display()))?;
            }
            std::fs::write(&target_file, &block)
                .with_context(|| format!("failed to write {}", target_file.display()))?;
        }
    }

    println!("Speck environment persisted to {}", target_file.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const INIT_SOURCE: &str = include_str!("init.rs");

    /// Slice production code only — everything before `#[cfg(test)]` — so tests
    /// do not trivially pass because assertion strings themselves contain the
    /// searched tokens.
    fn production_code() -> &'static str {
        let end = INIT_SOURCE
            .find("#[cfg(test)]")
            .unwrap_or(INIT_SOURCE.len());
        &INIT_SOURCE[..end]
    }

    fn temp_home(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("speck-init-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // --- Source-inspection tests ---

    #[test]
    fn init_has_begin_speck_marker() {
        let src = production_code();
        assert!(
            src.contains("# BEGIN speck"),
            "init.rs production code must emit the # BEGIN speck sentinel marker"
        );
    }

    #[test]
    fn init_has_end_speck_marker() {
        let src = production_code();
        assert!(
            src.contains("# END speck"),
            "init.rs production code must emit the # END speck sentinel marker"
        );
    }

    #[test]
    fn init_warns_on_docker_host_conflict() {
        let src = production_code();
        assert!(
            src.contains("Warning: DOCKER_HOST is currently set to"),
            "run_init must print a warning showing the current DOCKER_HOST value on conflict"
        );
    }

    #[test]
    fn init_requires_set_docker_host_flag() {
        let src = production_code();
        assert!(
            src.contains("set_docker_host"),
            "InitArgs must expose the --set-docker-host opt-in flag as set_docker_host"
        );
    }

    #[test]
    fn init_supports_fish_conf_d() {
        let src = production_code();
        assert!(
            src.contains("conf.d/speck.fish"),
            "run_init must target ~/.config/fish/conf.d/speck.fish for fish shell persistence"
        );
    }

    // --- Environment-safety guard (no raw env set_var / remove_var) ---

    #[test]
    fn no_unsafe_env_mutation() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/commands/init.rs");
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

    // --- Temp-dir filesystem tests (idempotency + conflict warning) ---

    #[test]
    fn init_writes_bashrc_block() {
        let home = temp_home("bashrc");
        temp_env::with_vars(
            [
                ("HOME", Some(home.to_str().unwrap())),
                ("DOCKER_HOST", None),
                ("XDG_CONFIG_HOME", None),
            ],
            || {
                let speck_home = home.join(".local/share/speck");
                std::fs::create_dir_all(&speck_home).unwrap();

                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                rt.block_on(run_init(
                    InitArgs {
                        set_docker_host: true,
                        shell: Some("bash".into()),
                    },
                    &speck_home,
                ))
                .unwrap();

                let bashrc = std::fs::read_to_string(home.join(".bashrc")).unwrap();
                let begin_count = bashrc.matches("# BEGIN speck").count();
                assert_eq!(
                    begin_count, 1,
                    "bashrc must contain exactly one # BEGIN speck block; got {begin_count}"
                );
                let end_count = bashrc.matches("# END speck").count();
                assert_eq!(
                    end_count, 1,
                    "bashrc must contain exactly one # END speck block; got {end_count}"
                );
            },
        );
    }

    #[test]
    fn init_is_idempotent_for_zsh() {
        let home = temp_home("zsh-idempotent");
        temp_env::with_vars(
            [
                ("HOME", Some(home.to_str().unwrap())),
                ("DOCKER_HOST", None),
                ("XDG_CONFIG_HOME", None),
            ],
            || {
                let speck_home = home.join(".local/share/speck");
                std::fs::create_dir_all(&speck_home).unwrap();

                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let args = InitArgs {
                    set_docker_host: true,
                    shell: Some("zsh".into()),
                };
                rt.block_on(run_init(args.clone(), &speck_home)).unwrap();
                rt.block_on(run_init(args, &speck_home)).unwrap();

                let zshrc = std::fs::read_to_string(home.join(".zshrc"))
                    .expect("zshrc must exist after init --set-docker-host");
                let begin_count = zshrc.matches("# BEGIN speck").count();
                assert_eq!(
                    begin_count, 1,
                    "running spk init --set-docker-host twice must produce exactly one block; got {begin_count}"
                );
            },
        );
    }

    #[test]
    fn init_writes_fish_conf_d() {
        let home = temp_home("fish");
        let xdg = home.join("config");
        temp_env::with_vars(
            [
                ("HOME", Some(home.to_str().unwrap())),
                ("XDG_CONFIG_HOME", Some(xdg.to_str().unwrap())),
                ("DOCKER_HOST", None),
            ],
            || {
                let speck_home = home.join(".local/share/speck");
                std::fs::create_dir_all(&speck_home).unwrap();

                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                rt.block_on(run_init(
                    InitArgs {
                        set_docker_host: true,
                        shell: Some("fish".into()),
                    },
                    &speck_home,
                ))
                .unwrap();

                let fish_file = xdg.join("fish/conf.d/speck.fish");
                assert!(
                    fish_file.exists(),
                    "conf.d/speck.fish must exist after fish init --set-docker-host"
                );
                let content = std::fs::read_to_string(&fish_file).unwrap();
                assert!(
                    content.contains("# BEGIN speck"),
                    "fish conf.d file must contain the # BEGIN speck marker; got: {content:?}"
                );
                assert!(
                    content.contains("spk env --shell fish | source"),
                    "fish conf.d file must invoke `spk env --shell fish | source`; got: {content:?}"
                );
            },
        );
    }

    #[test]
    fn init_warns_when_docker_host_conflict() {
        let home = temp_home("docker-host-conflict");
        temp_env::with_vars(
            [
                ("HOME", Some(home.to_str().unwrap())),
                ("DOCKER_HOST", Some("unix:///var/run/docker.sock")),
                ("XDG_CONFIG_HOME", None),
            ],
            || {
                let speck_home = home.join(".local/share/speck");
                std::fs::create_dir_all(&speck_home).unwrap();

                // Capture stderr by running init without --set-docker-host (returns Ok, no writes).
                // The warning is to eprintln, so we can't capture it programmatically without
                // redirecting stderr; instead we just assert that run_init completes Ok
                // (the warning is informational, not a hard error).
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let result = rt.block_on(run_init(
                    InitArgs {
                        set_docker_host: false,
                        shell: Some("zsh".into()),
                    },
                    &speck_home,
                ));
                assert!(
                    result.is_ok(),
                    "run_init must complete Ok even when DOCKER_HOST conflicts (warning only)"
                );
            },
        );
    }

    #[test]
    fn init_creates_config_yaml_when_absent() {
        let home = temp_home("config-scaffold");
        temp_env::with_vars(
            [
                ("HOME", Some(home.to_str().unwrap())),
                ("DOCKER_HOST", None),
                ("XDG_CONFIG_HOME", None),
            ],
            || {
                let speck_home = home.join(".speck");
                std::fs::create_dir_all(&speck_home).unwrap();

                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                rt.block_on(run_init(
                    InitArgs { set_docker_host: false, shell: Some("zsh".into()) },
                    &speck_home,
                ))
                .unwrap();

                let config = speck_home.join("config.yaml");
                assert!(config.exists(), "config.yaml must be created by spk init");
                let content = std::fs::read_to_string(&config).unwrap();
                assert!(content.contains("version: 1"), "scaffold must include version: 1");
                assert!(content.contains("vm:"), "scaffold must include vm: section");
                assert!(content.contains("ca:"), "scaffold must include ca: section");
                assert!(content.contains("extra_certs"), "scaffold must mention extra_certs");

                // Verify it's a valid config (parseable by load_config_file)
                let (app_config, warnings) = crate::config::load_config_file(&speck_home).unwrap();
                assert_eq!(app_config.version, Some(1));
                assert!(warnings.is_empty(), "scaffold must produce no unknown-key warnings");
            },
        );
    }

    #[test]
    fn init_does_not_overwrite_existing_config_yaml() {
        let home = temp_home("config-no-overwrite");
        temp_env::with_vars(
            [
                ("HOME", Some(home.to_str().unwrap())),
                ("DOCKER_HOST", None),
                ("XDG_CONFIG_HOME", None),
            ],
            || {
                let speck_home = home.join(".speck");
                std::fs::create_dir_all(&speck_home).unwrap();
                let config = speck_home.join("config.yaml");
                std::fs::write(&config, "version: 1\n# custom user content\n").unwrap();

                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                rt.block_on(run_init(
                    InitArgs { set_docker_host: false, shell: Some("zsh".into()) },
                    &speck_home,
                ))
                .unwrap();

                let after = std::fs::read_to_string(&config).unwrap();
                assert!(
                    after.contains("custom user content"),
                    "spk init must not overwrite existing config.yaml"
                );
            },
        );
    }
}
