use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::containerd_client::{ContainerCreateSpec, ContainerInfo, TaskSpec, TaskStatus};
use crate::error::{DockerApiError, Result};
use crate::log_relay;
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

    let id = query
        .name
        .unwrap_or_else(|| generated_container_id(&body.image));
    validate_container_name(&id)?;

    let host_config = body.host_config.unwrap_or_default();
    let restart = host_config.restart_policy.unwrap_or_default();
    let mut labels = HashMap::new();
    labels.insert("speck.docker.api_version".into(), "1.44".into());
    labels.insert("speck.tty".into(), body.tty.unwrap_or(false).to_string());

    if let Some(exposed_ports) = body.exposed_ports {
        labels.insert(
            "speck.exposed_ports".into(),
            join_map_keys(exposed_ports.keys()),
        );
    }
    if let Some(port_bindings) = host_config.port_bindings {
        labels.insert(
            "speck.port_bindings".into(),
            serialize_json(&port_bindings)?,
        );
    }
    if let Some(binds) = host_config.binds {
        // Validate each bind string eagerly at create time so malformed or
        // unsafe paths return a 400 error before any label is stored.
        // (Rule 2 / T-07-06: path-traversal and non-existent host paths are
        // rejected here rather than silently at container start time.)
        let validated: Vec<String> = binds
            .iter()
            .map(|s| parse_docker_bind(s).map(|b| b.to_label_string()))
            .collect::<Result<Vec<_>>>()?;
        labels.insert("speck.binds".into(), validated.join("\u{1f}"));
    }

    let image_for_event = body.image.clone();
    // Capture port_bindings_json before labels is moved into ContainerCreateSpec.
    let port_bindings_json: Option<String> = labels.get("speck.port_bindings").cloned();
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

    // Persist container metadata (including port bindings) to SQLite.
    // Storage failures are non-fatal — the container was already created in containerd.
    {
        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs().to_string())
            .unwrap_or_default();
        let create_body_json = format!("{{\"Image\":\"{}\"}}", image_for_event);
        let storage = state.storage.lock().await;
        let _ = storage.save_container_meta(
            &container_id,
            &image_for_event,
            &create_body_json,
            &port_bindings_json,
            &created_at,
        );
    }

    crate::handlers::events::emit_event(
        &state,
        json!({"Type": "container", "Action": "create", "Actor": {"ID": container_id, "Attributes": {"image": image_for_event}}}),
    );

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
    let terminal = info
        .labels
        .get("speck.tty")
        .is_some_and(|value| value == "true");
    let (stdout_fifo, stderr_fifo) = if let Some(port) = state.guest.log_relay_vsock_port() {
        let stdout_path = format!("/tmp/speck-logs/{id}.stdout");
        let stderr_path = format!("/tmp/speck-logs/{id}.stderr");
        match log_relay::create(&state.guest, port, &id) {
            Ok(()) => (Some(stdout_path), Some(stderr_path)),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    container_id = %id,
                    "log relay create failed — logs will be unavailable"
                );
                (None, None)
            }
        }
    } else {
        (None, None)
    };

    let relay_created = stdout_fifo.is_some();

    let task_result = async {
        client
            .task_create(TaskSpec {
                container_id: id.clone(),
                terminal,
                stdout_fifo,
                stderr_fifo,
            })
            .await?;
        client.task_start(&id).await
    }
    .await;
    if let Err(err) = task_result {
        // Relay state was created before task_create — close it so stale
        // guest FIFOs/readers do not leak into a retried start (CR-02).
        if relay_created
            && let Some(port) = state.guest.log_relay_vsock_port()
            && let Err(close_err) = log_relay::close(&state.guest, port, &id)
        {
            tracing::warn!(
                error = %close_err,
                container_id = %id,
                "log relay close failed after task start failure"
            );
        }
        return Err(err);
    }

    // Apply port bindings from the speck.port_bindings label stored during create().
    // Malformed or missing labels are silently skipped — port map failures must not
    // cause the container start to fail.
    if let Some(pb_json) = info.labels.get("speck.port_bindings") {
        match serde_json::from_str::<HashMap<String, Vec<PortBindingBody>>>(pb_json) {
            Err(e) => {
                tracing::warn!(
                    label = pb_json,
                    error = %e,
                    "failed to deserialize speck.port_bindings; skipping port maps"
                );
            }
            Ok(port_bindings) => {
                for (port_proto, host_bindings) in &port_bindings {
                    let cp: u16 = match port_proto.split('/').next().and_then(|s| s.parse().ok()) {
                        Some(p) => p,
                        None => {
                            tracing::warn!(
                                port_proto = port_proto.as_str(),
                                "invalid container port in speck.port_bindings; skipping"
                            );
                            continue;
                        }
                    };
                    for hb in host_bindings {
                        let Some(hp_str) = hb.host_port.as_deref() else {
                            continue;
                        };
                        let hp: u16 = match hp_str.parse() {
                            Ok(p) => p,
                            Err(_) => {
                                tracing::warn!(
                                    host_port = hp_str,
                                    container_port = cp,
                                    "invalid host port in speck.port_bindings; skipping"
                                );
                                continue;
                            }
                        };
                        if let Err(e) = state.guest.add_port_map(hp, cp) {
                            tracing::warn!(
                                host_port = hp,
                                container_port = cp,
                                error = %e,
                                "failed to add port map"
                            );
                        }
                    }
                }
            }
        }
    }

    // Apply bind mounts from the speck.binds label stored during create() (D-05).
    // The label holds '\u{1f}'-delimited bind strings validated at create time.
    // Malformed or missing labels are silently skipped — bind mount failures
    // must not cause the container start to fail.
    if let Some(binds_label) = info.labels.get("speck.binds") {
        let mounts: Vec<speck_vz::config::VolumeMountConfig> = binds_label
            .split('\u{1f}')
            .filter(|s| !s.is_empty())
            .filter_map(|s| match parse_docker_bind(s) {
                Ok(b) => Some(speck_vz::config::VolumeMountConfig {
                    host_path: b.host_path,
                    container_path: b.container_path,
                    read_only: b.read_only,
                    volume_name: None,
                }),
                Err(e) => {
                    tracing::warn!(
                        bind = s,
                        error = %e,
                        "invalid bind string in speck.binds label; skipping"
                    );
                    None
                }
            })
            .collect();

        if !mounts.is_empty()
            && let Err(e) = state.guest.add_bind_mounts(mounts)
        {
            tracing::warn!(
                container_id = id.as_str(),
                error = %e,
                "failed to update virtiofs-binds VZMultipleDirectoryShare; bind mounts unavailable"
            );
        }
    }

    crate::handlers::events::emit_event(
        &state,
        json!({"Type": "container", "Action": "start", "Actor": {"ID": id}}),
    );
    Ok(StatusCode::NO_CONTENT)
}

