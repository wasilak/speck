use std::path::Path;

/// Shell dialect supported by `spk env` and printed by `spk up`.
#[derive(Clone, Copy)]
pub enum EnvShell {
    Posix,
    Fish,
}

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
}