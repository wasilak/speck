use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DockerApiError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("internal error: {0}")]
    Internal(String),
}

pub type Result<T> = std::result::Result<T, DockerApiError>;

#[derive(Serialize)]
struct ErrorBody {
    message: String,
}

impl IntoResponse for DockerApiError {
    fn into_response(self) -> Response {
        let status = match self {
            DockerApiError::NotFound(_) => StatusCode::NOT_FOUND,
            DockerApiError::Conflict(_) => StatusCode::CONFLICT,
            DockerApiError::BadRequest(_) => StatusCode::BAD_REQUEST,
            DockerApiError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let body = Json(ErrorBody {
            message: self.to_string(),
        });

        (status, body).into_response()
    }
}
