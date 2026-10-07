//! Session settings and strategy choices.

use std::time::Duration;

use tempest_protocol::message::MAX_REQUEST_CHUNKS;

use crate::select::{self, ChunkSelector};
use crate::upload::{self, UploadStrategy};

/// Everything a [`Session`](crate::Session) can be configured with.
///
/// The defaults are starting points, to be tuned with simulation (`specs/strategies.md`).
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// The client name and version sent in Hello.
    pub client: String,
    /// The largest Request `count` this peer accepts, sent in Hello: a power of two from 1 to
    /// 1,024.
    pub max_request_chunks: u32,
    /// The most Requests this peer accepts open at once from one requester, sent in Hello.
    pub max_open_requests: u32,
    /// The number of chunks this peer asks for in one Request, when it can: a power of two from 1
    /// to 1,024. Requests are smaller at file ends and when the provider's limit is lower.
    pub request_chunks: u32,
    /// The most requests this peer keeps open with one provider.
    pub requests_in_flight: u32,
    /// How far below a provider's `max_open_requests` this peer stays, since a provider's count
    /// can lag behind (`protocol/messages.md`, Request).
    pub request_headroom: u32,
    /// The most Data responses this peer sends at once, across all connections. Further accepted
    /// requests wait in a queue.
    pub upload_slots: u32,
    /// The most connections for the bundle (D-064).
    pub max_connections: u32,
    /// How long to wait before asking a provider again after it rejected a request.
    pub retry_delay: Duration,
    /// How long to wait before dialing a peer again after a connection to it failed or ended.
    pub redial_delay: Duration,
    /// The window over which transfer rates are measured.
    pub rate_window: Duration,
    /// The most chunk ranges this peer keeps from a peer's Have and HaveUpdates while it doesn't
    /// have the manifest yet. A peer that sends more is disconnected.
    pub max_pending_ranges: usize,
    /// How to choose which chunks to request.
    pub chunk_selection: ChunkSelection,
    /// How to choose whom to serve.
    pub upload: Upload,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            client: concat!("tempest/", env!("CARGO_PKG_VERSION")).to_owned(),
            max_request_chunks: 64,
            max_open_requests: 32,
            request_chunks: 16,
            requests_in_flight: 8,
            request_headroom: 2,
            upload_slots: 8,
            max_connections: 50,
            retry_delay: Duration::from_secs(1),
            redial_delay: Duration::from_secs(60),
            rate_window: Duration::from_secs(20),
            max_pending_ranges: 1 << 20,
            chunk_selection: ChunkSelection::default(),
            upload: Upload::default(),
        }
    }
}

/// A chunk selection strategy, with its parameters (D-059).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkSelection {
    /// Request the chunks the fewest connected peers have, after picking the first few at random.
    RarestFirst {
        /// How many chunks to pick at random before switching to rarest-first.
        random_first: u32,
    },
    /// Request any needed chunk at random.
    Random,
    /// Request chunks in order.
    Sequential,
}

impl Default for ChunkSelection {
    fn default() -> Self {
        Self::RarestFirst { random_first: 4 }
    }
}

impl ChunkSelection {
    /// Builds the strategy.
    pub fn build(self) -> Box<dyn ChunkSelector> {
        match self {
            Self::RarestFirst { random_first } => Box::new(select::RarestFirst { random_first }),
            Self::Random => Box::new(select::Random),
            Self::Sequential => Box::new(select::Sequential),
        }
    }
}

/// An upload strategy, with its parameters (D-062).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Upload {
    /// Serve every peer that wants data.
    #[default]
    Cooperative,
}

impl Upload {
    /// Builds the strategy.
    pub fn build(self) -> Box<dyn UploadStrategy> {
        match self {
            Self::Cooperative => Box::new(upload::Cooperative),
        }
    }
}

/// A setting that is out of range.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    /// A chunk count was not a power of two from 1 to 1,024.
    #[error("{field} is {value}, not a power of two from 1 to {MAX_REQUEST_CHUNKS}")]
    NotRequestSize {
        /// The setting.
        field: &'static str,
        /// Its value.
        value: u32,
    },
    /// A count that must be at least 1 was 0.
    #[error("{0} must be at least 1")]
    Zero(&'static str),
}

impl Config {
    /// Checks that every setting is in range.
    pub fn validate(&self) -> Result<(), ConfigError> {
        for (field, value) in [
            ("max_request_chunks", self.max_request_chunks),
            ("request_chunks", self.request_chunks),
        ] {
            if !value.is_power_of_two() || value > MAX_REQUEST_CHUNKS {
                return Err(ConfigError::NotRequestSize { field, value });
            }
        }
        for (field, value) in [
            ("max_open_requests", self.max_open_requests),
            ("requests_in_flight", self.requests_in_flight),
            ("upload_slots", self.upload_slots),
        ] {
            if value == 0 {
                return Err(ConfigError::Zero(field));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_valid() {
        assert_eq!(Config::default().validate(), Ok(()));
    }

    #[test]
    fn rejects_out_of_range_settings() {
        let config = Config {
            request_chunks: 12,
            ..Config::default()
        };
        assert!(matches!(
            config.validate(),
            Err(ConfigError::NotRequestSize { value: 12, .. })
        ));
        let config = Config {
            upload_slots: 0,
            ..Config::default()
        };
        assert_eq!(config.validate(), Err(ConfigError::Zero("upload_slots")));
    }
}
