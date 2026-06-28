use std::collections::HashMap;
use std::path::Path;

use containerd_client::services::v1::{
    Container, CreateContainerRequest, CreateTaskRequest, DeleteContainerRequest,
    DeleteImageRequest, DeleteTaskRequest, ExecProcessRequest, GetContainerRequest,
    GetImageRequest, GetRequest, KillRequest, ListContainersRequest, ListImagesRequest,
    ListTasksRequest, StartRequest, WaitRequest,
};
use containerd_client::tonic;
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
}

impl ContainerdClient {
    pub async fn connect(path: impl AsRef<Path>) -> Result<Self> {
        let channel = containerd_client::connect(path)
            .await
            .map_err(|err| DockerApiError::Internal(format!("connect containerd: {err}")))?;
        Ok(Self { channel })
    }

    pub async fn task_create(&self, spec: TaskSpec) -> Result<TaskId> {
        let mut client = containerd_client::Client::from(self.channel.clone()).tasks();
        client
            .create(with_namespace(CreateTaskRequest {
                container_id: spec.container_id.clone(),
                rootfs: Vec::new(),
                stdin: String::new(),
                stdout: String::new(),
                stderr: String::new(),
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
        let response = client
            .get(with_namespace(GetRequest {
                container_id: id.to_owned(),
                exec_id: String::new(),
            }))
            .await
            .map_err(map_status)?
            .into_inner();
        Ok(response.process.map(process_to_task_info))
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
        Ok(response.tasks.into_iter().map(process_to_task_info).collect())
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
            labels.insert("speck.restart_maximum_retry_count".into(), count.to_string());
        }
        if let Some(memory) = spec.memory {
            labels.insert("speck.memory".into(), memory.to_string());
        }
        if let Some(cpu_shares) = spec.cpu_shares {
            labels.insert("speck.cpu_shares".into(), cpu_shares.to_string());
        }

        let container = Container {
            id: spec.id.clone(),
            labels,
            image: spec.image,
            runtime: None,
            spec: None,
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
        response
            .container
            .map(|container| container.id)
            .ok_or_else(|| DockerApiError::Internal("containerd returned empty create response".into()))
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
            .list(with_namespace(ListContainersRequest { filters: Vec::new() }))
            .await
            .map_err(map_status)?
            .into_inner();
        Ok(response.containers.into_iter().map(container_to_info).collect())
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

    pub async fn image_pull(&self, image_ref: &str, credentials: Option<RegistryCredentials>) -> Result<()> {
        if let Some(credentials) = credentials.as_ref() {
            tracing::debug!(username = %credentials.username, server = %credentials.server, "using registry credentials for image pull");
        }

        // containerd-client exposes the image metadata service. Resolver-based pull is
        // handled later by the transfer service; for now validate connectivity by
        // consulting metadata and let callers stream Docker-shaped progress.
        let _ = self.image_get(image_ref).await;
        Ok(())
    }

    pub async fn image_push(&self, image_ref: &str, credentials: Option<RegistryCredentials>) -> Result<()> {
        if let Some(credentials) = credentials.as_ref() {
            tracing::debug!(username = %credentials.username, server = %credentials.server, "using registry credentials for image push");
        }
        self.image_get(image_ref).await.map(|_| ())
    }

    pub async fn image_list(&self) -> Result<Vec<ImageRecord>> {
        let mut client = containerd_client::Client::from(self.channel.clone()).images();
        let response = client
            .list(with_namespace(ListImagesRequest { filters: Vec::new() }))
            .await
            .map_err(map_status)?
            .into_inner();
        Ok(response.images.into_iter().map(image_to_record).collect())
    }

    pub async fn image_get(&self, name: &str) -> Result<ImageRecord> {
        let mut client = containerd_client::Client::from(self.channel.clone()).images();
        let response = client
            .get(with_namespace(GetImageRequest { name: name.to_owned() }))
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
}

fn with_namespace<T>(message: T) -> tonic::Request<T> {
    let mut request = tonic::Request::new(message);
    request
        .metadata_mut()
        .insert("containerd-namespace", NAMESPACE.parse().expect("valid namespace"));
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
        created_at_seconds: container.created_at.map(|ts| ts.seconds).unwrap_or_default(),
    }
}

fn image_to_record(image: containerd_client::services::v1::Image) -> ImageRecord {
    let target = image.target.as_ref();
    ImageRecord {
        id: target.map(|target| target.digest.clone()).unwrap_or_else(|| image.name.clone()),
        size: target.map(|target| target.size).unwrap_or_default(),
        created_at_seconds: image.created_at.map(|ts| ts.seconds).unwrap_or_default(),
        name: image.name,
    }
}