pub async fn stop(State(state): State<AppState>, Path(id): Path<String>) -> Result<StatusCode> {
    let client = state.containerd_client().await?;
    client.task_stop(&id).await?;
    let _ = client.task_wait(&id).await;
    crate::handlers::events::emit_event(
        &state,
        json!({"Type": "container", "Action": "stop", "Actor": {"ID": id}}),
    );
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

pub async fn wait(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse> {
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
    let task = client
        .task_get(&id)
        .await?
        .unwrap_or_else(|| stopped_task(&id));
    let port_bindings = {
        let storage = state.storage.lock().await;
        storage.load_port_bindings(&id).unwrap_or(None)
    };
    Ok(Json(container_inspect_json(info, task, port_bindings)))
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
    if let Some(port) = state.guest.log_relay_vsock_port() {
        let _ = log_relay::close(&state.guest, port, &id);
    }
    client.container_delete(&id).await?;
    crate::handlers::events::emit_event(
        &state,
        json!({"Type": "container", "Action": "destroy", "Actor": {"ID": id}}),
    );
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
    crate::error::DockerApiError::Internal("route should be handled by exec::create".into())
        .into_response()
}

/// A validated Docker bind mount specification parsed from a `host:container[:mode]` string.
///
/// Mode is optional; `ro` means read-only, `rw` (or no mode) means read-write.
/// Anonymous volumes (no host path) are not supported and will be rejected during create.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DockerBind {
    /// Absolute path on the host filesystem (must exist at create time).
    pub(crate) host_path: PathBuf,
    /// Absolute mount path inside the container (no `..` allowed).
    pub(crate) container_path: PathBuf,
    /// True if the mount should be read-only.
    pub(crate) read_only: bool,
}

impl DockerBind {
    /// Serialize back to the canonical label-storage form (`host:container` or
    /// `host:container:ro`).  The `\u{1f}` join delimiter is applied by the caller.
    fn to_label_string(&self) -> String {
        let host = self.host_path.display();
        let container = self.container_path.display();
        if self.read_only {
            format!("{host}:{container}:ro")
        } else {
            format!("{host}:{container}")
        }
    }
}

/// Parse and validate a single Docker bind-mount string.
///
/// Accepted formats:
/// - `host_path:container_path`       — read-write
/// - `host_path:container_path:ro`    — read-only
/// - `host_path:container_path:rw`    — read-write (explicit)
///
/// Errors (HTTP 400):
/// - Empty host or container path.
/// - Container path is not absolute (does not start with `/`).
/// - Container path contains `..`.
/// - Host path is not absolute.
/// - Host path does not exist on the host filesystem.
/// - Unknown mount mode.
fn parse_docker_bind(s: &str) -> Result<DockerBind> {
    // Split into at most 3 parts: host, container, optional mode.
    let parts: Vec<&str> = s.splitn(3, ':').collect();
    let (host, container, read_only) = match parts.as_slice() {
        [host, container] => (*host, *container, false),
        [host, container, mode] => {
            let ro = match *mode {
                "ro" => true,
                "rw" => false,
                other => {
                    return Err(DockerApiError::BadRequest(format!(
                        "invalid bind mount mode '{other}'; expected 'ro' or 'rw'"
                    )));
                }
            };
            (*host, *container, ro)
        }
        _ => {
            return Err(DockerApiError::BadRequest(
                "bind mount must be in format 'host_path:container_path[:ro|:rw]'".into(),
            ));
        }
    };

    if host.is_empty() {
        return Err(DockerApiError::BadRequest(
            "bind host path cannot be empty".into(),
        ));
    }

    if container.is_empty() {
        return Err(DockerApiError::BadRequest(
            "bind container path cannot be empty".into(),
        ));
    }

    let container_path = std::path::Path::new(container);
    if !container_path.is_absolute() {
        return Err(DockerApiError::BadRequest(format!(
            "bind container path must be absolute, got: {container}"
        )));
    }

    // Reject path traversal via `..` components.
    if container.contains("..") {
        return Err(DockerApiError::BadRequest(format!(
            "bind container path must not contain '..': {container}"
        )));
    }

    let host_path = PathBuf::from(host);
    if !host_path.is_absolute() {
        return Err(DockerApiError::BadRequest(format!(
            "bind host path must be absolute, got: {host}"
        )));
    }

    if !host_path.exists() {
        return Err(DockerApiError::BadRequest(format!(
            "bind host path does not exist: {host}"
        )));
    }

    Ok(DockerBind {
        host_path,
        container_path: container_path.to_owned(),
        read_only,
    })
}

fn validate_container_name(name: &str) -> Result<()> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return Err(DockerApiError::BadRequest(
            "container name cannot be empty".into(),
        ));
    };
    if !first.is_ascii_alphanumeric() {
        return Err(DockerApiError::BadRequest(
            "container name must start with an ASCII letter or digit".into(),
        ));
    }
    if !chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-')) {
        return Err(DockerApiError::BadRequest(
            "container name contains invalid characters".into(),
        ));
    }
    Ok(())
}

