//! What the app asks the runtime to do, and how it reports the outcome.

use std::collections::BTreeSet;
use std::path::PathBuf;

use tempest_engine::api::{AddRequest, QueueStep};
use tempest_protocol::BundleId;

/// The transfer a request is about, named as it was when the request was made, since it may
/// be gone (or renamed) by the time the answer comes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// Its bundle ID.
    pub id: BundleId,
    /// Its name.
    pub name: String,
}

/// Something done to one transfer through the engine's API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Start it through the queue.
    Start,
    /// Start it regardless of the queue.
    ForceStart,
    /// Stop it.
    Stop,
    /// Check its data on disk again.
    Recheck,
    /// Remove it, deleting its files too if set.
    Remove {
        /// Whether its files go too.
        delete_files: bool,
    },
    /// Show it under this name, or its bundle's with `None`.
    Rename(Option<String>),
    /// Replace its tags.
    Tags(BTreeSet<String>),
    /// Put it in this category, or none.
    Category(Option<String>),
    /// Move its files into this folder.
    Move(PathBuf),
    /// Move it in the queue.
    Queue(QueueStep),
}

/// Something the app can't do itself: a request to the engine, or opening a folder.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    /// Act on a transfer.
    Transfer(Target, Action),
    /// Add a download from a link.
    Add(Box<AddRequest>),
    /// Fetch the categories.
    Categories,
    /// Open a folder on this machine.
    Open(PathBuf),
}

/// What a request that succeeded brought back, where the app needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// Nothing beyond success: the change arrives on the event stream.
    Done,
    /// The name of the transfer just added.
    Added(String),
    /// Every category's name.
    Categories(Vec<String>),
}

impl Request {
    /// The notice for its success, if it deserves one.
    pub(super) fn success(&self, reply: &Reply) -> Option<String> {
        Some(match (self, reply) {
            (Self::Transfer(target, action), _) => action.success(&target.name),
            (Self::Add(_), Reply::Added(name)) => format!("Added {name}."),
            (Self::Add(_), _) => "Added the link.".to_owned(),
            (Self::Categories, _) => return None,
            (Self::Open(path), _) => format!("Opened {}.", path.display()),
        })
    }

    /// The notice for its failure, given the engine's explanation.
    pub(super) fn failure(&self, error: &str) -> String {
        match self {
            Self::Transfer(target, action) => {
                format!("Couldn't {} {}: {error}", action.verb(), target.name)
            }
            Self::Add(_) => format!("Couldn't add the link: {error}"),
            Self::Categories => format!("Couldn't load the categories: {error}"),
            Self::Open(path) => format!("Couldn't open {}: {error}", path.display()),
        }
    }
}

impl Action {
    fn verb(&self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::ForceStart => "force-start",
            Self::Stop => "stop",
            Self::Recheck => "recheck",
            Self::Remove { .. } => "remove",
            Self::Rename(_) => "rename",
            Self::Tags(_) => "tag",
            Self::Category(_) => "set the category of",
            Self::Move(_) => "move",
            Self::Queue(_) => "reorder",
        }
    }

    fn success(&self, name: &str) -> String {
        match self {
            Self::Start => format!("Started {name}."),
            Self::ForceStart => format!("Force-started {name}."),
            Self::Stop => format!("Stopped {name}."),
            Self::Recheck => format!("Checking {name}'s data again."),
            Self::Remove {
                delete_files: false,
            } => format!("Removed {name}. Its files stay."),
            Self::Remove { delete_files: true } => {
                format!("Removed {name} and deleted its files.")
            }
            Self::Rename(Some(new)) => format!("Renamed {name} to {new}."),
            Self::Rename(None) => format!("{name} has its bundle's name again."),
            Self::Tags(tags) if tags.is_empty() => format!("Cleared {name}'s tags."),
            Self::Tags(tags) => {
                let tags: Vec<&str> = tags.iter().map(String::as_str).collect();
                format!("Tagged {name}: {}.", tags.join(", "))
            }
            Self::Category(Some(category)) => format!("Put {name} in {category}."),
            Self::Category(None) => format!("Took {name} out of its category."),
            Self::Move(folder) => format!("Moving {name} to {}.", folder.display()),
            Self::Queue(step) => {
                let place = match step {
                    QueueStep::Top => "to the top of",
                    QueueStep::Up => "up",
                    QueueStep::Down => "down",
                    QueueStep::Bottom => "to the bottom of",
                };
                format!("Moved {name} {place} the queue.")
            }
        }
    }
}

/// Tags typed as a comma-separated list: trimmed, without a leading `#`, empty ones dropped.
pub fn parse_tags(text: &str) -> BTreeSet<String> {
    text.split(',')
        .map(|tag| tag.trim().trim_start_matches('#').trim())
        .filter(|tag| !tag.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_are_split_on_commas_and_trimmed() {
        assert_eq!(
            parse_tags(" music, #live,,  field recordings ,music"),
            ["field recordings", "live", "music"]
                .map(str::to_owned)
                .into()
        );
        assert!(parse_tags(" , ").is_empty());
    }
}
