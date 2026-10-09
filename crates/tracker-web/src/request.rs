//! Requests to the tracker's API, from the page the tracker served (see
//! [`tempest_ui_kit::request`]). The session travels in the tracker's `HttpOnly` cookie, which
//! `fetch` sends on its own.

use serde::Serialize;
use serde::de::DeserializeOwned;
use tempest_protocol::tracker::{API_PREFIX, ErrorResponse};
use tempest_tracker_api::codes;
use tempest_ui_kit::request::{self, Server};

pub use tempest_ui_kit::request::{method, sentence};

/// The tracker that served the page.
pub struct Tracker;

impl Server for Tracker {
    type Error = ErrorResponse;
    const PREFIX: &'static str = API_PREFIX;
    const NAME: &'static str = "The tracker";
    const UNREACHABLE: &'static str = "Couldn't reach the tracker. Is it still running?";

    fn code(error: &ErrorResponse) -> &str {
        error.error.as_str()
    }

    fn message(error: &ErrorResponse) -> &str {
        &error.message
    }
}

/// Why a request to the tracker failed.
pub type RequestError = request::RequestError<Tracker>;

/// Whether the tracker refused because there is no valid session.
pub fn needs_sign_in(error: &RequestError) -> bool {
    error.code() == Some(codes::UNAUTHORIZED)
}

/// Whether the tracker has no such thing, or won't show it.
pub fn not_found(error: &RequestError) -> bool {
    error.status() == Some(404)
}

/// Sends `method` to the tracker endpoint at `path` (relative to [`API_PREFIX`]), with `body` as
/// JSON, and reads the answer.
pub async fn send<T: DeserializeOwned>(
    method: &str,
    path: &str,
    body: Option<&impl Serialize>,
) -> Result<T, RequestError> {
    request::send(method, path, body).await
}

/// `GET` of the tracker endpoint at `path`.
pub async fn get<T: DeserializeOwned>(path: &str) -> Result<T, RequestError> {
    request::get(path).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusals_say_whether_to_sign_in_or_that_nothing_is_there() {
        let unauthorized = r#"{"error":"unauthorized","message":"sign in first"}"#;
        let refused = request::decode::<Tracker, ()>(401, unauthorized).unwrap_err();
        assert_eq!(refused.to_string(), "Sign in first.");
        assert!(needs_sign_in(&refused) && !not_found(&refused));
        let missing = r#"{"error":"not_found","message":"no such listing"}"#;
        assert!(not_found(
            &request::decode::<Tracker, ()>(404, missing).unwrap_err()
        ));
    }
}
