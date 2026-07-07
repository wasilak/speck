use std::path::Path;

/// Shell dialect supported by `spk env` and printed by `spk up`.
#[derive(Clone, Copy)]
pub enum EnvShell {
    Posix,
    Fish,
}

/// Shell target for `spk init` persistent bootstrap writes.
///
/// Bash and Zsh share the same POSIX `if command -v spk` block format written
/// to `~/.bashrc` / `~/.zshrc`; Fish uses a dedicated `conf.d/speck.fish` file
/// with fish-native `if command -q spk` syntax.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShellTarget {
    Bash,
    Zsh,
    Fish,
}

/// Detect the user's shell from the `SHELL` environment variable.
///
/// Inspects `$SHELL` for `bash`, `zsh`, or `fish` substrings (case-sensitive).
/// On macOS the default interactive shell is zsh, so an unset or unrecognized
/// `SHELL` value falls back to [`ShellTarget::Zsh`].
pub fn detect_shell() -> ShellTarget {
    match std::env::var("SHELL") {
        Ok(ref s) if s.contains("bash") => ShellTarget::Bash,
        Ok(ref s) if s.contains("fish") => ShellTarget::Fish,
        Ok(ref s) if s.contains("zsh") => ShellTarget::Zsh,
        _ => ShellTarget::Zsh,
    }
}

/// Render the idempotent bootstrap block for the given shell target.
///
/// The block is wrapped in `# BEGIN speck` / `# END speck` sentinel markers so
/// repeated `spk init --set-docker-host` runs replace the region instead of
/// appending duplicates. Bash/Zsh emit `if command -v spk >/dev/null 2>&1; then
/// eval "$(spk env)"; fi`; Fish emits `if command -q spk; spk env --shell fish |
/// source; end`.
pub fn render_init_block(target: ShellTarget) -> String {
    match target {
        ShellTarget::Bash | ShellTarget::Zsh => format!(
            "{begin}\nif command -v spk >/dev/null 2>&1; then\n  eval \"$(spk env)\"\nfi\n{end}\n",
            begin = BEGIN_MARKER,
            end = END_MARKER,
        ),
        ShellTarget::Fish => format!(
            "{begin}\nif command -q spk\n  spk env --shell fish | source\nend\n{end}\n",
            begin = BEGIN_MARKER,
            end = END_MARKER,
        ),
    }
}

/// Sentinel marker marking the start of a Speck bootstrap block.
pub const BEGIN_MARKER: &str = "# BEGIN speck";

/// Sentinel marker marking the end of a Speck bootstrap block.
pub const END_MARKER: &str = "# END speck";

/// Render shell-ready export/set statements for `DOCKER_HOST` and
/// `TESTCONTAINERS_DOCKER_SOCKET_OVERRIDE`, derived from `speck_home`.
///
/// The socket path is `speck_home/speck.sock`; the Docker host string uses
/// the `unix://` scheme. POSIX shells receive `export KEY=VALUE` lines, fish
/// receives `set -gx KEY VALUE` lines. Output always ends with a trailing
/// newline so callers can concatenate it safely.
pub fn render_env(speck_home: &Path, shell: EnvShell) -> String {
    let sock_path = speck_home.join("speck.sock");
    let docker_host = format!("unix://{}", sock_path.display());
    match shell {
        EnvShell::Posix => format!(
            "export DOCKER_HOST={docker_host}\nexport TESTCONTAINERS_DOCKER_SOCKET_OVERRIDE=/var/run/docker.sock\n"
        ),
        EnvShell::Fish => format!(
            "set -gx DOCKER_HOST {docker_host};\nset -gx TESTCONTAINERS_DOCKER_SOCKET_OVERRIDE /var/run/docker.sock;\n"
        ),
    }
}

