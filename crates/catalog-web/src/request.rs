//! Requests to the catalog's API, from the page the tracker served, with the browser's `fetch`.
//! The session travels in the tracker's `HttpOnly` cookie, which `fetch` sends on its own.

use std::fmt;

use serde::Serialize;
use serde::de::DeserializeOwned;
use tempest_catalog_api::{PREFIX, codes};
use tempest_protocol::tracker::ErrorResponse;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

/// Why a request failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestError {
    /// The tracker didn't answer.
    Unreachable,
    /// The tracker refused the request, with its HTTP status.
    Refused(u16, ErrorResponse),
    /// The answer wasn't what the UI expected.
    Unexpected(String),
}

impl RequestError {
    /// Whether the tracker refused because there is no valid session.
    pub fn needs_sign_in(&self) -> bool {
        matches!(self, Self::Refused(_, e) if e.error.as_str() == codes::UNAUTHORIZED)
    }

    /// Whether the tracker has no such thing, or won't show it.
    pub fn not_found(&self) -> bool {
        matches!(self, Self::Refused(404, _))
    }
}

impl fmt::Display for RequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreachable => write!(f, "Couldn't reach the tracker. Is it still running?"),
            Self::Refused(_, error) => write!(f, "{}", sentence(&error.message)),
            Self::Unexpected(why) => write!(f, "The tracker answered oddly: {why}"),
        }
    }
}

/// `text` with a capital and a full stop, as the API's messages are clauses.
pub fn sentence(text: &str) -> String {
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

/// The answer to a request, from its HTTP status and body: the value, or the tracker's error.
pub fn decode<T: DeserializeOwned>(status: u16, body: &str) -> Result<T, RequestError> {
    if (200..300).contains(&status) {
        return serde_json::from_str(body).map_err(|e| RequestError::Unexpected(e.to_string()));
    }
    match serde_json::from_str::<ErrorResponse>(body) {
        Ok(error) => Err(RequestError::Refused(status, error)),
        Err(_) => Err(RequestError::Unexpected(format!("status {status}"))),
    }
}

/// HTTP methods, by name.
pub mod method {
    /// Reads.
    pub const GET: &str = "GET";
    /// Acts.
    pub const POST: &str = "POST";
    /// Replaces.
    pub const PUT: &str = "PUT";
    /// Removes.
    pub const DELETE: &str = "DELETE";
}

/// Sends `method` to the catalog endpoint at `path` (relative to [`PREFIX`]), with `body` as
/// JSON, and reads the answer.
pub async fn send<T: DeserializeOwned>(
    method: &str,
    path: &str,
    body: Option<&impl Serialize>,
) -> Result<T, RequestError> {
    let body = body
        .map(serde_json::to_string)
        .transpose()
        .map_err(|e| RequestError::Unexpected(e.to_string()))?;
    let url = format!("{PREFIX}{path}");
    let (status, text) = fetch(method, &url, body)
        .await
        .map_err(|_| RequestError::Unreachable)?;
    decode(status, &text)
}

/// `GET` of the catalog endpoint at `path`.
pub async fn get<T: DeserializeOwned>(path: &str) -> Result<T, RequestError> {
    send(method::GET, path, None::<&()>).await
}

async fn fetch(method: &str, url: &str, body: Option<String>) -> Result<(u16, String), JsValue> {
    let init = web_sys::RequestInit::new();
    init.set_method(method);
    // The catalog refuses bodies that don't say JSON, so other sites' forms can't post here;
    // every write sends one, even an empty object.
    if method != method::GET {
        let headers = web_sys::Headers::new()?;
        headers.set("content-type", "application/json")?;
        init.set_headers(&headers);
        init.set_body(&JsValue::from_str(body.as_deref().unwrap_or("{}")));
    }
    let window = web_sys::window().ok_or("no window")?;
    let response: web_sys::Response = JsFuture::from(window.fetch_with_str_and_init(url, &init))
        .await?
        .dyn_into()?;
    let text = JsFuture::from(response.text()?).await?;
    Ok((response.status(), text.as_string().unwrap_or_default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_decode_into_values_or_the_trackers_errors() {
        assert_eq!(decode::<Vec<u8>>(200, "[1,2]"), Ok(vec![1, 2]));
        let refused = decode::<()>(401, r#"{"error":"unauthorized","message":"sign in first"}"#);
        let refused = refused.unwrap_err();
        assert_eq!(refused.to_string(), "Sign in first.");
        assert!(refused.needs_sign_in());
        let missing = decode::<()>(404, r#"{"error":"not_found","message":"no such listing"}"#);
        assert!(missing.unwrap_err().not_found());
        assert!(matches!(
            decode::<()>(502, "<html>"),
            Err(RequestError::Unexpected(_))
        ));
    }
}
