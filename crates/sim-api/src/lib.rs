//! The simulation results API (`specs/simulation.md`): what `tempest-sim web` serves from the
//! results database, and the queries that select it.
//!
//! The server and the results UI both use these types, so they build for WebAssembly as well
//! as natively: they depend on serde only.
//!
//! - [`data`]: the API's answers: experiments, runs, their time series, comparisons of
//!   variants and checks against golden baselines.
//! - [`query`]: the filters, sorts and selections in the API's query strings, which the UI
//!   writes and the server reads.

pub mod data;
pub mod query;

use serde::{Deserialize, Serialize};

/// The prefix of every API route.
pub const PREFIX: &str = "/v1";

/// An error the API answers with, in the body of a 4xx or 5xx response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    /// A short, stable name for the kind of error, such as `not_found`.
    pub error: String,
    /// What went wrong, as a clause in plain language.
    pub message: String,
}

impl ApiError {
    /// An error of kind `error`.
    pub fn new(error: &str, message: impl Into<String>) -> Self {
        Self {
            error: error.to_owned(),
            message: message.into(),
        }
    }
}
