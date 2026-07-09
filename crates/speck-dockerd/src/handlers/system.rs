use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::state::AppState;

pub async fn ping() -> Response {
    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", HeaderValue::from_static("text/plain"));
    headers.insert("Api-Version", HeaderValue::from_static("1.44"));
    headers.insert("Docker-Experimental", HeaderValue::from_static("false"));
    headers.insert("Ostype", HeaderValue::from_static("linux"));

    (StatusCode::OK, headers, "OK").into_response()
}

pub async fn version() -> impl IntoResponse {
    Json(json!({
        "Platform": {"Name": "Speck"},
        "Version": env!("CARGO_PKG_VERSION"),
        "ApiVersion": "1.44",
        "MinAPIVersion": "1.24",
        "GitCommit": "speck",
        "GoVersion": "N/A",
        "Os": "linux",
        "Arch": "arm64",
        "KernelVersion": "5.x",
    }))
}

/// POST /auth — docker login credential check.
///
/// The real credential verification happens at the registry when docker
/// pushes or pulls. This endpoint only needs to acknowledge the request so
/// docker stores the credentials in ~/.docker/config.json.
pub async fn auth() -> impl IntoResponse {
    Json(json!({
        "Status": "Login Succeeded",
        "IdentityToken": ""
    }))
}

pub async fn info(State(state): State<AppState>) -> impl IntoResponse {
    let containers = match state.containerd_client().await {
        Ok(client) => client.container_list().await.unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    let container_count = containers.len();

    Json(json!({
        "ServerVersion": env!("CARGO_PKG_VERSION"),
        "Containers": container_count,
        "ContainersRunning": 0,
        "ContainersPaused": 0,
        "ContainersStopped": container_count,
        "Images": 0,
        "MemTotal": 0,
        "NCPU": 1,
        "OperatingSystem": "Speck VM",
        "OSType": "linux",
        "Architecture": "aarch64",
    }))
}
