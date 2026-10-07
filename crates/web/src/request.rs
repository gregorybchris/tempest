//! Requests to the engine's API, from the page it served, with the browser's `fetch`.

use std::fmt;

use serde::Serialize;
use serde::de::DeserializeOwned;
use tempest_api::ApiError;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

/// Why a request failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestError {
    /// The engine didn't answer.
    Unreachable,
    /// The engine refused the request.
    Refused(ApiError),
    /// The answer wasn't what the UI expected.
    Unexpected(String),
}

impl fmt::Display for RequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreachable => write!(f, "Couldn't reach the engine. Is it still running?"),
            Self::Refused(error) => write!(f, "{}", sentence(&error.message)),
            Self::Unexpected(why) => write!(f, "The engine answered oddly: {why}"),
        }
    }
}

/// `text` with a capital and a full stop, as the API's messages are clauses.
fn sentence(text: &str) -> String {
    let mut chars = text.trim().chars();
    let mut out: String = match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => return String::new(),
    };
    if !out.ends_with(['.', '!', '?']) {
        out.push('.');
    }
    out
}

/// The answer to a request, from its HTTP status and body: the value, or the API's error.
pub fn decode<T: DeserializeOwned>(status: u16, body: &str) -> Result<T, RequestError> {
    if (200..300).contains(&status) {
        // An empty body (204) reads as `null`, which `()` and `Option` accept.
        let body = if body.trim().is_empty() { "null" } else { body };
        return serde_json::from_str(body).map_err(|e| RequestError::Unexpected(e.to_string()));
    }
    match serde_json::from_str::<ApiError>(body) {
        Ok(error) => Err(RequestError::Refused(ApiError { status, ..error })),
        Err(_) => Err(RequestError::Unexpected(format!("status {status}"))),
    }
}

/// Sends `method` to `path`, with `body` as JSON, and reads the answer.
pub async fn send<T: DeserializeOwned>(
    method: &str,
    path: &str,
    body: Option<&impl Serialize>,
) -> Result<T, RequestError> {
    let body = body
        .map(serde_json::to_string)
        .transpose()
        .map_err(|e| RequestError::Unexpected(e.to_string()))?;
    let (status, text) = fetch(method, path, body)
        .await
        .map_err(|_| RequestError::Unreachable)?;
    decode(status, &text)
}

/// `GET path`.
pub async fn get<T: DeserializeOwned>(path: &str) -> Result<T, RequestError> {
    send(method::GET, path, None::<&()>).await
}

/// HTTP methods, by name.
pub mod method {
    /// Reads.
    pub const GET: &str = "GET";
    /// Acts.
    pub const POST: &str = "POST";
    /// Replaces.
    pub const PUT: &str = "PUT";
    /// Changes some fields.
    pub const PATCH: &str = "PATCH";
    /// Removes.
    pub const DELETE: &str = "DELETE";
}

async fn fetch(method: &str, path: &str, body: Option<String>) -> Result<(u16, String), JsValue> {
    let init = web_sys::RequestInit::new();
    init.set_method(method);
    if let Some(body) = body {
        let headers = web_sys::Headers::new()?;
        headers.set("content-type", "application/json")?;
        init.set_headers(&headers);
        init.set_body(&JsValue::from_str(&body));
    }
    let window = web_sys::window().ok_or("no window")?;
    let response: web_sys::Response = JsFuture::from(window.fetch_with_str_and_init(path, &init))
        .await?
        .dyn_into()?;
    let text = JsFuture::from(response.text()?).await?;
    Ok((response.status(), text.as_string().unwrap_or_default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_decode_into_values_or_the_apis_errors() {
        assert_eq!(decode::<Vec<u8>>(200, "[1,2]"), Ok(vec![1, 2]));
        assert_eq!(decode::<()>(204, ""), Ok(()));
        let refused = decode::<()>(
            409,
            r#"{"error":"already_added","message":"the bundle is already added"}"#,
        );
        let Err(RequestError::Refused(error)) = &refused else {
            panic!("{refused:?}");
        };
        assert_eq!((error.status, error.error.as_str()), (409, "already_added"));
        assert_eq!(
            refused.unwrap_err().to_string(),
            "The bundle is already added."
        );
        assert!(matches!(
            decode::<()>(502, "<html>"),
            Err(RequestError::Unexpected(_))
        ));
        assert!(matches!(
            decode::<u8>(200, "\"x\""),
            Err(RequestError::Unexpected(_))
        ));
    }
}
