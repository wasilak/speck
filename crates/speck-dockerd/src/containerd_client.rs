use std::collections::HashMap;
use std::path::Path;

use containerd_client::services::v1::snapshots::{MountsRequest, PrepareSnapshotRequest};
use containerd_client::services::v1::{
    Container, CreateContainerRequest, CreateTaskRequest, DeleteContainerRequest,
    DeleteImageRequest, DeleteTaskRequest, ExecProcessRequest, GetContainerRequest,
    GetImageRequest, GetRequest, KillRequest, ListContainersRequest, ListImagesRequest,
    ListTasksRequest, StartRequest, TransferRequest, WaitRequest,
    container::Runtime as ContainerRuntime,
};
use containerd_client::tonic;
use containerd_client::types;
use containerd_client::types::transfer as transfer_types;
use containerd_client::types::v1::{Process, Status};
use tonic::transport::Channel;

use crate::error::{DockerApiError, Result};
use crate::registry_auth::RegistryCredentials;

const NAMESPACE: &str = "speck";

#[derive(Debug, Clone)]
pub struct ContainerdClient {
    channel: Channel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskId(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskSpec {
    pub container_id: String,
    pub terminal: bool,
    pub stdout_fifo: Option<String>,
    pub stderr_fifo: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskInfo {
    pub container_id: String,
    pub exec_id: String,
    pub pid: u32,
    pub status: TaskStatus,
    pub exit_status: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskStatus {
    Created,
    Running,
    Stopped,
    Paused,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerCreateSpec {
    pub id: String,
    pub image: String,
    pub cmd: Vec<String>,
    pub env: Vec<String>,
    pub labels: HashMap<String, String>,
    pub restart_policy: Option<String>,
    pub restart_maximum_retry_count: Option<i64>,
    pub memory: Option<i64>,
    pub cpu_shares: Option<i64>,
    pub tty: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerInfo {
    pub id: String,
    pub image: String,
    pub labels: HashMap<String, String>,
    pub created_at_seconds: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecProcessSpec {
    pub container_id: String,
    pub exec_id: String,
    pub cmd: Vec<String>,
    pub env: Vec<String>,
    pub terminal: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageRecord {
    pub name: String,
    pub id: String,
    pub size: i64,
    pub created_at_seconds: i64,
    pub labels: HashMap<String, String>,
}

impl ContainerdClient {
    pub async fn connect(path: impl AsRef<Path>) -> Result<Self> {
        let channel = containerd_client::connect(path)
            .await
            .map_err(|err| DockerApiError::Internal(format!("connect containerd: {err}")))?;
        Ok(Self { channel })
    }

    pub async fn task_create(&self, spec: TaskSpec) -> Result<TaskId> {
        let mounts = self.container_mounts(&spec.container_id).await?;
        let mut client = containerd_client::Client::from(self.channel.clone()).tasks();
        client
            .create(with_namespace(CreateTaskRequest {
                container_id: spec.container_id.clone(),
                rootfs: mounts,
                stdin: String::new(),
                stdout: spec.stdout_fifo.unwrap_or_default(),
                stderr: spec.stderr_fifo.unwrap_or_default(),
                terminal: spec.terminal,
                checkpoint: None,
                options: None,
                runtime_path: String::new(),
            }))
            .await
            .map_err(map_status)?;
        Ok(TaskId(spec.container_id))
    }

    pub async fn task_start(&self, id: &str) -> Result<()> {
        let mut client = containerd_client::Client::from(self.channel.clone()).tasks();
        client
            .start(with_namespace(StartRequest {
                container_id: id.to_owned(),
                exec_id: String::new(),
            }))
            .await
            .map_err(map_status)?;
        Ok(())
    }

    pub async fn task_stop(&self, id: &str) -> Result<()> {
        self.task_kill(id, 15).await
    }

    pub async fn task_kill(&self, id: &str, signal: u32) -> Result<()> {
        let mut client = containerd_client::Client::from(self.channel.clone()).tasks();
        client
            .kill(with_namespace(KillRequest {
                container_id: id.to_owned(),
                exec_id: String::new(),
                signal,
                all: true,
            }))
            .await
            .map_err(map_status)?;
        Ok(())
    }

    pub async fn task_wait(&self, id: &str) -> Result<u32> {
        let mut client = containerd_client::Client::from(self.channel.clone()).tasks();
        let response = client
            .wait(with_namespace(WaitRequest {
                container_id: id.to_owned(),
                exec_id: String::new(),
            }))
            .await
            .map_err(map_status)?
            .into_inner();
        Ok(response.exit_status)
    }

    pub async fn task_delete(&self, id: &str) -> Result<()> {
        let mut client = containerd_client::Client::from(self.channel.clone()).tasks();
        client
            .delete(with_namespace(DeleteTaskRequest {
                container_id: id.to_owned(),
            }))
            .await
            .map_err(map_status)?;
        Ok(())
    }

    pub async fn task_get(&self, id: &str) -> Result<Option<TaskInfo>> {
        let mut client = containerd_client::Client::from(self.channel.clone()).tasks();
        match client
            .get(with_namespace(GetRequest {
                container_id: id.to_owned(),
                exec_id: String::new(),
            }))
            .await
        {
            Ok(response) => Ok(response.into_inner().process.map(process_to_task_info)),
            Err(status) if status.code() == tonic::Code::NotFound => Ok(None),
            Err(status) => Err(map_status(status)),
        }
    }

    pub async fn task_list(&self) -> Result<Vec<TaskInfo>> {
        let mut client = containerd_client::Client::from(self.channel.clone()).tasks();
        let response = client
            .list(with_namespace(ListTasksRequest {
                filter: String::new(),
            }))
            .await
            .map_err(map_status)?
            .into_inner();
        Ok(response
            .tasks
            .into_iter()
            .map(process_to_task_info)
            .collect())
    }

    /// Prepare a writable snapshot for a container, using the image's top
    /// committed snapshot as the parent.
    async fn container_prepare_snapshot(&self, container_id: &str, image_ref: &str) -> Result<()> {
        self.image_set_snapshot_key(image_ref).await?;
        let image = self.image_get(image_ref).await?;

        let parent_key = image
            .labels
            .get("containerd.io/snapshot/overlayfs.key")
            .ok_or_else(|| {
                DockerApiError::Internal(format!(
                    "image {image_ref} has no unpacked snapshot key (was it pulled with unpack?)"
                ))
            })?
            .clone();

        let mut client = containerd_client::Client::from(self.channel.clone()).snapshots();
        client
            .prepare(with_namespace(PrepareSnapshotRequest {
                snapshotter: "overlayfs".into(),
                key: container_id.to_owned(),
                parent: parent_key,
                labels: HashMap::new(),
            }))
            .await
            .map_err(|status| {
                DockerApiError::Internal(format!(
                    "failed to prepare snapshot for container {container_id}: {status}"
                ))
            })?;

        Ok(())
    }

    /// Get the rootfs mounts for a container's active snapshot.
    async fn container_mounts(&self, container_id: &str) -> Result<Vec<types::Mount>> {
        let mut client = containerd_client::Client::from(self.channel.clone()).snapshots();
        let response = client
            .mounts(with_namespace(MountsRequest {
                snapshotter: "overlayfs".into(),
                key: container_id.to_owned(),
            }))
            .await
            .map_err(|status| {
                DockerApiError::Internal(format!(
                    "failed to get mounts for container {container_id}: {status}"
                ))
            })?;
        Ok(response.into_inner().mounts)
    }

    pub async fn container_create(&self, spec: ContainerCreateSpec) -> Result<String> {
        let mut labels = spec.labels;
        labels.insert("speck.image".into(), spec.image.clone());
        labels.insert("speck.cmd".into(), spec.cmd.join("\u{1f}"));
        labels.insert("speck.env".into(), spec.env.join("\u{1f}"));
        if let Some(policy) = spec.restart_policy {
            labels.insert("speck.restart_policy".into(), policy);
        }
        if let Some(count) = spec.restart_maximum_retry_count {
            labels.insert(
                "speck.restart_maximum_retry_count".into(),
                count.to_string(),
            );
        }
        if let Some(memory) = spec.memory {
            labels.insert("speck.memory".into(), memory.to_string());
        }
        if let Some(cpu_shares) = spec.cpu_shares {
            labels.insert("speck.cpu_shares".into(), cpu_shares.to_string());
        }

        let mut env = spec.env.clone();
        if !env.iter().any(|e| e.starts_with("PATH=")) {
            env.insert(
                0,
                "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".into(),
            );
        }

        let caps = [
            "CAP_CHOWN",
            "CAP_DAC_OVERRIDE",
            "CAP_FSETID",
            "CAP_FOWNER",
            "CAP_MKNOD",
            "CAP_NET_RAW",
            "CAP_SETGID",
            "CAP_SETUID",
            "CAP_SETFCAP",
            "CAP_SETPCAP",
            "CAP_NET_BIND_SERVICE",
            "CAP_SYS_CHROOT",
            "CAP_KILL",
            "CAP_AUDIT_WRITE",
        ];

        let spec_json = serde_json::json!({
            "ociVersion": "1.0.0",
            "process": {
                "terminal": spec.tty,
                "user": { "uid": 0, "gid": 0 },
                "args": spec.cmd,
                "env": env,
                "cwd": "/",
                "capabilities": {
                    "bounding": caps,
                    "effective": caps,
                    "permitted": caps,
                },
                "rlimits": [{ "type": "RLIMIT_NOFILE", "hard": 1024, "soft": 1024 }],
                "noNewPrivileges": true,
            },
            "root": {
                "path": "rootfs",
                "readonly": false,
            },
            "mounts": [
                { "destination": "/proc", "type": "proc", "source": "proc", "options": ["nosuid", "noexec", "nodev"] },
                { "destination": "/dev", "type": "tmpfs", "source": "tmpfs", "options": ["nosuid", "strictatime", "mode=755", "size=65536k"] },
                { "destination": "/dev/pts", "type": "devpts", "source": "devpts", "options": ["nosuid", "noexec", "newinstance", "ptmxmode=0666", "mode=0620", "gid=5"] },
                { "destination": "/dev/shm", "type": "tmpfs", "source": "shm", "options": ["nosuid", "noexec", "nodev", "mode=1777", "size=65536k"] },
                { "destination": "/dev/mqueue", "type": "mqueue", "source": "mqueue", "options": ["nosuid", "noexec", "nodev"] },
                { "destination": "/sys", "type": "sysfs", "source": "sysfs", "options": ["nosuid", "noexec", "nodev", "ro"] },
                { "destination": "/run", "type": "tmpfs", "source": "tmpfs", "options": ["nosuid", "strictatime", "mode=755", "size=65536k"] },
            ],
            "linux": {
                "resources": {
                    "devices": [{ "allow": false, "access": "rwm" }],
                },
                "namespaces": [
                    { "type": "pid" },
                    { "type": "ipc" },
                    { "type": "uts" },
                    { "type": "mount" },
                    { "type": "cgroup" },
                    { "type": "network" },
                ],
                "maskedPaths": [
                    "/proc/acpi", "/proc/asound", "/proc/kcore", "/proc/keys",
                    "/proc/latency_stats", "/proc/timer_list", "/proc/timer_stats",
                    "/proc/sched_debug", "/sys/firmware", "/proc/scsi",
                ],
                "readonlyPaths": [
                    "/proc/bus", "/proc/fs", "/proc/irq", "/proc/sys", "/proc/sysrq-trigger",
                ],
            },
        });

        let spec_bytes = serde_json::to_vec(&spec_json)
            .map_err(|e| DockerApiError::Internal(format!("failed to serialize OCI spec: {e}")))?;

        let spec_any = prost_types::Any {
            type_url: "types.containerd.io/opencontainers/runtime-spec/1/Spec".into(),
            value: spec_bytes,
        };

        let image_ref = spec.image.clone();
        let container = Container {
            id: spec.id.clone(),
            labels,
            image: image_ref.clone(),
            runtime: Some(ContainerRuntime {
                name: "io.containerd.runc.v2".to_string(),
                options: None,
            }),
            spec: Some(spec_any),
            snapshotter: "overlayfs".into(),
            snapshot_key: spec.id.clone(),
            created_at: None,
            updated_at: None,
            extensions: HashMap::new(),
            sandbox: String::new(),
        };

        let mut client = containerd_client::Client::from(self.channel.clone()).containers();
        let response = client
            .create(with_namespace(CreateContainerRequest {
                container: Some(container),
            }))
            .await
            .map_err(map_status)?
            .into_inner();
        let container_id = response
            .container
            .map(|container| container.id)
            .ok_or_else(|| {
                DockerApiError::Internal("containerd returned empty create response".into())
            })?;

        self.container_prepare_snapshot(&container_id, &image_ref)
            .await?;

        Ok(container_id)
    }

    pub async fn container_get(&self, id: &str) -> Result<ContainerInfo> {
        let mut client = containerd_client::Client::from(self.channel.clone()).containers();
        let response = client
            .get(with_namespace(GetContainerRequest { id: id.to_owned() }))
            .await
            .map_err(map_status)?
            .into_inner();
        response
            .container
            .map(container_to_info)
            .ok_or_else(|| DockerApiError::NotFound(format!("container {id}")))
    }

    pub async fn container_list(&self) -> Result<Vec<ContainerInfo>> {
        let mut client = containerd_client::Client::from(self.channel.clone()).containers();
        let response = client
            .list(with_namespace(ListContainersRequest {
                filters: Vec::new(),
            }))
            .await
            .map_err(map_status)?
            .into_inner();
        Ok(response
            .containers
            .into_iter()
            .map(container_to_info)
            .collect())
    }

    pub async fn container_delete(&self, id: &str) -> Result<()> {
        let mut client = containerd_client::Client::from(self.channel.clone()).containers();
        client
            .delete(with_namespace(DeleteContainerRequest { id: id.to_owned() }))
            .await
            .map_err(map_status)?;
        Ok(())
    }

    pub async fn exec_create(&self, spec: ExecProcessSpec) -> Result<()> {
        let mut client = containerd_client::Client::from(self.channel.clone()).tasks();
        client
            .exec(with_namespace(ExecProcessRequest {
                container_id: spec.container_id,
                stdin: String::new(),
                stdout: String::new(),
                stderr: String::new(),
                terminal: spec.terminal,
                spec: None,
                exec_id: spec.exec_id,
            }))
            .await
            .map_err(map_status)?;
        Ok(())
    }

    pub async fn exec_start(&self, container_id: &str, exec_id: &str) -> Result<()> {
        let mut client = containerd_client::Client::from(self.channel.clone()).tasks();
        client
            .start(with_namespace(StartRequest {
                container_id: container_id.to_owned(),
                exec_id: exec_id.to_owned(),
            }))
            .await
            .map_err(map_status)?;
        Ok(())
    }

    /// Normalize an image reference to a fully-qualified form that
    /// containerd's transfer service can resolve.
    fn normalize_reference(raw: &str) -> String {
        if raw.contains('/') {
            raw.to_owned()
        } else {
            format!("docker.io/library/{raw}")
        }
    }

    pub async fn image_pull(
        &self,
        image_ref: &str,
        _credentials: Option<RegistryCredentials>,
    ) -> Result<()> {
        tracing::debug!(reference = %image_ref, "pulling image via transfer service");

        let store_name = image_ref.to_owned();
        let pull_ref = Self::normalize_reference(image_ref);

        let source = transfer_types::OciRegistry {
            reference: pull_ref,
            resolver: None,
        };

        let dest = transfer_types::ImageStore {
            name: store_name,
            labels: HashMap::new(),
            platforms: vec![],
            all_metadata: false,
            manifest_limit: 0,
            extra_references: vec![],
            unpacks: vec![transfer_types::UnpackConfiguration {
                platform: Some(types::Platform {
                    os: "linux".into(),
                    architecture: "arm64".into(),
                    variant: String::new(),
                    os_version: String::new(),
                }),
                snapshotter: "overlayfs".into(),
            }],
        };

        let request = TransferRequest {
            source: Some(containerd_client::to_any(&source)),
            destination: Some(containerd_client::to_any(&dest)),
            options: None,
        };

        let mut client = containerd_client::Client::from(self.channel.clone()).transfer();
        client
            .transfer(with_namespace(request))
            .await
            .map_err(|status| {
                DockerApiError::Internal(format!(
                    "image pull via transfer service failed: {status}"
                ))
            })?;

        tracing::info!(reference = %image_ref, "image pulled successfully");
        Ok(())
    }

    /// Set the snapshotter key label on an image so container snapshot
    /// preparation can find the parent committed snapshot.
    ///
    /// The transfer service may not always propagate unpack labels to the
    /// image metadata, so we set it explicitly by listing the committed
    /// snapshots owned by this image.
    async fn image_set_snapshot_key(&self, image_ref: &str) -> Result<()> {
        let image = self.image_get(image_ref).await?;

        if image
            .labels
            .contains_key("containerd.io/snapshot/overlayfs.key")
        {
            return Ok(());
        }

        // List committed snapshots to find the one that belongs to this
        // image.  The snapshotter uses the image digest as part of the
        // chain of committed snapshots — look for the parent-most (last)
        // committed snapshot whose labels reference this image.
        let mut stream = containerd_client::Client::from(self.channel.clone())
            .snapshots()
            .list(with_namespace(
                containerd_client::services::v1::snapshots::ListSnapshotsRequest {
                    snapshotter: "overlayfs".into(),
                    filters: vec![],
                },
            ))
            .await
            .map_err(|status| {
                DockerApiError::Internal(format!("failed to list snapshots: {status}"))
            })?;

        use tokio_stream::StreamExt;
        let all_snapshots = stream
            .get_mut()
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .filter_map(|resp| resp.ok())
            .flat_map(|resp| resp.info)
            .collect::<Vec<_>>();

        // Identify committed snapshots that are part of an image layer chain.
        // Image unpack creates committed snapshots whose names are layer
        // digest prefixes (e.g. "sha256:abc...").  The top-most snapshot
        // is the one whose name is NOT referenced as a parent by any other
        // committed snapshot.
        let committed: Vec<_> = all_snapshots
            .iter()
            .filter(|info| {
                info.kind == containerd_client::services::v1::snapshots::Kind::Committed as i32
            })
            .collect();

        let top_key = committed
            .iter()
            .find(|info| !committed.iter().any(|other| other.name == info.parent))
            .map(|info| info.name.clone())
            .ok_or_else(|| {
                DockerApiError::Internal(format!(
                    "no committed snapshots found for {image_ref} ({} total snapshots)",
                    all_snapshots.len(),
                ))
            })?;

        // Update the image with the snapshot key label
        let mut labels = image.labels.clone();
        labels.insert("containerd.io/snapshot/overlayfs.key".into(), top_key);

        let mut img_client = containerd_client::Client::from(self.channel.clone()).images();
        let current = img_client
            .get(with_namespace(
                containerd_client::services::v1::GetImageRequest {
                    name: image.name.clone(),
                },
            ))
            .await
            .map_err(|e| DockerApiError::Internal(format!("re-fetch image: {e}")))?
            .into_inner()
            .image
            .ok_or_else(|| DockerApiError::Internal("image disappeared".into()))?;

        img_client
            .update(with_namespace(
                containerd_client::services::v1::UpdateImageRequest {
                    image: Some(containerd_client::services::v1::Image { labels, ..current }),
                    update_mask: Some(prost_types::FieldMask {
                        paths: vec!["labels".into()],
                    }),
                    source_date_epoch: None,
                },
            ))
            .await
            .map_err(|e| DockerApiError::Internal(format!("update image labels: {e}")))?;

        Ok(())
    }

    pub async fn image_push(
        &self,
        image_ref: &str,
        credentials: Option<RegistryCredentials>,
    ) -> Result<()> {
        if let Some(credentials) = credentials.as_ref() {
            tracing::debug!(username = %credentials.username, server = %credentials.server, "using registry credentials for image push");
        }
        self.image_get(image_ref).await.map(|_| ())
    }

    pub async fn image_list(&self) -> Result<Vec<ImageRecord>> {
        let mut client = containerd_client::Client::from(self.channel.clone()).images();
        let response = client
            .list(with_namespace(ListImagesRequest {
                filters: Vec::new(),
            }))
            .await
            .map_err(map_status)?
            .into_inner();
        Ok(response.images.into_iter().map(image_to_record).collect())
    }

    pub async fn image_get(&self, name: &str) -> Result<ImageRecord> {
        let mut client = containerd_client::Client::from(self.channel.clone()).images();
        let response = client
            .get(with_namespace(GetImageRequest {
                name: name.to_owned(),
            }))
            .await
            .map_err(map_status)?
            .into_inner();
        response
            .image
            .map(image_to_record)
            .ok_or_else(|| DockerApiError::NotFound(format!("image {name}")))
    }

    pub async fn image_delete(&self, name: &str) -> Result<()> {
        let mut client = containerd_client::Client::from(self.channel.clone()).images();
        client
            .delete(with_namespace(DeleteImageRequest {
                name: name.to_owned(),
                sync: true,
                target: None,
            }))
            .await
            .map_err(map_status)?;
        Ok(())
    }

    /// Read stdout/stderr output from a running task as raw bytes.
    ///
    /// The containerd Tasks gRPC service does not expose a streaming read RPC
    /// for task I/O; stdout/stderr are FIFO pipes configured at task-create
    /// time and are not accessible via the API after the fact. This method
    /// returns the current task status as a diagnostic log line instead.
    ///
    /// Returns `DockerApiError::NotFound` if no task exists for
    /// `container_id`.
    pub async fn task_logs(&self, container_id: &str) -> Result<Vec<u8>> {
        let task = self
            .task_get(container_id)
            .await?
            .ok_or_else(|| DockerApiError::NotFound(format!("container {container_id}")))?;
        Ok(format!("container {} status {:?}\n", task.container_id, task.status).into_bytes())
    }
}

fn with_namespace<T>(message: T) -> tonic::Request<T> {
    let mut request = tonic::Request::new(message);
    request.metadata_mut().insert(
        "containerd-namespace",
        NAMESPACE.parse().expect("valid namespace"),
    );
    request
}

fn map_status(status: tonic::Status) -> DockerApiError {
    match status.code() {
        tonic::Code::NotFound => DockerApiError::NotFound(status.message().to_owned()),
        tonic::Code::AlreadyExists | tonic::Code::FailedPrecondition => {
            DockerApiError::Conflict(status.message().to_owned())
        }
        tonic::Code::InvalidArgument => DockerApiError::BadRequest(status.message().to_owned()),
        _ => DockerApiError::Internal(status.to_string()),
    }
}

fn process_to_task_info(process: Process) -> TaskInfo {
    TaskInfo {
        container_id: process.container_id,
        exec_id: process.id,
        pid: process.pid,
        status: match Status::try_from(process.status).unwrap_or(Status::Unknown) {
            Status::Created => TaskStatus::Created,
            Status::Running => TaskStatus::Running,
            Status::Stopped => TaskStatus::Stopped,
            Status::Paused | Status::Pausing => TaskStatus::Paused,
            Status::Unknown => TaskStatus::Unknown,
        },
        exit_status: process.exit_status,
    }
}

fn container_to_info(container: Container) -> ContainerInfo {
    ContainerInfo {
        id: container.id,
        image: container.image,
        labels: container.labels,
        created_at_seconds: container
            .created_at
            .map(|ts| ts.seconds)
            .unwrap_or_default(),
    }
}

fn image_to_record(image: containerd_client::services::v1::Image) -> ImageRecord {
    let target = image.target.as_ref();
    ImageRecord {
        id: target
            .map(|target| target.digest.clone())
            .unwrap_or_else(|| image.name.clone()),
        size: target.map(|target| target.size).unwrap_or_default(),
        created_at_seconds: image.created_at.map(|ts| ts.seconds).unwrap_or_default(),
        name: image.name,
        labels: image.labels,
    }
}
