use std::convert::Infallible;

use axum::Json;
use axum::body::{Body, Bytes};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;

use crate::error::{DockerApiError, Result};
use crate::registry_auth::{RegistryCredentials, decode_registry_auth_header, parse_docker_config};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct ImageCreateQuery {
    #[serde(rename = "fromImage")]
    from_image: String,
    tag: Option<String>,
}

pub async fn image_pull(
    State(state): State<AppState>,
    Query(query): Query<ImageCreateQuery>,
    headers: HeaderMap,
) -> Result<Response> {
    validate_image_ref(&query.from_image)?;
    let image = image_ref_with_tag(&query.from_image, query.tag.as_deref());
    let server = registry_server(&image);
    let credentials = registry_credentials(&headers, &server);
    let client = state.containerd_client().await?;
    client.image_pull(&image, credentials).await?;

    Ok(json_progress_stream(vec![
        json!({"status": format!("Pulling from {image}"), "progressDetail": {}, "id": query.tag.unwrap_or_else(|| "latest".into())}),
        json!({"status": format!("Status: Downloaded newer image for {image}")}),
    ]))
}

pub async fn image_list(State(state): State<AppState>) -> Result<impl IntoResponse> {
    let client = state.containerd_client().await?;
    let images = client
        .image_list()
        .await?
        .into_iter()
        .map(|image| speck_core::image::ImageSummary {
            id: image.id,
            repo_tags: vec![image.name],
            size: image.size,
            created: image.created_at_seconds,
        })
        .collect::<Vec<_>>();
    Ok(Json(images))
}

pub async fn image_inspect(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<impl IntoResponse> {
    let client = state.containerd_client().await?;
    let image = client.image_get(&name).await?;
    Ok(Json(speck_core::image::ImageInspect {
        id: image.id,
        repo_tags: vec![image.name],
        size: image.size,
        created: image.created_at_seconds,
        architecture: "arm64".into(),
    }))
}

pub async fn image_push(
    State(state): State<AppState>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Result<Response> {
    validate_image_ref(&name)?;
    let server = registry_server(&name);
    let credentials = registry_credentials(&headers, &server);
    let client = state.containerd_client().await?;
    client.image_push(&name, credentials).await.map_err(|e| {
        tracing::warn!(%name, error = %e, "image push failed");
        e
    })?;
    Ok(json_progress_stream(vec![
        json!({"status": format!("Pushed {name}")}),
    ]))
}

pub async fn image_remove(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<impl IntoResponse> {
    let client = state.containerd_client().await?;
    client.image_delete(&name).await?;
    Ok(Json(vec![json!({"Untagged": name})]))
}

fn registry_credentials(headers: &HeaderMap, server: &str) -> Option<RegistryCredentials> {
    headers
        .get("X-Registry-Auth")
        .and_then(|value| value.to_str().ok())
        .and_then(decode_registry_auth_header)
        .or_else(|| parse_docker_config(server))
}

fn image_ref_with_tag(from_image: &str, tag: Option<&str>) -> String {
    match tag {
        Some(tag) if !tag.is_empty() && !from_image.contains(':') => format!("{from_image}:{tag}"),
        _ => from_image.to_owned(),
    }
}

fn registry_server(image: &str) -> String {
    let first = image.split('/').next().unwrap_or_default();
    if first.contains('.') || first.contains(':') || first == "localhost" {
        first.to_owned()
    } else {
        "registry-1.docker.io".to_owned()
    }
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

fn json_progress_stream(values: Vec<serde_json::Value>) -> Response {
    let stream = tokio_stream::iter(values.into_iter().map(|value| {
        let mut line = serde_json::to_vec(&value).expect("progress JSON serializes");
        line.push(b'\n');
        Ok::<Bytes, Infallible>(Bytes::from(line))
    }));
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        Body::from_stream(stream),
    )
        .into_response()
}
