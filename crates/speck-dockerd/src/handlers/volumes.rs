use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;
use serde_json::json;
use speck_core::volume::VolumeSummary;

use crate::error::{DockerApiError, Result};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct VolumeCreateBody {
    name: String,
    driver: Option<String>,
}

pub async fn volume_list(State(state): State<AppState>) -> impl IntoResponse {
    let volumes = state.volume_store.lock().await;
    Json(json!({"Volumes": volumes.values().cloned().collect::<Vec<_>>(), "Warnings": []}))
}

pub async fn volume_create(
    State(state): State<AppState>,
    Json(body): Json<VolumeCreateBody>,
) -> Result<impl IntoResponse> {
    validate_volume_name(&body.name)?;
    let volume = VolumeSummary {
        mountpoint: format!("/var/lib/speck/volumes/{}", body.name),
        name: body.name.clone(),
        driver: body.driver.unwrap_or_else(|| "local".into()),
        created_at: None,
    };
    state.volume_store.lock().await.insert(volume.name.clone(), volume.clone());
    Ok((StatusCode::CREATED, Json(volume)))
}

pub async fn volume_inspect(State(state): State<AppState>, Path(name): Path<String>) -> Result<impl IntoResponse> {
    let volumes = state.volume_store.lock().await;
    let volume = volumes
        .get(&name)
        .cloned()
        .ok_or_else(|| DockerApiError::NotFound(format!("volume {name}")))?;
    Ok(Json(volume))
}

pub async fn volume_remove(State(state): State<AppState>, Path(name): Path<String>) -> StatusCode {
    state.volume_store.lock().await.remove(&name);
    StatusCode::NO_CONTENT
}

fn validate_volume_name(name: &str) -> Result<()> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return Err(DockerApiError::BadRequest("volume name cannot be empty".into()));
    };
    if !first.is_ascii_alphanumeric() {
        return Err(DockerApiError::BadRequest("volume name must start with an ASCII letter or digit".into()));
    }
    if !chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-')) {
        return Err(DockerApiError::BadRequest("volume name contains invalid characters".into()));
    }
    Ok(())
}
