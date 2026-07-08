#![deny(clippy::print_stdout)]
#![deny(clippy::print_stderr)]
#![deny(clippy::dbg_macro)]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::RwLock;

use speck_core::VmState;

pub mod buildkit;
pub mod containerd_client;
pub mod error;
pub mod handlers;
pub mod log_relay;
pub mod middleware;
pub mod registry_auth;
pub mod router;
pub mod server;
pub mod state;
pub mod storage;
pub mod stream;

pub use error::{DockerApiError, Result};

use crate::middleware::restart_503::RestartCheckLayer;

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
        let containerd_proxy_path = guest
            .containerd_unix_proxy()
            .map_err(|err| DockerApiError::Internal(err.to_string()))?;

        let speck_home = sock_path
            .parent()
            .ok_or_else(|| DockerApiError::Internal("sock_path has no parent directory".into()))?;
        let storage_dir = speck_home.join("data");
        std::fs::create_dir_all(&storage_dir)
            .map_err(|e| DockerApiError::Internal(format!("create storage dir: {e}")))?;
        let storage_path = storage_dir.join("docker-state.db");
        let storage = storage::Storage::open(&storage_path)?;
        tracing::info!(path = %storage_path.display(), "SQLite Docker state storage initialized");

        let reconciled = storage.reconcile_execs()?;
        if reconciled > 0 {
            tracing::info!(count = reconciled, "reconciled exec sessions on startup");
        }

        let state = state::AppState::with_storage(guest, containerd_proxy_path, storage);
        let router = router::build_router(state).layer(RestartCheckLayer::new(vm_state));
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
