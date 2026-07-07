use std::time::{SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;
use serde_json::json;
use speck_core::network::NetworkSummary;

use crate::error::{DockerApiError, Result};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct NetworkCreateBody {
    name: String,
    driver: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct NetworkConnectBody {
    container: Option<String>,
}

pub async fn network_list(State(state): State<AppState>) -> impl IntoResponse {
    let networks = state.network_store.lock().await;
    Json(networks.values().cloned().collect::<Vec<_>>())
}

pub async fn network_create(
    State(state): State<AppState>,
    Json(body): Json<NetworkCreateBody>,
) -> Result<impl IntoResponse> {
    validate_network_name(&body.name)?;
    let id = format!("{}-{:x}", body.name, now_nanos());
    let network = NetworkSummary {
        id: id.clone(),
        name: body.name,
        driver: body.driver.unwrap_or_else(|| "bridge".into()),
        scope: "local".into(),
    };
    state
        .network_store
        .lock()
        .await
        .insert(id.clone(), network.clone());
    if let Ok(storage) = state.storage.lock() {
        if let Err(e) = storage.save_network(&network) {
            tracing::warn!(error = ?e, network = %network.name, "failed to persist network to SQLite");
        }
    }
    crate::handlers::events::emit_event(
        &state,
        json!({"Type": "network", "Action": "create", "Actor": {"ID": id, "Attributes": {"name": network.name}}}),
    );
    Ok((
        StatusCode::CREATED,
        Json(json!({"Id": network.id, "Warning": ""})),
    ))
}

pub async fn network_inspect(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse> {
    let networks = state.network_store.lock().await;
    let network = networks
        .get(&id)
        .or_else(|| networks.values().find(|network| network.name == id))
        .cloned()
        .ok_or_else(|| DockerApiError::NotFound(format!("network {id}")))?;
    Ok(Json(network))
}

pub async fn network_remove(State(state): State<AppState>, Path(id): Path<String>) -> StatusCode {
    let mut networks = state.network_store.lock().await;
    let remove_id = networks
        .contains_key(&id)
        .then_some(id.clone())
        .or_else(|| {
            networks
                .iter()
                .find_map(|(key, network)| (network.name == id).then(|| key.clone()))
        });
    if let Some(ref remove_id) = remove_id {
        networks.remove(remove_id);
    }
    drop(networks);
    if let Some(remove_id) = remove_id {
        if let Ok(storage) = state.storage.lock() {
            if let Err(e) = storage.delete_network(&remove_id) {
                tracing::warn!(error = ?e, network = %remove_id, "failed to delete network from SQLite");
            }
        }
    }
    StatusCode::NO_CONTENT
}

pub async fn network_connect(
    State(_state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<NetworkConnectBody>,
) -> Result<StatusCode> {
    validate_network_name(&id)?;
    let _ = body.container;
    Ok(StatusCode::OK)
}

pub async fn network_disconnect(Path(id): Path<String>) -> Result<StatusCode> {
    validate_network_name(&id)?;
    Ok(StatusCode::OK)
}

fn validate_network_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        return Err(DockerApiError::BadRequest(
            "network name cannot be empty".into(),
        ));
    }
    if name.chars().any(|ch| {
        matches!(
            ch,
            '/' | '\\' | ';' | '&' | '|' | '`' | '$' | '<' | '>' | '\n' | '\r'
        )
    }) {
        return Err(DockerApiError::BadRequest(
            "network name contains invalid characters".into(),
        ));
    }
    Ok(())
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default()
}
