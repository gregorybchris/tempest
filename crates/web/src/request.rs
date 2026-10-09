//! Requests to the engine's API, from the page it served (see [`tempest_ui_kit::request`]).

use serde::Serialize;
use serde::de::DeserializeOwned;
use tempest_api::ApiError;
use tempest_ui_kit::request::{self, Server};

pub use tempest_ui_kit::request::method;

/// The engine that served the page.
pub struct Engine;

impl Server for Engine {
    type Error = ApiError;
    // Paths name the API's version themselves.
    const PREFIX: &'static str = "";
    const NAME: &'static str = "The engine";
    const UNREACHABLE: &'static str = "Couldn't reach the engine. Is it still running?";

    fn code(error: &ApiError) -> &str {
        &error.error
    }

    fn message(error: &ApiError) -> &str {
        &error.message
    }
}

/// Why a request to the engine failed.
pub type RequestError = request::RequestError<Engine>;

/// Sends `method` to `path`, with `body` as JSON, and reads the answer.
pub async fn send<T: DeserializeOwned>(
    method: &str,
    path: &str,
    body: Option<&impl Serialize>,
) -> Result<T, RequestError> {
    request::send(method, path, body).await
}

/// `GET path`.
pub async fn get<T: DeserializeOwned>(path: &str) -> Result<T, RequestError> {
    request::get(path).await
}
