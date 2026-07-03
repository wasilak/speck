use std::path::Path;

/// Shell dialect supported by `spk env` and printed by `spk up`.
pub enum EnvShell {
    Posix,
    Fish,
}

/// Render shell-ready export/set statements for `DOCKER_HOST` and
/// `TESTCONTAINERS_DOCKER_SOCKET_OVERRIDE`, derived from `speck_home`.
///
/// The socket path is `speck_home/speck.sock`; the Docker host string uses
/// the `unix://` scheme. STUB: real implementation lands in the GREEN commit.
pub fn render_env(_speck_home: &Path, _shell: EnvShell) -> String {
    String::new()
}

/// Render the single shell export/set line for `SPECK_HOME`.
///
/// STUB: real implementation lands in the GREEN commit.
pub fn render_speck_home(_speck_home: &Path, _shell: EnvShell) -> String {
    String::new()
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