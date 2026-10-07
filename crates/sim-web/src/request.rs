//! Requests to the results API, from the page it served, with the browser's `fetch`.

use std::fmt;

use leptos::prelude::*;
use serde::de::DeserializeOwned;
use tempest_sim_api::ApiError;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

/// Why a request failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestError {
    /// The server didn't answer.
    Unreachable,
    /// The server refused the request, or couldn't answer it.
    Refused(ApiError),
    /// The answer wasn't what the UI expected.
    Unexpected(String),
}

impl fmt::Display for RequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreachable => write!(
                f,
                "Couldn't reach the results server. Is tempest sim web still running?"
            ),
            Self::Refused(error) => write!(f, "{}", sentence(&error.message)),
            Self::Unexpected(why) => write!(f, "The server answered oddly: {why}"),
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
        return serde_json::from_str(body).map_err(|e| RequestError::Unexpected(e.to_string()));
    }
    match serde_json::from_str::<ApiError>(body) {
        Ok(error) => Err(RequestError::Refused(error)),
        Err(_) => Err(RequestError::Unexpected(format!("status {status}"))),
    }
}

/// `GET path`, read as `T`.
pub async fn get<T: DeserializeOwned>(path: &str) -> Result<T, RequestError> {
    let (status, text) = fetch(path).await.map_err(|_| RequestError::Unreachable)?;
    decode(status, &text)
}

async fn fetch(path: &str) -> Result<(u16, String), JsValue> {
    let window = web_sys::window().ok_or("no window")?;
    let response: web_sys::Response = JsFuture::from(window.fetch_with_str(path))
        .await?
        .dyn_into()?;
    let text = JsFuture::from(response.text()?).await?;
    Ok((response.status(), text.as_string().unwrap_or_default()))
}

/// What a page loads: `None` while the first answer is on its way.
pub type Loaded<T> = LocalResource<Result<T, RequestError>>;

/// Loads `path()` from the API, and again whenever the signals it reads change. While a new
/// answer is on its way, the last one stays.
pub fn load<T>(path: impl Fn() -> String + 'static) -> Loaded<T>
where
    T: DeserializeOwned + 'static,
{
    LocalResource::new(move || {
        let path = path();
        async move { get::<T>(&path).await }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_decode_into_values_or_the_apis_errors() {
        assert_eq!(decode::<Vec<u8>>(200, "[1,2]"), Ok(vec![1, 2]));
        let refused = decode::<()>(503, r#"{"error":"busy","message":"the database is busy"}"#);
        assert_eq!(refused.unwrap_err().to_string(), "The database is busy.");
        let missing = decode::<()>(404, r#"{"error":"not_found","message":"no run"}"#);
        assert!(matches!(missing, Err(RequestError::Refused(e)) if e.error == "not_found"));
        assert!(matches!(
            decode::<()>(502, "<html>"),
            Err(RequestError::Unexpected(_))
        ));
    }
}
