use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::containerd_client::{ContainerCreateSpec, ContainerInfo, TaskSpec, TaskStatus};
use crate::error::{DockerApiError, Result};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct CreateQuery {
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ContainerCreateBody {
    pub image: String,
    #[serde(default)]
    pub cmd: Option<Vec<String>>,
    #[serde(default)]
    pub env: Option<Vec<String>>,
    #[serde(default)]
    pub exposed_ports: Option<HashMap<String, Value>>,
    #[serde(default)]
    pub host_config: Option<HostConfig>,
    #[serde(default)]
    pub tty: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct HostConfig {
    #[serde(default)]
    pub port_bindings: Option<HashMap<String, Vec<PortBindingBody>>>,
    #[serde(default)]
    pub memory: Option<i64>,
    #[serde(default)]
    pub cpu_shares: Option<i64>,
    #[serde(default)]
    pub restart_policy: Option<RestartPolicy>,
    #[serde(default)]
    pub binds: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct PortBindingBody {
    #[serde(default)]
    pub host_ip: Option<String>,
    #[serde(default)]
    pub host_port: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct RestartPolicy {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub maximum_retry_count: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub all: bool,
}

#[derive(Debug, Deserialize)]
pub struct KillQuery {
    #[serde(default)]
    pub signal: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct CreateResponse {
    id: String,
    warnings: Vec<String>,
}

pub async fn create(
    State(state): State<AppState>,
    Query(query): Query<CreateQuery>,
    Json(body): Json<ContainerCreateBody>,
) -> Result<impl IntoResponse> {
    validate_image_ref(&body.image)?;
    if let Some(name) = query.name.as_deref() {
        validate_container_name(name)?;
    }

    let id = query.name.unwrap_or_else(|| generated_container_id(&body.image));
    validate_container_name(&id)?;

    let host_config = body.host_config.unwrap_or_default();
    let restart = host_config.restart_policy.unwrap_or_default();
    let mut labels = HashMap::new();
    labels.insert("speck.docker.api_version".into(), "1.44".into());
    labels.insert("speck.tty".into(), body.tty.unwrap_or(false).to_string());

    if let Some(exposed_ports) = body.exposed_ports {
        labels.insert("speck.exposed_ports".into(), join_map_keys(exposed_ports.keys()));
    }
    if let Some(port_bindings) = host_config.port_bindings {
        labels.insert("speck.port_bindings".into(), serialize_json(&port_bindings)?);
    }
    if let Some(binds) = host_config.binds {
        labels.insert("speck.binds".into(), binds.join("\u{1f}"));
    }

    let client = state.containerd_client().await?;
    let container_id = client
        .container_create(ContainerCreateSpec {
            id,
            image: body.image,
            cmd: body.cmd.unwrap_or_default(),
            env: body.env.unwrap_or_default(),
            labels,
            restart_policy: restart.name,
            restart_maximum_retry_count: restart.maximum_retry_count,
            memory: host_config.memory,
            cpu_shares: host_config.cpu_shares,
        })
        .await?;

    Ok((
        StatusCode::CREATED,
        Json(CreateResponse {
            id: container_id,
            warnings: Vec::new(),
        }),
    ))
}

pub async fn start(State(state): State<AppState>, Path(id): Path<String>) -> Result<StatusCode> {
    let client = state.containerd_client().await?;
    let info = client.container_get(&id).await?;
    let terminal = info.labels.get("speck.tty").is_some_and(|value| value == "true");
    client
        .task_create(TaskSpec {
            container_id: id.clone(),
            terminal,
        })
        .await?;
    client.task_start(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn stop(State(state): State<AppState>, Path(id): Path<String>) -> Result<StatusCode> {
    let client = state.containerd_client().await?;
    client.task_stop(&id).await?;
    let _ = client.task_wait(&id).await;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn kill(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<KillQuery>,
) -> Result<StatusCode> {
    let signal = parse_signal(query.signal.as_deref().unwrap_or("SIGTERM"))?;
    let client = state.containerd_client().await?;
    client.task_kill(&id, signal).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn wait(State(state): State<AppState>, Path(id): Path<String>) -> Result<impl IntoResponse> {
    let client = state.containerd_client().await?;
    let exit_code = client.task_wait(&id).await?;
    Ok(Json(json!({ "StatusCode": exit_code })))
}

pub async fn inspect(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse> {
    let client = state.containerd_client().await?;
    let info = client.container_get(&id).await?;
    let task = client.task_get(&id).await?.unwrap_or_else(|| stopped_task(&id));
    Ok(Json(container_inspect_json(info, task)))
}

pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> Result<impl IntoResponse> {
    let client = state.containerd_client().await?;
    let containers = client.container_list().await?;
    let running_ids: HashSet<String> = client
        .task_list()
        .await?
        .into_iter()
        .filter(|task| task.status == TaskStatus::Running)
        .map(|task| task.container_id)
        .collect();

    let summaries: Vec<Value> = containers
        .into_iter()
        .filter(|container| query.all || running_ids.contains(&container.id))
        .map(|container| container_summary_json(container, &running_ids))
        .collect();

    Ok(Json(summaries))
}

pub async fn remove(State(state): State<AppState>, Path(id): Path<String>) -> Result<StatusCode> {
    let client = state.containerd_client().await?;
    let _ = client.task_kill(&id, 15).await;
    let _ = client.task_delete(&id).await;
    client.container_delete(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn logs() -> StatusCode {
    StatusCode::NOT_IMPLEMENTED
}

pub async fn archive() -> StatusCode {
    StatusCode::NOT_IMPLEMENTED
}

pub async fn put_archive() -> StatusCode {
    StatusCode::NOT_IMPLEMENTED
}

pub async fn exec() -> Response {
    crate::error::DockerApiError::Internal("route should be handled by exec::create".into()).into_response()
}

fn validate_container_name(name: &str) -> Result<()> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return Err(DockerApiError::BadRequest("container name cannot be empty".into()));
    };
    if !first.is_ascii_alphanumeric() {
        return Err(DockerApiError::BadRequest("container name must start with an ASCII letter or digit".into()));
    }
    if !chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-')) {
        return Err(DockerApiError::BadRequest("container name contains invalid characters".into()));
    }
    Ok(())
}

fn validate_image_ref(image: &str) -> Result<()> {
    if image.trim().is_empty() {
        return Err(DockerApiError::BadRequest("image reference cannot be empty".into()));
    }
    if image.chars().any(|ch| matches!(ch, ';' | '&' | '|' | '`' | '$' | '<' | '>' | '\n' | '\r')) {
        return Err(DockerApiError::BadRequest("image reference contains shell metacharacters".into()));
    }
    Ok(())
}

fn parse_signal(signal: &str) -> Result<u32> {
    match signal.trim().to_ascii_uppercase().as_str() {
        "SIGTERM" | "TERM" | "15" => Ok(15),
        "SIGKILL" | "KILL" | "9" => Ok(9),
        "SIGINT" | "INT" | "2" => Ok(2),
        other => Err(DockerApiError::BadRequest(format!("unsupported signal {other}"))),
    }
}

fn generated_container_id(image: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let safe_image = image
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .take(12)
        .collect::<String>();
    format!("{}-{nanos:x}", if safe_image.is_empty() { "speck" } else { &safe_image })
}

fn join_map_keys<'a>(keys: impl Iterator<Item = &'a String>) -> String {
    keys.cloned().collect::<Vec<_>>().join(",")
}

fn serialize_json<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(|err| DockerApiError::BadRequest(err.to_string()))
}

fn stopped_task(id: &str) -> crate::containerd_client::TaskInfo {
    crate::containerd_client::TaskInfo {
        container_id: id.to_owned(),
        exec_id: String::new(),
        pid: 0,
        status: TaskStatus::Stopped,
        exit_status: 0,
    }
}

fn container_summary_json(container: ContainerInfo, running_ids: &HashSet<String>) -> Value {
    let running = running_ids.contains(&container.id);
    json!({
        "Id": container.id,
        "Names": [format!("/{}", container.labels.get("speck.name").cloned().unwrap_or_else(|| "speck".into()))],
        "Image": container.image,
        "State": if running { "running" } else { "exited" },
        "Status": if running { "Up" } else { "Exited" },
        "Created": container.created_at_seconds,
        "Ports": [],
    })
}

fn container_inspect_json(container: ContainerInfo, task: crate::containerd_client::TaskInfo) -> Value {
    let running = task.status == TaskStatus::Running;
    json!({
        "Id": container.id,
        "Name": format!("/{}", container.labels.get("speck.name").cloned().unwrap_or_else(|| "speck".into())),
        "Image": container.image,
        "State": {
            "Status": if running { "running" } else { "exited" },
            "Running": running,
            "Pid": task.pid,
            "ExitCode": task.exit_status,
        },
        "Mounts": [],
        "NetworkSettings": { "Ports": {} },
    })
}
