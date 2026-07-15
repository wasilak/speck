#![deny(clippy::print_stdout)]
#![deny(clippy::print_stderr)]
#![deny(clippy::dbg_macro)]

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::RwLock;

use speck_core::VmState;

pub mod error;
pub mod middleware;
pub mod proxy;
pub mod server;

pub use error::{DockerApiError, Result};

pub struct SpeckDockerd {
    sock_path: PathBuf,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
}

impl SpeckDockerd {
    pub fn start(
        guest: Arc<speck_vz::Guest>,
        sock_path: PathBuf,
        vm_state: Arc<RwLock<VmState>>,
    ) -> Result<Self> {
        let speck_home = sock_path
            .parent()
            .ok_or_else(|| DockerApiError::Internal("sock_path has no parent directory".into()))?;
        let run_dir = speck_home.join("run");
        std::fs::create_dir_all(&run_dir)
            .map_err(|e| DockerApiError::Internal(format!("create runtime dir: {e}")))?;
        std::fs::set_permissions(&run_dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| DockerApiError::Internal(format!("chmod runtime dir 0700: {e}")))?;

        let internal_sock_path = run_dir.join("dockerd-proxy.sock");

        guest
            .docker_api_unix_proxy(internal_sock_path.clone())
            .map_err(|err| DockerApiError::Internal(err.to_string()))?;
        std::fs::set_permissions(&internal_sock_path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| {
                DockerApiError::Internal(format!("chmod internal dockerd proxy socket 0600: {e}"))
            })?;

        tracing::info!(
            public_sock = %sock_path.display(),
            internal_sock = %internal_sock_path.display(),
            "serving speck.sock via guest dockerd passthrough proxy"
        );

        let router = proxy::build_proxy_router(internal_sock_path, Some(guest), vm_state);
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

        tokio::spawn(server::serve(router, sock_path.clone(), shutdown_rx));

        Ok(Self {
            sock_path,
            shutdown_tx,
        })
    }

    pub fn sock_path(&self) -> &std::path::Path {
        &self.sock_path
    }

    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }
}

impl Drop for SpeckDockerd {
    fn drop(&mut self) {
        let _ = self.shutdown_tx.send(true);
    }
}

#[cfg(test)]
mod tests {
    const SOURCE: &str = include_str!("lib.rs");

    #[test]
    fn speck_sock_is_wired_to_guest_dockerd_proxy_d21() {
        assert!(
            SOURCE.contains("guest\n            .docker_api_unix_proxy")
                || SOURCE.contains("guest\r\n            .docker_api_unix_proxy"),
            "SpeckDockerd::start must expose speck.sock via guest.docker_api_unix_proxy (D-21)"
        );
        assert!(
            SOURCE.contains("proxy::build_proxy_router"),
            "SpeckDockerd::start must build the passthrough proxy router"
        );
        assert!(
            !SOURCE.contains(concat!("router::build_", "router(state)")),
            "SpeckDockerd::start must not wire speck.sock back to containerd-backed endpoint handlers"
        );
    }
}
