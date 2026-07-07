use std::time::{SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use hyper::upgrade;
use hyper_util::rt::TokioIo;
use serde::Deserialize;
use serde_json::json;
use tokio::io::AsyncWriteExt;

use crate::containerd_client::ExecProcessSpec;
use crate::error::{DockerApiError, Result};
use crate::state::{AppState, ExecSpec};
use crate::stream::encode_frame;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ExecCreateBody {
    #[serde(default)]
    pub cmd: Vec<String>,
    #[serde(default)]
    pub attach_stdin: bool,
    #[serde(default)]
    pub attach_stdout: bool,
    #[serde(default)]
    pub attach_stderr: bool,
    #[serde(default)]
    pub tty: bool,
    #[serde(default)]
    pub env: Option<Vec<String>>,
}

pub async fn create(
    State(state): State<AppState>,
    Path(container_id): Path<String>,
    Json(body): Json<ExecCreateBody>,
) -> Result<impl IntoResponse> {
    if body.cmd.is_empty() {
        return Err(DockerApiError::BadRequest(
            "exec Cmd cannot be empty".into(),
        ));
    }

    let exec_id = generated_exec_id(&container_id);
    let spec = ExecSpec {
        id: exec_id.clone(),
        container_id: container_id.clone(),
        cmd: body.cmd,
        env: body.env.unwrap_or_default(),
        attach_stdin: body.attach_stdin,
        attach_stdout: body.attach_stdout,
        attach_stderr: body.attach_stderr,
        tty: body.tty,
        running: false,
        exit_code: None,
    };

    state
        .containerd_client()
        .await?
        .exec_create(ExecProcessSpec {
            container_id,
            exec_id: exec_id.clone(),
            cmd: spec.cmd.clone(),
            env: spec.env.clone(),
            terminal: spec.tty,
        })
        .await?;

    if let Ok(storage) = state.storage.lock() {
        if let Err(e) = storage.save_exec(&spec) {
            tracing::warn!(error = ?e, exec_id = %exec_id, "failed to persist exec session to SQLite");
        }
    }
    state.exec_store.lock().await.insert(exec_id.clone(), spec);

    Ok((StatusCode::CREATED, Json(json!({ "Id": exec_id }))))
}

pub async fn start(
    State(state): State<AppState>,
    Path(id): Path<String>,
    req: Request<Body>,
) -> Result<Response> {
    let upgrade = upgrade::on(req);
    let spec = state
        .exec_store
        .lock()
        .await
        .get(&id)
        .ok_or_else(|| DockerApiError::NotFound(format!("exec {id}")))?;

    state
        .containerd_client()
        .await?
        .exec_start(&spec.container_id, &id)
        .await?;

    let mut running_spec = spec.clone();
    running_spec.running = true;
    state
        .exec_store
        .lock()
        .await
        .update(&id, running_spec.clone());
    if let Ok(storage) = state.storage.lock() {
        if let Err(e) = storage.update_exec(&running_spec) {
            tracing::warn!(error = ?e, exec_id = %running_spec.id, "failed to update exec session in SQLite");
        }
    }

    tokio::spawn(async move {
        match upgrade.await {
            Ok(upgraded) => {
                let mut io = TokioIo::new(upgraded);
                if running_spec.attach_stdout {
                    let payload = Vec::new();
                    let data = if running_spec.tty {
                        payload
                    } else {
                        encode_frame(1, &payload)
                    };
                    let _ = io.write_all(&data).await;
                }
                if running_spec.attach_stderr && !running_spec.tty {
                    let _ = io.write_all(&encode_frame(2, &[])).await;
                }
                let _ = io.shutdown().await;
            }
            Err(err) => {
                tracing::warn!(?err, exec_id = %running_spec.id, "Docker exec HTTP upgrade failed")
            }
        }
    });

    Ok(Response::builder()
        .status(StatusCode::SWITCHING_PROTOCOLS)
        .header("Upgrade", "tcp")
        .header("Connection", "Upgrade")
        .header("Content-Type", "application/vnd.docker.raw-stream")
        .body(Body::empty())
        .expect("valid switching protocols response"))
}

pub async fn inspect(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse> {
    let spec = state
        .exec_store
        .lock()
        .await
        .get(&id)
        .ok_or_else(|| DockerApiError::NotFound(format!("exec {id}")))?;

    Ok(Json(json!({
        "ID": spec.id,
        "Running": spec.running,
        "ExitCode": spec.exit_code,
        "ContainerID": spec.container_id,
        "ProcessConfig": { "cmd": spec.cmd },
    })))
}

fn generated_exec_id(container_id: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("{container_id}-exec-{nanos:x}")
}
