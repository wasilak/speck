use std::path::Path;

use crate::EnvArgs;
use crate::shell::{EnvShell, render_env, render_speck_home};

/// Print shell-ready env exports for the current Speck session.
///
/// `spk env` is the session-scoped counterpart to `spk init`: it emits the
/// `DOCKER_HOST`, `TESTCONTAINERS_DOCKER_SOCKET_OVERRIDE`, and `SPECK_HOME`
/// exports (or fish `set -gx` lines) that point Docker-compatible tooling at
/// the Speck socket. The output is meant to be eval'd by the caller's shell.
pub fn run_env(args: EnvArgs, speck_home: &Path) {
    let shell = match args.shell.as_str() {
        "posix" => EnvShell::Posix,
        "fish" => EnvShell::Fish,
        other => {
            eprintln!("error: unsupported shell `{other}` (expected `posix` or `fish`)");
            std::process::exit(1);
        }
    };
    print!("{}", render_env(speck_home, shell));
    print!("{}", render_speck_home(speck_home, shell));
}
