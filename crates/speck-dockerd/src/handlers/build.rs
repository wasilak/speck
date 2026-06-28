use std::collections::HashMap;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use tracing::Instrument;

use crate::state::AppState;

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub struct BuildQuery {
    dockerfile: Option<String>,
    t: Option<String>,
    remote: Option<String>,
    q: Option<bool>,
    nocache: Option<bool>,
    cachefrom: Option<String>,
    pull: Option<bool>,
    rm: Option<bool>,
    forcerm: Option<bool>,
    labels: Option<String>,
    buildargs: Option<String>,
    shmsize: Option<i64>,
    ulimits: Option<String>,
    networkmode: Option<String>,
    platform: Option<String>,
    target: Option<String>,
    outputs: Option<String>,
}

pub async fn build(
    State(state): State<AppState>,
    Query(query): Query<BuildQuery>,
) -> impl IntoResponse {
    let buildkit = match state.buildkit_client().await {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"message": format!("BuildKit unavailable: {e}")})),
            );
        }
    };

    let tag = query.t.clone().unwrap_or_else(|| "latest".into());
    let frontend = if query.remote.is_some() {
        "gateway.v0"
    } else {
        "dockerfile.v0"
    };

    let mut frontend_attrs: HashMap<String, String> = HashMap::new();
    if let Some(df) = &query.dockerfile {
        frontend_attrs.insert("filename".into(), df.clone());
    }
    if let Some(target) = &query.target {
        frontend_attrs.insert("target".into(), target.clone());
    }
    if let Some(buildargs) = &query.buildargs {
        if let Ok(args) = serde_json::from_str::<HashMap<String, String>>(buildargs) {
            for (k, v) in args {
                frontend_attrs.insert(format!("build-arg:{k}"), v);
            }
        }
    }
    if let Some(labels) = &query.labels {
        if let Ok(labs) = serde_json::from_str::<HashMap<String, String>>(labels) {
            for (k, v) in labs {
                frontend_attrs.insert(format!("label:{k}"), v);
            }
        }
    }
    if let Some(platform) = &query.platform {
        frontend_attrs.insert("platform".into(), platform.clone());
    }
    if let Some(nocache) = query.nocache {
        if nocache {
            frontend_attrs.insert("no-cache".into(), "".into());
        }
    }
    if let Some(cachefrom) = &query.cachefrom {
        frontend_attrs.insert("cache-from".into(), cachefrom.clone());
    }

    let req = crate::buildkit::proto::SolveRequest {
        r#ref: tag.clone(),
        frontend: frontend.into(),
        frontend_attrs,
        cache: None,
        exports: Vec::new(),
    };

    let span = tracing::info_span!("buildkit_solve", ref_ = %tag);
    match buildkit.solve(req).instrument(span).await {
        Ok(_) => {
            let body = serde_json::json!({"stream": format!("Build complete for {tag}\n")});
            (StatusCode::OK, Json(body))
        }
        Err(e) => {
            let body = serde_json::json!({"message": format!("Build failed: {e}")});
            (StatusCode::INTERNAL_SERVER_ERROR, Json(body))
        }
    }
}
