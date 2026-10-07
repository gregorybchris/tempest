//! Named profiles: complete sets of settings to start from (D-005).

use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{ChunkSelection, Config, Endgame, Upload, WhenComplete};

/// A named set of settings. Overrides then change single settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Profile {
    /// The defaults: BitTorrent-style reciprocity, rarest-first, adaptive pipelining, limited
    /// endgame, round-robin once complete.
    #[default]
    Balanced,
    /// The classic BitTorrent client, as a baseline: one-chunk requests with a fixed pipeline,
    /// classic endgame, and the same upload strategy once complete.
    Classic,
    /// For trusted swarms: serve everyone and keep sharing.
    Cooperative,
    /// Polite background sharing: fewer connections, fewer uploads at once.
    Background,
    /// For playing media while downloading: chunks near the playback position first.
    Streaming,
}

impl Profile {
    /// Every profile.
    pub const ALL: [Self; 5] = [
        Self::Balanced,
        Self::Classic,
        Self::Cooperative,
        Self::Background,
        Self::Streaming,
    ];

    /// The profile's name, as written in settings.
    pub fn name(self) -> &'static str {
        match self {
            Self::Balanced => "balanced",
            Self::Classic => "classic",
            Self::Cooperative => "cooperative",
            Self::Background => "background",
            Self::Streaming => "streaming",
        }
    }

    /// The profile's settings.
    pub fn config(self) -> Config {
        let mut config = Config::default();
        match self {
            Self::Balanced => {}
            Self::Classic => {
                config.strategies.when_complete = WhenComplete::Unchanged;
                config.strategies.endgame = Endgame::Classic;
                config.requests.chunks = 1;
                config.requests.adaptive = false;
                config.requests.initial_in_flight = 5;
            }
            Self::Cooperative => {
                config.strategies.upload = Upload::Cooperative;
                config.lingering.keep_sharing = true;
            }
            Self::Background => {
                config.connections.max = 20;
                config.upload.parallel_responses = 2;
                config.upload.reciprocal_slots = 2;
                config.lingering.idle_timeout = Duration::from_secs(120);
            }
            Self::Streaming => {
                config.strategies.chunk_selection = ChunkSelection::Streaming;
            }
        }
        config
    }
}

impl fmt::Display for Profile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The name matched no profile.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("there is no profile {0:?}")]
pub struct UnknownProfile(pub String);

impl FromStr for Profile {
    type Err = UnknownProfile;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|p| p.name() == name)
            .ok_or_else(|| UnknownProfile(name.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_profile_is_valid_and_named_consistently() {
        for profile in Profile::ALL {
            assert_eq!(profile.config().validate(), Ok(()), "{profile}");
            assert_eq!(profile.name().parse(), Ok(profile));
            let json = serde_json::to_value(profile).unwrap();
            assert_eq!(json, profile.name());
        }
        assert_eq!(Profile::Balanced.config(), Config::default());
        assert!("fast".parse::<Profile>().is_err());
    }
}
