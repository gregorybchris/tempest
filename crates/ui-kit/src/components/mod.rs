//! The building blocks every web UI shares: navigation, buttons, badges, panels, tables,
//! drawers, dialogs, toasts and key hints, and the form controls of [`crate::form`]. Each
//! renders the stylesheet's classes.

mod chrome;
mod controls;
mod layout;
mod marks;
mod overlay;
mod table;
mod toast;

pub use self::chrome::*;
pub use self::controls::*;
pub use self::layout::*;
pub use self::marks::*;
pub use self::overlay::*;
pub use self::table::*;
pub use self::toast::*;
pub use crate::form::*;

/// An accent, for text and marks that say how something is going.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Tone {
    /// Plain text.
    #[default]
    Neutral,
    /// Quiet: stopped, queued, done.
    Dim,
    /// Downloading.
    Down,
    /// Uploading or sharing.
    Up,
    /// Needs attention now: checking, moving.
    Hot,
    /// Something is wrong but may fix itself: stalled.
    Warn,
    /// Something failed.
    Bad,
}

impl Tone {
    /// Its class, which sets `--tone`.
    pub fn class(self) -> &'static str {
        match self {
            Tone::Neutral => "tone--neutral",
            Tone::Dim => "tone--dim",
            Tone::Down => "tone--down",
            Tone::Up => "tone--up",
            Tone::Hot => "tone--hot",
            Tone::Warn => "tone--warn",
            Tone::Bad => "tone--bad",
        }
    }
}
