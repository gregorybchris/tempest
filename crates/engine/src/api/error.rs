//! The API's errors, all in one JSON shape, and the JSON body extractor that produces them.

use axum::body::Bytes;
use axum::extract::{FromRequest, Request};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde::de::DeserializeOwned;
pub use tempest_api::ApiError;

use crate::EngineError;

impl From<EngineError> for ApiError {
    fn from(error: EngineError) -> Self {
        use EngineError as E;
        let (status, code) = match &error {
            E::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
            E::AlreadyAdded(_) => (StatusCode::CONFLICT, "already_added"),
            E::Removing(_) => (StatusCode::CONFLICT, "removing"),
            E::Moving(_) => (StatusCode::CONFLICT, "moving"),
            E::NoData(_) => (StatusCode::CONFLICT, "no_data"),
            E::UnknownCategory(_) => (StatusCode::BAD_REQUEST, "unknown_category"),
            E::Config(_)
            | E::InvalidLink(_)
            | E::Build(_)
            | E::Empty(_)
            | E::InvalidPath(_)
            | E::Fixed(_) => (StatusCode::BAD_REQUEST, "invalid_request"),
            E::Stopped | E::Busy(_) => (StatusCode::SERVICE_UNAVAILABLE, "unavailable"),
            E::Store(_) | E::Identity(_) | E::Io { .. } | E::Bind { .. } | E::Storage(_) => {
                (StatusCode::INTERNAL_SERVER_ERROR, "internal")
            }
        };
        Self::new(status.as_u16(), code, error.to_string())
    }
}

/// A JSON response.
pub(super) fn json(status: StatusCode, body: &impl Serialize) -> Response {
    let body = serde_json::to_vec(body).expect("the API's types serialize");
    let content_type = HeaderValue::from_static("application/json");
    (status, [(header::CONTENT_TYPE, content_type)], body).into_response()
}

/// A JSON request body. Unlike axum's, its rejections are [`ApiError`]s. Bodies must be sent
/// as `application/json`, which browsers can't do across origins without asking first.
pub(super) struct Json<T>(pub T);

impl<T: DeserializeOwned, S: Send + Sync> FromRequest<S> for Json<T> {
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        if !is_json(request.headers()) {
            return Err(ApiError::new(
                StatusCode::UNSUPPORTED_MEDIA_TYPE.as_u16(),
                "unsupported_media_type",
                "the body must be JSON, sent as application/json",
            ));
        }
        let body = Bytes::from_request(request, state)
            .await
            .map_err(|rejection| ApiError {
                status: rejection.status().as_u16(),
                ..ApiError::invalid(rejection.body_text())
            })?;
        serde_json::from_slice(&body)
            .map(Json)
            .map_err(|error| ApiError::invalid(format!("invalid JSON body: {error}")))
    }
}

fn is_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
}
