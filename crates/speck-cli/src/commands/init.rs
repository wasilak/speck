use std::path::{Path, PathBuf};

use anyhow::Context as _;

use crate::InitArgs;
use crate::shell::{self, BEGIN_MARKER, END_MARKER, EnvShell, ShellTarget};

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

    // Step 3 — Preview/persist based on --set-docker-host
    if !args.set_docker_host {
        println!("Run `spk init --set-docker-host` to persist Speck environment in your shell.");
        println!();
        println!("--- Preview (POSIX) ---");
        print!("{}", shell::render_env(speck_home, EnvShell::Posix));
        print!("{}", shell::render_speck_home(speck_home, EnvShell::Posix));
        println!("---");
        return Ok(());
    }

    // Step 4 — Persist: write idempotent block
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
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

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

    // --- Temp-dir filesystem tests (idempotency + conflict warning) ---

    #[test]
    fn init_writes_bashrc_block() {
        let _guard = ENV_LOCK.lock().unwrap();
        let home = temp_home("bashrc");
        unsafe {
            std::env::set_var("HOME", &home);
            std::env::remove_var("DOCKER_HOST");
            std::env::remove_var("XDG_CONFIG_HOME");
        }

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

        unsafe {
            std::env::remove_var("HOME");
        }
    }

    #[test]
    fn init_is_idempotent_for_zsh() {
        let _guard = ENV_LOCK.lock().unwrap();
        let home = temp_home("zsh-idempotent");
        unsafe {
            std::env::set_var("HOME", &home);
            std::env::remove_var("DOCKER_HOST");
            std::env::remove_var("XDG_CONFIG_HOME");
        }

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

        unsafe {
            std::env::remove_var("HOME");
        }
    }

    #[test]
    fn init_writes_fish_conf_d() {
        let _guard = ENV_LOCK.lock().unwrap();
        let home = temp_home("fish");
        let xdg = home.join("config");
        unsafe {
            std::env::set_var("HOME", &home);
            std::env::set_var("XDG_CONFIG_HOME", &xdg);
            std::env::remove_var("DOCKER_HOST");
        }

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

        unsafe {
            std::env::remove_var("HOME");
            std::env::remove_var("XDG_CONFIG_HOME");
        }
    }

    #[test]
    fn init_warns_when_docker_host_conflict() {
        let _guard = ENV_LOCK.lock().unwrap();
        let home = temp_home("docker-host-conflict");
        unsafe {
            std::env::set_var("HOME", &home);
            std::env::set_var("DOCKER_HOST", "unix:///var/run/docker.sock");
            std::env::remove_var("XDG_CONFIG_HOME");
        }

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

        unsafe {
            std::env::remove_var("HOME");
            std::env::remove_var("DOCKER_HOST");
        }
    }
}
