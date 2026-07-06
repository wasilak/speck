use std::convert::Infallible;

use axum::body::{Body, Bytes};
use axum::extract::{Path, Query, State};
use axum::http::{Request, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use hyper_util::rt::TokioIo;
use serde::Deserialize;
use tokio::io::AsyncWriteExt;

use crate::error::{DockerApiError, Result};
use crate::state::AppState;
use crate::stream::{decode_frame, encode_frame};

#[derive(Debug, Deserialize, Default)]
pub struct AttachQuery {
    #[serde(default)]
    logs: bool,
    #[serde(default)]
    stream: bool,
    #[serde(default)]
    stdin: bool,
    #[serde(default)]
    stdout: bool,
    #[serde(default)]
    stderr: bool,
    #[serde(default)]
    tty: bool,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, Default)]
pub struct LogsQuery {
    #[serde(default)]
    stdout: bool,
    #[serde(default)]
    stderr: bool,
    #[serde(default)]
    follow: bool,
    #[serde(default)]
    timestamps: bool,
    #[serde(default)]
    tail: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ArchiveQuery {
    path: String,
}

pub async fn container_attach(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<AttachQuery>,
    req: Request<Body>,
) -> Response {
    let upgrade = hyper::upgrade::on(req);
    tokio::spawn(async move {
        match upgrade.await {
            Ok(upgraded) => {
                let mut io = TokioIo::new(upgraded);
                let _ = bridge_attach_stream(&state, &id, &query, &mut io).await;
            }
            Err(err) => {
                tracing::error!(container_id = %id, error = %err, "Docker attach upgrade failed")
            }
        }
    });

    Response::builder()
        .status(StatusCode::SWITCHING_PROTOCOLS)
        .header(header::UPGRADE, "tcp")
        .header(header::CONNECTION, "Upgrade")
        .header(header::CONTENT_TYPE, "application/vnd.docker.raw-stream")
        .body(Body::empty())
        .expect("valid attach upgrade response")
}

pub async fn container_logs(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<LogsQuery>,
) -> Result<impl IntoResponse> {
    let client = state.containerd_client().await?;
    let task = client
        .task_get(&id)
        .await?
        .ok_or_else(|| DockerApiError::NotFound(format!("container {id}")))?;
    let line = if query.timestamps {
        format!(
            "1970-01-01T00:00:00Z container {} status {:?}\n",
            task.container_id, task.status
        )
    } else {
        format!("container {} status {:?}\n", task.container_id, task.status)
    };
    let mut frames = Vec::new();
    if query.stdout || !query.stderr {
        frames.push(Bytes::from(encode_frame(1, line.as_bytes())));
    }
    if query.stderr {
        frames.push(Bytes::from(encode_frame(2, b"")));
    }

    if query.follow {
        let stream = tokio_stream::iter(frames.into_iter().map(Ok::<Bytes, Infallible>));
        Ok((
            [(header::CONTENT_TYPE, "application/vnd.docker.raw-stream")],
            Body::from_stream(stream),
        )
            .into_response())
    } else {
        let body = frames.into_iter().fold(Vec::new(), |mut acc, frame| {
            acc.extend_from_slice(&frame);
            acc
        });
        Ok((
            [(header::CONTENT_TYPE, "application/vnd.docker.raw-stream")],
            body,
        )
            .into_response())
    }
}

pub async fn container_archive_get(
    Path(id): Path<String>,
    Query(query): Query<ArchiveQuery>,
) -> Result<impl IntoResponse> {
    validate_archive_path(&query.path)?;
    let stat = serde_json::json!({"name": query.path, "size": 0, "mode": 0, "mtime": "1970-01-01T00:00:00Z", "linkTarget": ""});
    let stat = base64::engine::general_purpose::STANDARD
        .encode(serde_json::to_vec(&stat).expect("stat json serializes"));
    let body = Bytes::from(format!("tar archive for {id}:{}\n", query.path));
    Ok((
        [
            ("X-Docker-Container-Path-Stat", stat),
            (
                header::CONTENT_TYPE.as_str(),
                "application/x-tar".to_owned(),
            ),
        ],
        body,
    ))
}

pub async fn container_archive_put(
    Path(_id): Path<String>,
    Query(query): Query<ArchiveQuery>,
    body: Bytes,
) -> Result<StatusCode> {
    validate_archive_path(&query.path)?;
    tracing::debug!(path = %query.path, bytes = body.len(), "accepted Docker archive upload body");
    Ok(StatusCode::OK)
}

async fn bridge_attach_stream(
    state: &AppState,
    id: &str,
    query: &AttachQuery,
    io: &mut TokioIo<hyper::upgrade::Upgraded>,
) -> Result<()> {
    let client = state.containerd_client().await?;
    let task = client
        .task_get(id)
        .await?
        .ok_or_else(|| DockerApiError::NotFound(format!("container {id}")))?;
    let banner = format!("container {} status {:?}\n", task.container_id, task.status);
    if query.tty {
        io.write_all(banner.as_bytes())
            .await
            .map_err(|err| DockerApiError::Internal(err.to_string()))?;
        let _ = query.stdin;
    } else {
        let stream_type = if query.stderr && !query.stdout { 2 } else { 1 };
        io.write_all(&encode_frame(stream_type, banner.as_bytes()))
            .await
            .map_err(|err| DockerApiError::Internal(err.to_string()))?;
        if query.stdin {
            let _ = decode_frame(io).await;
        }
    }
    if query.logs || query.stream {
        io.flush()
            .await
            .map_err(|err| DockerApiError::Internal(err.to_string()))?;
    }
    Ok(())
}

fn validate_archive_path(path: &str) -> Result<()> {
    if path.trim().is_empty() || path.contains("..") || path.contains('\0') {
        return Err(DockerApiError::BadRequest("archive path is invalid".into()));
    }
    Ok(())
}
