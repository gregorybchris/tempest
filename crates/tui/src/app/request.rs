//! What the app asks the runtime to do, and how it reports the outcome.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde_json::Value;
use tempest_engine::api::{AddRequest, QueueStep};
use tempest_engine::inspect::{ChunkMapInfo, FileInfo, PeerInfo};
use tempest_engine::{SettingsView, TransferInfo};
use tempest_protocol::BundleId;

use super::Look;

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
    /// Replace its own settings, by dotted key.
    Settings(BTreeMap<String, Value>),
    /// Withdraw its listing from this tracker's catalog.
    Unpublish(String),
}

/// Something the app can't do itself: a request to the engine, or opening a folder.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    /// Act on a transfer.
    Transfer(Target, Action),
    /// Add a download from a link.
    Add(Box<AddRequest>),
    /// Share a file or folder.
    Share(Box<AddRequest>),
    /// Fetch the categories.
    Categories,
    /// Fetch the settings and the categories.
    Settings,
    /// Change settings by dotted key.
    ChangeSettings {
        /// The new values.
        changes: BTreeMap<String, Value>,
        /// What they are, for the notice: "the download limit". Without it, success goes
        /// unannounced.
        about: Option<String>,
    },
    /// Give a setting the config file's value again.
    ResetSetting {
        /// The setting, by dotted key.
        key: String,
        /// What it is, for the notice.
        about: String,
    },
    /// Add a category, or change its save folder.
    SetCategory {
        /// Its name.
        name: String,
        /// Its save folder.
        folder: PathBuf,
        /// Whether it is new.
        new: bool,
    },
    /// Remove a category.
    RemoveCategory(String),
    /// Put text on the clipboard through the terminal (OSC 52).
    Copy {
        /// The text.
        text: String,
        /// What it is, for the notice: "field-recordings's link".
        what: String,
    },
    /// Open a folder on this machine.
    Open(PathBuf),
    /// Fetch something from inside a transfer.
    Look(BundleId, Look),
}

/// What a request that succeeded brought back, where the app needs it.
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    /// Nothing beyond success: the change arrives on the event stream.
    Done,
    /// The name of the transfer just added.
    Added(String),
    /// The share just made.
    Shared(Box<TransferInfo>),
    /// Every category's save folder, by name.
    Categories(BTreeMap<String, PathBuf>),
    /// The settings, after a change.
    Settings(Box<SettingsView>),
    /// The settings, and every category's save folder.
    Loaded(Box<SettingsView>, BTreeMap<String, PathBuf>),
    /// A transfer's files.
    Files(Vec<FileInfo>),
    /// A transfer's connections.
    Peers(Vec<PeerInfo>),
    /// A transfer's chunk map.
    Chunks(ChunkMapInfo),
}

impl Request {
    /// The notice for its success, if it deserves one.
    pub(super) fn success(&self, reply: &Reply) -> Option<String> {
        Some(match (self, reply) {
            (Self::Transfer(target, action), _) => action.success(&target.name),
            (Self::Add(_), Reply::Added(name)) => format!("Added {name}."),
            (Self::Add(_), _) => "Added the link.".to_owned(),
            (Self::Share(_), Reply::Shared(transfer)) => {
                format!("Shared {}. Press y to copy its link.", transfer.name)
            }
            (Self::Share(_), _) => "Shared it.".to_owned(),
            (Self::Categories | Self::Settings | Self::Look(..), _) => return None,
            (Self::ChangeSettings { about, .. }, _) => format!("Saved {}.", about.as_ref()?),
            (Self::ResetSetting { about, .. }, _) => {
                format!("Gave {about} the config file's value again.")
            }
            (
                Self::SetCategory {
                    name, new: true, ..
                },
                _,
            ) => format!("Added the category {name}."),
            (Self::SetCategory { name, folder, .. }, _) => {
                format!("{name} now saves to {}.", folder.display())
            }
            (Self::RemoveCategory(name), _) => format!("Removed the category {name}."),
            (Self::Copy { what, .. }, _) => format!("Copied {what} to the clipboard."),
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
            Self::Share(add) => {
                let path = add.path.as_deref().unwrap_or_else(|| "it".as_ref());
                format!("Couldn't share {}: {error}", path.display())
            }
            Self::Categories => format!("Couldn't load the categories: {error}"),
            Self::Settings => format!("Couldn't load the settings: {error}"),
            Self::ChangeSettings { about, .. } => {
                let about = about.as_deref().unwrap_or("the settings");
                format!("Couldn't save {about}: {error}")
            }
            Self::ResetSetting { about, .. } => format!("Couldn't reset {about}: {error}"),
            Self::SetCategory { name, .. } => {
                format!("Couldn't save the category {name}: {error}")
            }
            Self::RemoveCategory(name) => {
                format!("Couldn't remove the category {name}: {error}")
            }
            Self::Copy { what, .. } => format!("Couldn't copy {what}: {error}"),
            Self::Open(path) => format!("Couldn't open {}: {error}", path.display()),
            Self::Look(_, look) => {
                let what = match look {
                    Look::Files => "files",
                    Look::Peers => "peers",
                    Look::Chunks(_) => "chunk map",
                };
                format!("Couldn't load the {what}: {error}")
            }
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
            Self::Settings(_) => "set the limits of",
            Self::Unpublish(_) => "unpublish",
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
            Self::Settings(_) => format!("Saved {name}'s limits."),
            Self::Unpublish(tracker) => format!("Withdrew {name} from {tracker}'s catalog."),
        }
    }
}

pub use tempest_api::settings::parse_tags;
