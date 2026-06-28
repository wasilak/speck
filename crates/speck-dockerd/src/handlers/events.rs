use std::convert::Infallible;

use axum::body::{Body, Bytes};
use axum::extract::{Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::Value;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

use crate::state::AppState;

#[derive(Debug, Deserialize, Default)]
pub struct EventsQuery {
    #[serde(default)]
    filters: Option<String>,
}

pub async fn events_stream(
    State(state): State<AppState>,
    Query(_query): Query<EventsQuery>,
) -> Response {
    tracing::debug!("opening Docker events broadcast stream");
    let rx = state.event_tx.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|event| match event {
        Ok(value) => {
            let mut line = serde_json::to_vec(&value).expect("event JSON serializes");
            line.push(b'\n');
            Some(Ok::<Bytes, Infallible>(Bytes::from(line)))
        }
        Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(skipped)) => {
            tracing::warn!(skipped, "Docker event subscriber lagged; dropping events");
            None
        }
    });

    (
        [(header::CONTENT_TYPE, "application/json")],
        Body::from_stream(stream),
    )
        .into_response()
}

pub fn emit_event(state: &AppState, event: Value) {
    let _ = state.event_tx.send(event);
}
