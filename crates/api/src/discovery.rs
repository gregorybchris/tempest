//! The files in an engine's data directory that tell local clients where its API is.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The file in an engine's data directory that says where its API is.
pub const DISCOVERY_FILE: &str = "api.json";
/// The file in an engine's data directory that holds the access token for remote access.
pub const TOKEN_FILE: &str = "api-token";

/// Where a running engine's API is, as its discovery file records it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Discovery {
    /// The API's base URL, such as `http://127.0.0.1:4776`.
    pub url: String,
    /// The certificate the API presents over HTTPS, which local clients trust exactly.
    pub cert: Option<PathBuf>,
    /// The engine's process ID.
    pub pid: u32,
}

impl Discovery {
    /// Reads the discovery file in `data_dir`, if there is one.
    pub fn read(data_dir: &Path) -> io::Result<Option<Self>> {
        match std::fs::read(data_dir.join(DISCOVERY_FILE)) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(io::Error::other),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }
}
