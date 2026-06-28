use std::path::PathBuf;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use crate::containerd_client::ContainerdClient;
use crate::error::{DockerApiError, Result};

#[derive(Clone)]
pub struct AppState {
    pub guest: Arc<speck_vz::Guest>,
    pub containerd_proxy: Arc<tokio::sync::Mutex<Option<PathBuf>>>,
    pub exec_store: Arc<tokio::sync::Mutex<ExecStore>>,
}

#[derive(Default)]
pub struct ExecStore {
    entries: HashMap<String, ExecSpec>,
    order: VecDeque<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecSpec {
    pub id: String,
    pub container_id: String,
    pub cmd: Vec<String>,
    pub env: Vec<String>,
    pub attach_stdin: bool,
    pub attach_stdout: bool,
    pub attach_stderr: bool,
    pub tty: bool,
    pub running: bool,
    pub exit_code: Option<i64>,
}

impl ExecStore {
    const MAX_ENTRIES: usize = 1000;

    pub fn insert(&mut self, id: String, spec: ExecSpec) {
        if self.entries.len() >= Self::MAX_ENTRIES {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
                tracing::warn!(exec_id = %oldest, "evicted oldest Docker exec spec from capped exec_store");
            }
        }
        self.order.push_back(id.clone());
        self.entries.insert(id, spec);
    }

    pub fn get(&self, id: &str) -> Option<ExecSpec> {
        self.entries.get(id).cloned()
    }

    pub fn update(&mut self, id: &str, spec: ExecSpec) {
        self.entries.insert(id.to_owned(), spec);
    }
}

impl AppState {
    pub fn new(guest: Arc<speck_vz::Guest>) -> Self {
        Self {
            guest,
            containerd_proxy: Arc::new(tokio::sync::Mutex::new(None)),
            exec_store: Arc::new(tokio::sync::Mutex::new(ExecStore::default())),
        }
    }

    pub fn with_containerd_proxy(guest: Arc<speck_vz::Guest>, path: PathBuf) -> Self {
        Self {
            guest,
            containerd_proxy: Arc::new(tokio::sync::Mutex::new(Some(path))),
            exec_store: Arc::new(tokio::sync::Mutex::new(ExecStore::default())),
        }
    }

    pub async fn containerd_path(&self) -> Result<PathBuf> {
        let mut guard = self.containerd_proxy.lock().await;
        if let Some(path) = guard.as_ref() {
            return Ok(path.clone());
        }

        let path = self
            .guest
            .containerd_unix_proxy()
            .map_err(|err| DockerApiError::Internal(err.to_string()))?;
        *guard = Some(path.clone());

        Ok(path)
    }

    pub async fn containerd_client(&self) -> Result<ContainerdClient> {
        let path = self.containerd_path().await?;
        ContainerdClient::connect(path).await
    }
}