fn validate_image_ref(image: &str) -> Result<()> {
    if image.trim().is_empty() {
        return Err(DockerApiError::BadRequest(
            "image reference cannot be empty".into(),
        ));
    }
    if image
        .chars()
        .any(|ch| matches!(ch, ';' | '&' | '|' | '`' | '$' | '<' | '>' | '\n' | '\r'))
    {
        return Err(DockerApiError::BadRequest(
            "image reference contains shell metacharacters".into(),
        ));
    }
    Ok(())
}

fn parse_signal(signal: &str) -> Result<u32> {
    match signal.trim().to_ascii_uppercase().as_str() {
        "SIGTERM" | "TERM" | "15" => Ok(15),
        "SIGKILL" | "KILL" | "9" => Ok(9),
        "SIGINT" | "INT" | "2" => Ok(2),
        other => Err(DockerApiError::BadRequest(format!(
            "unsupported signal {other}"
        ))),
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
    format!(
        "{}-{nanos:x}",
        if safe_image.is_empty() {
            "speck"
        } else {
            &safe_image
        }
    )
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

fn container_inspect_json(
    container: ContainerInfo,
    task: crate::containerd_client::TaskInfo,
    port_bindings: Option<HashMap<String, Vec<PortBindingBody>>>,
) -> Value {
    let running = task.status == TaskStatus::Running;
    let ports: serde_json::Map<String, Value> = match port_bindings {
        Some(pb) => pb
            .into_iter()
            .map(|(k, v)| {
                let bindings = v
                    .into_iter()
                    .map(|binding| {
                        json!({
                            "HostIp": binding.host_ip.filter(|ip| !ip.is_empty()).unwrap_or_else(|| "0.0.0.0".to_string()),
                            "HostPort": binding.host_port.unwrap_or_default(),
                        })
                    })
                    .collect();
                (k, Value::Array(bindings))
            })
            .collect(),
        None => serde_json::Map::new(),
    };
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
        "NetworkSettings": { "Ports": Value::Object(ports) },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::containerd_client::TaskInfo;

    // ---- parse_docker_bind: success cases ----

    #[test]
    fn test_containers_bind_rw_default() {
        // /tmp always exists on macOS; read-write is the default when no mode is given.
        let b = parse_docker_bind("/tmp:/app").expect("valid rw bind");
        assert_eq!(b.host_path, PathBuf::from("/tmp"));
        assert_eq!(b.container_path, PathBuf::from("/app"));
        assert!(!b.read_only, "no mode suffix → read-write");
    }

    #[test]
    fn test_containers_bind_ro_explicit() {
        let b = parse_docker_bind("/tmp:/data:ro").expect("valid ro bind");
        assert!(b.read_only, ":ro suffix → read-only");
        assert_eq!(b.container_path, PathBuf::from("/data"));
    }

    #[test]
    fn test_containers_bind_rw_explicit() {
        let b = parse_docker_bind("/tmp:/data:rw").expect("valid rw bind");
        assert!(!b.read_only, ":rw suffix → read-write");
    }

    #[test]
    fn test_containers_bind_label_string_rw() {
        let b = DockerBind {
            host_path: PathBuf::from("/tmp"),
            container_path: PathBuf::from("/app"),
            read_only: false,
        };
        assert_eq!(b.to_label_string(), "/tmp:/app");
    }

    #[test]
    fn test_containers_bind_label_string_ro() {
        let b = DockerBind {
            host_path: PathBuf::from("/tmp"),
            container_path: PathBuf::from("/data"),
            read_only: true,
        };
        assert_eq!(b.to_label_string(), "/tmp:/data:ro");
    }

    // ---- parse_docker_bind: rejection cases ----

    #[test]
    fn test_containers_bind_empty_string_rejected() {
        let err = parse_docker_bind("").unwrap_err();
        assert!(
            matches!(err, DockerApiError::BadRequest(_)),
            "empty bind must be 400 bad request"
        );
    }

    #[test]
    fn test_containers_bind_empty_host_rejected() {
        let err = parse_docker_bind(":/app").unwrap_err();
        assert!(
            matches!(err, DockerApiError::BadRequest(_)),
            "empty host path must be 400 bad request"
        );
    }

    #[test]
    fn test_containers_bind_empty_container_rejected() {
        let err = parse_docker_bind("/tmp:").unwrap_err();
        assert!(
            matches!(err, DockerApiError::BadRequest(_)),
            "empty container path must be 400 bad request"
        );
    }

    #[test]
    fn test_containers_bind_relative_container_rejected() {
        let err = parse_docker_bind("/tmp:relative/path").unwrap_err();
        assert!(
            matches!(err, DockerApiError::BadRequest(_)),
            "relative container path must be 400 bad request"
        );
    }

    #[test]
    fn test_containers_bind_dotdot_container_rejected() {
        // Container path containing .. is a path-traversal risk.
        let err = parse_docker_bind("/tmp:/app/../etc").unwrap_err();
        assert!(
            matches!(err, DockerApiError::BadRequest(_)),
            "container path with .. must be 400 bad request"
        );
    }

    #[test]
    fn test_containers_bind_nonexistent_host_rejected() {
        let err = parse_docker_bind("/nonexistent/path/xyz:/app").unwrap_err();
        assert!(
            matches!(err, DockerApiError::BadRequest(_)),
            "non-existent host path must be 400 bad request"
        );
    }

    #[test]
    fn test_containers_bind_relative_existing_host_rejected() {
        let err = parse_docker_bind(".:/app").unwrap_err();
        assert!(
            matches!(err, DockerApiError::BadRequest(_)),
            "relative host path must be 400 bad request even when it exists"
        );
    }

    #[test]
    fn test_containers_bind_unknown_mode_rejected() {
        let err = parse_docker_bind("/tmp:/app:shared").unwrap_err();
        assert!(
            matches!(err, DockerApiError::BadRequest(_)),
            "unknown mount mode must be 400 bad request"
        );
    }

    // ---- no anonymous volumes ----

    #[test]
    fn test_containers_bind_volume_name_only_rejected() {
        // An anonymous volume (no host path, just a name like "myvolume:/app")
        // with a relative host part must be rejected. Absolute paths only.
        let err = parse_docker_bind("myvolume:/app").unwrap_err();
        assert!(
            matches!(err, DockerApiError::BadRequest(_)),
            "anonymous volume (non-existent relative host) must be rejected"
        );
    }

    // ── GAP-04 / D-05 regression guards ─────────────────────────────────

    /// Verifies the label round-trip used by the start handler (D-05).
    ///
    /// The create handler stores validated binds as `\u{1f}`-delimited label
    /// strings.  The start handler re-parses them before calling
    /// `guest.add_bind_mounts`.  This test guards the contract between the
    /// two phases.
    #[test]
    fn test_binds_label_roundtrip_for_start_handler() {
        // Build the label value exactly as create() does.
        let raw = ["/tmp:/app", "/tmp:/data:ro"];
        let stored: String = raw
            .iter()
            .map(|s| parse_docker_bind(s).unwrap().to_label_string())
            .collect::<Vec<_>>()
            .join("\u{1f}");

        // Re-parse exactly as start() does.
        let reparsed: Vec<DockerBind> = stored
            .split('\u{1f}')
            .filter(|s| !s.is_empty())
            .filter_map(|s| parse_docker_bind(s).ok())
            .collect();

        assert_eq!(reparsed.len(), 2, "both binds must survive the round-trip");
        assert_eq!(reparsed[0].container_path, std::path::PathBuf::from("/app"));
        assert!(!reparsed[0].read_only, "first bind must be read-write");
        assert_eq!(
            reparsed[1].container_path,
            std::path::PathBuf::from("/data")
        );
        assert!(reparsed[1].read_only, "second bind must be read-only");
    }

    #[test]
    fn test_inspect_ports_defaults_omitted_host_ip() {
        let container = ContainerInfo {
            id: "cid".into(),
            image: "alpine".into(),
            labels: HashMap::new(),
            created_at_seconds: 0,
        };
        let mut port_bindings = HashMap::new();
        port_bindings.insert(
            "80/tcp".into(),
            vec![PortBindingBody {
                host_ip: None,
                host_port: Some("18081".into()),
            }],
        );

        let json = container_inspect_json(container, stopped_task("cid"), Some(port_bindings));

        assert_eq!(
            json["NetworkSettings"]["Ports"]["80/tcp"][0]["HostIp"],
            Value::String("0.0.0.0".into())
        );
        assert_eq!(
            json["NetworkSettings"]["Ports"]["80/tcp"][0]["HostPort"],
            Value::String("18081".into())
        );
    }

    #[test]
    fn test_inspect_ports_defaults_empty_host_ip() {
        let container = ContainerInfo {
            id: "cid".into(),
            image: "alpine".into(),
            labels: HashMap::new(),
            created_at_seconds: 0,
        };
        let mut port_bindings = HashMap::new();
        port_bindings.insert(
            "80/tcp".into(),
            vec![PortBindingBody {
                host_ip: Some("".into()),
                host_port: Some("18081".into()),
            }],
        );

        let json = container_inspect_json(container, stopped_task("cid"), Some(port_bindings));

        assert_eq!(
            json["NetworkSettings"]["Ports"]["80/tcp"][0]["HostIp"],
            Value::String("0.0.0.0".into())
        );
        assert_eq!(
            json["NetworkSettings"]["Ports"]["80/tcp"][0]["HostPort"],
            Value::String("18081".into())
        );
    }

    #[test]
    fn test_inspect_ports_preserves_explicit_host_ip() {
        let container = ContainerInfo {
            id: "cid".into(),
            image: "alpine".into(),
            labels: HashMap::new(),
            created_at_seconds: 0,
        };
        let task = TaskInfo {
            container_id: "cid".into(),
            exec_id: String::new(),
            pid: 0,
            status: TaskStatus::Stopped,
            exit_status: 0,
        };
        let mut port_bindings = HashMap::new();
        port_bindings.insert(
            "80/tcp".into(),
            vec![PortBindingBody {
                host_ip: Some("127.0.0.1".into()),
                host_port: Some("18081".into()),
            }],
        );

        let json = container_inspect_json(container, task, Some(port_bindings));

        assert_eq!(
            json["NetworkSettings"]["Ports"]["80/tcp"][0]["HostIp"],
            Value::String("127.0.0.1".into())
        );
        assert_eq!(
            json["NetworkSettings"]["Ports"]["80/tcp"][0]["HostPort"],
            Value::String("18081".into())
        );
    }

    #[test]
    fn test_inspect_ports_empty_when_none() {
        let container = ContainerInfo {
            id: "cid".into(),
            image: "alpine".into(),
            labels: HashMap::new(),
            created_at_seconds: 0,
        };

        let json = container_inspect_json(container, stopped_task("cid"), None);

        assert_eq!(
            json["NetworkSettings"]["Ports"],
            Value::Object(serde_json::Map::new())
        );
    }
}
