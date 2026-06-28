#![deny(clippy::print_stdout)]
#![deny(clippy::print_stderr)]
#![deny(clippy::dbg_macro)]

use std::path::PathBuf;
use std::sync::Arc;

pub mod buildkit;
pub mod containerd_client;
pub mod error;
pub mod handlers;
pub mod registry_auth;
pub mod router;
pub mod server;
pub mod state;
pub mod stream;

pub use error::{DockerApiError, Result};

pub struct SpeckDockerd {
    sock_path: PathBuf,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
}

impl SpeckDockerd {
    pub fn start(guest: Arc<speck_vz::Guest>, sock_path: PathBuf) -> Result<Self> {
        let containerd_proxy_path = guest
            .containerd_unix_proxy()
            .map_err(|err| DockerApiError::Internal(err.to_string()))?;
        let state = state::AppState::with_containerd_proxy(guest, containerd_proxy_path);
        let router = router::build_router(state);
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