/// Render the single shell export/set line for `SPECK_HOME`.
///
/// POSIX shells receive `export SPECK_HOME=<home>`, fish receives
/// `set -gx SPECK_HOME <home>;`. Output always ends with a trailing newline.
pub fn render_speck_home(speck_home: &Path, shell: EnvShell) -> String {
    let home = speck_home.display();
    match shell {
        EnvShell::Posix => format!("export SPECK_HOME={home}\n"),
        EnvShell::Fish => format!("set -gx SPECK_HOME {home};\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn home() -> PathBuf {
        PathBuf::from("/tmp/speck-home")
    }

    // --- Environment-safety guard ---

    #[test]
    fn no_unsafe_env_mutation() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/shell.rs");
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
    fn render_env_posix_emits_docker_host_with_unix_scheme_and_socket_path() {
        let out = render_env(&home(), EnvShell::Posix);
        assert!(
            out.contains("export DOCKER_HOST=unix:///tmp/speck-home/speck.sock"),
            "posix render_env must emit `export DOCKER_HOST=unix://<speck_home>/speck.sock`; got: {out:?}"
        );
    }

    #[test]
    fn render_env_posix_emits_testcontainers_socket_override() {
        let out = render_env(&home(), EnvShell::Posix);
        assert!(
            out.contains("export TESTCONTAINERS_DOCKER_SOCKET_OVERRIDE=/var/run/docker.sock"),
            "posix render_env must emit the testcontainers socket override; got: {out:?}"
        );
    }

    #[test]
    fn render_env_fish_emits_set_gx_docker_host() {
        let out = render_env(&home(), EnvShell::Fish);
        assert!(
            out.contains("set -gx DOCKER_HOST unix:///tmp/speck-home/speck.sock"),
            "fish render_env must emit `set -gx DOCKER_HOST unix://...`; got: {out:?}"
        );
    }

    #[test]
    fn render_env_fish_emits_set_gx_testcontainers_socket_override() {
        let out = render_env(&home(), EnvShell::Fish);
        assert!(
            out.contains("set -gx TESTCONTAINERS_DOCKER_SOCKET_OVERRIDE /var/run/docker.sock"),
            "fish render_env must emit the testcontainers socket override with set -gx; got: {out:?}"
        );
    }

    #[test]
    fn render_speck_home_posix_emits_export() {
        let out = render_speck_home(&home(), EnvShell::Posix);
        assert!(
            out.contains("export SPECK_HOME=/tmp/speck-home"),
            "posix render_speck_home must emit `export SPECK_HOME=<home>`; got: {out:?}"
        );
    }

    #[test]
    fn render_speck_home_fish_emits_set_gx() {
        let out = render_speck_home(&home(), EnvShell::Fish);
        assert!(
            out.contains("set -gx SPECK_HOME /tmp/speck-home"),
            "fish render_speck_home must emit `set -gx SPECK_HOME <home>`; got: {out:?}"
        );
    }

    // --- ShellTarget / detect_shell / render_init_block tests ---

    #[test]
    fn detect_shell_defaults_to_zsh_when_unset() {
        temp_env::with_var("SHELL", None::<&str>, || {
            assert_eq!(
                detect_shell(),
                ShellTarget::Zsh,
                "detect_shell must default to Zsh on macOS when SHELL is unset"
            );
        });
    }

    #[test]
    fn detect_shell_returns_bash_for_bash_shell() {
        temp_env::with_var("SHELL", Some("/bin/bash"), || {
            assert_eq!(
                detect_shell(),
                ShellTarget::Bash,
                "detect_shell must return Bash when SHELL contains `bash`"
            );
        });
    }

    #[test]
    fn detect_shell_returns_zsh_for_zsh_shell() {
        temp_env::with_var("SHELL", Some("/bin/zsh"), || {
            assert_eq!(
                detect_shell(),
                ShellTarget::Zsh,
                "detect_shell must return Zsh when SHELL contains `zsh`"
            );
        });
    }

    #[test]
    fn detect_shell_returns_fish_for_fish_shell() {
        temp_env::with_var("SHELL", Some("/usr/local/bin/fish"), || {
            assert_eq!(
                detect_shell(),
                ShellTarget::Fish,
                "detect_shell must return Fish when SHELL contains `fish`"
            );
        });
    }

    #[test]
    fn detect_shell_defaults_to_zsh_for_unrecognized_shell() {
        temp_env::with_var("SHELL", Some("/bin/sh"), || {
            assert_eq!(
                detect_shell(),
                ShellTarget::Zsh,
                "detect_shell must default to Zsh for unrecognized SHELL values"
            );
        });
    }

    #[test]
    fn render_init_block_bash_contains_begin_and_end_markers() {
        let block = render_init_block(ShellTarget::Bash);
        assert!(
            block.contains("# BEGIN speck"),
            "bash init block must contain # BEGIN speck sentinel; got: {block:?}"
        );
        assert!(
            block.contains("# END speck"),
            "bash init block must contain # END speck sentinel; got: {block:?}"
        );
    }

    #[test]
    fn render_init_block_bash_contains_command_v_spk_and_eval() {
        let block = render_init_block(ShellTarget::Bash);
        assert!(
            block.contains("command -v spk"),
            "bash init block must guard with `command -v spk`; got: {block:?}"
        );
        assert!(
            block.contains("eval \"$(spk env)\""),
            "bash init block must invoke `eval \"$(spk env)\"`; got: {block:?}"
        );
    }

    #[test]
    fn render_init_block_zsh_contains_same_format_as_bash() {
        let bash_block = render_init_block(ShellTarget::Bash);
        let zsh_block = render_init_block(ShellTarget::Zsh);
        assert_eq!(
            bash_block, zsh_block,
            "Zsh and Bash init blocks must be identical (both use POSIX eval syntax)"
        );
    }

    #[test]
    fn render_init_block_fish_contains_begin_and_end_markers() {
        let block = render_init_block(ShellTarget::Fish);
        assert!(
            block.contains("# BEGIN speck"),
            "fish init block must contain # BEGIN speck sentinel; got: {block:?}"
        );
        assert!(
            block.contains("# END speck"),
            "fish init block must contain # END speck sentinel; got: {block:?}"
        );
    }

    #[test]
    fn render_init_block_fish_contains_command_q_spk_and_source() {
        let block = render_init_block(ShellTarget::Fish);
        assert!(
            block.contains("command -q spk"),
            "fish init block must guard with `command -q spk`; got: {block:?}"
        );
        assert!(
            block.contains("spk env --shell fish | source"),
            "fish init block must pipe `spk env --shell fish` into source; got: {block:?}"
        );
    }

    #[test]
    fn render_init_block_fish_ends_with_end_not_fi() {
        let block = render_init_block(ShellTarget::Fish);
        assert!(
            block.contains("end"),
            "fish init block must use `end` to close the if block; got: {block:?}"
        );
        assert!(
            !block.contains("\nfi\n"),
            "fish init block must NOT use POSIX `fi` as a standalone closing keyword; got: {block:?}"
        );
    }
}
