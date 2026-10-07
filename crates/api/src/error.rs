//! The API's errors, all in one JSON shape.

use serde::{Deserialize, Serialize};

/// Why a request failed: the body of every response with an error status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{message}")]
pub struct ApiError {
    /// A short, stable code, such as `not_found`.
    pub error: String,
    /// What went wrong, in a sentence.
    pub message: String,
    /// The HTTP status.
    #[serde(skip)]
    pub status: u16,
}

impl ApiError {
    /// An error with HTTP `status`, `code` and `message`.
    pub fn new(status: u16, code: &str, message: impl Into<String>) -> Self {
        Self {
            error: code.to_owned(),
            message: message.into(),
            status,
        }
    }

    /// A request the API can't make sense of.
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(400, "invalid_request", message)
    }
}

#[cfg(feature = "axum")]
impl axum::response::IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        use axum::http::{StatusCode, header};

        let status = StatusCode::from_u16(self.status).unwrap_or(StatusCode::BAD_REQUEST);
        let body = serde_json::to_vec(&self).expect("errors serialize");
        (status, [(header::CONTENT_TYPE, "application/json")], body).into_response()
    }
}
