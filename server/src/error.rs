//! One error type for all API handlers, rendered as `{"error": "..."}`.

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;

#[derive(Debug)]
pub enum ApiError {
    BadRequest(&'static str),
    Unauthorized,
    Forbidden,
    NotFound,
    Conflict(&'static str),
    TooManyRequests,
    Unavailable(&'static str),
    TooLarge(&'static str),
    StorageFull(&'static str),
    Internal(anyhow::Error),
}

pub type ApiResult<T> = Result<T, ApiError>;

impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(err: E) -> Self {
        ApiError::Internal(err.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            ApiError::Unauthorized => (StatusCode::UNAUTHORIZED, "not signed in"),
            ApiError::Forbidden => (StatusCode::FORBIDDEN, "not allowed"),
            ApiError::NotFound => (StatusCode::NOT_FOUND, "not found"),
            ApiError::Conflict(m) => (StatusCode::CONFLICT, m),
            ApiError::TooManyRequests => (
                StatusCode::TOO_MANY_REQUESTS,
                "too many attempts, try again later",
            ),
            ApiError::Unavailable(m) => (StatusCode::SERVICE_UNAVAILABLE, m),
            ApiError::TooLarge(m) => (StatusCode::PAYLOAD_TOO_LARGE, m),
            ApiError::StorageFull(m) => (StatusCode::INSUFFICIENT_STORAGE, m),
            ApiError::Internal(err) => {
                // Log the detail, never send it to the client.
                tracing::error!(error = ?err, "internal error");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal error")
            }
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}
