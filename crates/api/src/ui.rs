//! The clients' shared look and views, kept in the engine's `ui` settings (D-195).

use serde::{Deserialize, Serialize};

/// The clients' look and saved views. The engine only keeps them: names it doesn't know are
/// left for the clients to ignore.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiSettings {
    /// The palette: `green`, `amber`, `vector` or `beige` (D-089).
    pub theme: String,
    /// The transfers list's optional columns to show, by name; `null` for the clients'
    /// default. Name, progress and status always show.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub columns: Option<Vec<String>>,
    /// Saved views of the transfers list.
    pub views: Vec<SavedView>,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            theme: "green".to_owned(),
            columns: None,
            views: Vec::new(),
        }
    }
}

/// A saved view of the transfers list: its filters and order, under a name.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SavedView {
    /// The view's name.
    pub name: String,
    /// The filter as typed: words matching the name, `#word` a tag.
    pub query: String,
    /// The status to show only, such as `downloading`, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// What to sort by, such as `size`; empty for queue order.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub sort: String,
    /// Against the sort's natural direction.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub reversed: bool,
}
