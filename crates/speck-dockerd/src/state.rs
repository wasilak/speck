use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use speck_core::network::NetworkSummary;
use speck_core::volume::VolumeSummary;

use crate::buildkit::BuildkitClient;
use crate::containerd_client::ContainerdClient;
use crate::error::{DockerApiError, Result};

#[derive(Clone)]
pub struct AppState {
    pub guest: Arc<speck_vz::Guest>,
    pub containerd_proxy: Arc<tokio::sync::Mutex<Option<PathBuf>>>,
    pub exec_store: Arc<tokio::sync::Mutex<ExecStore>>,
    pub network_store: Arc<tokio::sync::Mutex<HashMap<String, NetworkSummary>>>,
    pub volume_store: Arc<tokio::sync::Mutex<HashMap<String, VolumeSummary>>>,
    pub event_tx: Arc<tokio::sync::broadcast::Sender<serde_json::Value>>,
    pub storage: Arc<tokio::sync::Mutex<crate::storage::Storage>>,
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
        if self.entries.len() >= Self::MAX_ENTRIES
            && let Some(oldest) = self.order.pop_front()
        {
            self.entries.remove(&oldest);
            tracing::warn!(exec_id = %oldest, "evicted oldest Docker exec spec from capped exec_store");
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
        let storage = crate::storage::Storage::open(":memory:").expect("in-memory storage");
        Self {
            guest,
            containerd_proxy: Arc::new(tokio::sync::Mutex::new(None)),
            exec_store: Arc::new(tokio::sync::Mutex::new(ExecStore::default())),
            network_store: Arc::new(tokio::sync::Mutex::new(default_networks())),
            volume_store: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            event_tx: Arc::new(tokio::sync::broadcast::channel(256).0),
            storage: Arc::new(tokio::sync::Mutex::new(storage)),
        }
    }

    pub fn with_containerd_proxy(guest: Arc<speck_vz::Guest>, path: PathBuf) -> Self {
        let storage = crate::storage::Storage::open(":memory:").expect("in-memory storage");
        Self {
            guest,
            containerd_proxy: Arc::new(tokio::sync::Mutex::new(Some(path))),
            exec_store: Arc::new(tokio::sync::Mutex::new(ExecStore::default())),
            network_store: Arc::new(tokio::sync::Mutex::new(default_networks())),
            volume_store: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            event_tx: Arc::new(tokio::sync::broadcast::channel(256).0),
            storage: Arc::new(tokio::sync::Mutex::new(storage)),
        }
    }

    pub fn with_storage(
        guest: Arc<speck_vz::Guest>,
        path: PathBuf,
        storage: crate::storage::Storage,
    ) -> Self {
        let volumes = storage.load_volumes().unwrap_or_default();
        let networks = {
            let mut nets = default_networks();
            match storage.load_networks() {
                Ok(loaded) => nets.extend(loaded),
                Err(e) => {
                    tracing::warn!(error = %e, "failed to load networks from storage; using defaults")
                }
            }
            nets
        };
        tracing::info!(
            volumes = volumes.len(),
            networks = networks.len(),
            "loaded persisted Docker state from storage"
        );
        Self {
            guest,
            containerd_proxy: Arc::new(tokio::sync::Mutex::new(Some(path))),
            exec_store: Arc::new(tokio::sync::Mutex::new(ExecStore::default())),
            network_store: Arc::new(tokio::sync::Mutex::new(networks)),
            volume_store: Arc::new(tokio::sync::Mutex::new(volumes)),
            event_tx: Arc::new(tokio::sync::broadcast::channel(256).0),
            storage: Arc::new(tokio::sync::Mutex::new(storage)),
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

    pub async fn buildkitd_path(&self) -> Result<PathBuf> {
        let proxy = self
            .guest
            .buildkitd_unix_proxy()
            .map_err(|err| DockerApiError::Internal(err.to_string()))?;
        Ok(proxy)
    }

    pub async fn buildkit_client(&self) -> Result<BuildkitClient> {
        let path = self.buildkitd_path().await?;
        BuildkitClient::connect_unix(path)
            .await
            .map_err(|e| DockerApiError::Internal(e.to_string()))
    }

    pub async fn containerd_client(&self) -> Result<ContainerdClient> {
        let path = self.containerd_path().await?;
        ContainerdClient::connect(path).await
    }
}

fn default_networks() -> HashMap<String, NetworkSummary> {
    let bridge = NetworkSummary {
        id: "bridge".into(),
        name: "bridge".into(),
        driver: "bridge".into(),
        scope: "local".into(),
    };
    HashMap::from([("bridge".into(), bridge)])
}
