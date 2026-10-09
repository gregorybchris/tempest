//! Requests to the results API, from the page it served (see [`tempest_ui_kit::request`]).

use leptos::prelude::*;
use serde::de::DeserializeOwned;
use tempest_sim_api::ApiError;
use tempest_ui_kit::request::{self, Server};

/// The results server that served the page.
pub struct Results;

impl Server for Results {
    type Error = ApiError;
    // Paths name the API's version themselves.
    const PREFIX: &'static str = "";
    const NAME: &'static str = "The server";
    const UNREACHABLE: &'static str =
        "Couldn't reach the results server. Is tempest-sim web still running?";

    fn code(error: &ApiError) -> &str {
        &error.error
    }

    fn message(error: &ApiError) -> &str {
        &error.message
    }
}

/// Why a request to the results server failed.
pub type RequestError = request::RequestError<Results>;

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
        async move { request::get::<Results, T>(&path).await }
    })
}
