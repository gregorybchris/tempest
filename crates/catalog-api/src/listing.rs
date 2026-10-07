//! Listings, votes and comments.
//!
//! A bundle is listed when its sharer asks (D-129): by an announce's `listing` field
//! (`protocol/trackers.md`), or by a signed-in [`ListingEdit`]. The listing belongs to that key,
//! and shows once the tracker has the bundle's manifest; until then it is
//! [`ListingStatus::Pending`] and only its publisher sees it.

use serde::{Deserialize, Deserializer, Serialize};
use tempest_protocol::tracker::BundleStats;
use tempest_protocol::{BundleId, PeerId};

/// Someone, as the catalog shows them: a publisher or a comment's author.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Person {
    /// Their key.
    pub key: PeerId,
    /// The name they chose, if any.
    pub name: Option<String>,
}

/// Whether a listing shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListingStatus {
    /// Asked for, but the tracker doesn't have the manifest yet. Only the publisher sees it.
    Pending,
    /// Shown to everyone.
    Listed,
    /// Taken down by its publisher. Only the publisher sees it, and only an edit lists it again;
    /// announces don't.
    Withdrawn,
}

/// What a bundle holds, from its manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Content {
    /// The bundle's name: its single file's, or its top folder's.
    pub name: String,
    /// The total size of its files in bytes.
    pub size: u64,
    /// The number of files.
    pub files: u64,
}

/// A listing: the answer to `GET /catalog/v1/listings/{bundle}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listing {
    /// The listed bundle.
    pub bundle: BundleId,
    /// The publisher's title, or else the content's name, or else empty.
    pub title: String,
    /// The publisher's description, possibly empty.
    pub description: String,
    /// The category's slug, if the publisher chose one.
    pub category: Option<String>,
    /// The tags, in the publisher's order.
    pub tags: Vec<String>,
    /// Who listed it.
    pub publisher: Person,
    /// Whether it shows.
    pub status: ListingStatus,
    /// When it was first asked for, in seconds since the Unix epoch.
    pub created: u64,
    /// When the publisher last changed it, in seconds since the Unix epoch.
    pub updated: u64,
    /// What the bundle holds, once the tracker has its manifest.
    pub content: Option<Content>,
    /// The votes it has.
    pub votes: VoteCounts,
    /// The session's vote, or [`Vote::None`] without a session.
    pub my_vote: Vote,
    /// The number of comments.
    pub comments: u64,
    /// The swarm as the tracker sees it now.
    pub swarm: BundleStats,
}

/// The votes a listing has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct VoteCounts {
    /// Votes up.
    pub up: u64,
    /// Votes down.
    pub down: u64,
}

/// One account's vote on a listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Vote {
    /// For it.
    Up,
    /// Against it.
    Down,
    /// Neither: no vote, or a vote taken back.
    #[default]
    None,
}

/// The body of `PUT /catalog/v1/listings/{bundle}/vote`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoteRequest {
    /// The vote, replacing any earlier one.
    pub vote: Vote,
}

/// The body of `PUT /catalog/v1/listings/{bundle}`: asks for the bundle to be listed under the
/// session's key, or changes the session's listing. Fields left out stay as they are.
///
/// The rules in [`crate::text`] say what each field accepts.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ListingEdit {
    /// The title; empty shows the content's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The category's slug, or `null` for none.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub category: Option<Option<String>>,
    /// The tags, replacing the old ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
}

/// Deserializes a field that is there, possibly as `null`, into `Some`; with `default`, a field
/// that isn't there stays `None`.
fn present<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(d).map(Some)
}

/// A page of listings: the answer to `GET /catalog/v1/listings` and
/// `GET /catalog/v1/me/listings`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListingPage {
    /// The listings on this page.
    pub listings: Vec<Listing>,
    /// How many match, on every page.
    pub total: u64,
}

/// The most listings on one page.
pub const MAX_PAGE: u32 = 100;

/// The query string of `GET /catalog/v1/listings`: the newest listed bundles first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListingQuery {
    /// Only listings in this category.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// How many matches to skip.
    #[serde(default)]
    pub offset: u64,
    /// How many to return, at most [`MAX_PAGE`].
    #[serde(default = "default_limit")]
    pub limit: u32,
}

impl Default for ListingQuery {
    fn default() -> Self {
        Self {
            category: None,
            offset: 0,
            limit: default_limit(),
        }
    }
}

fn default_limit() -> u32 {
    25
}

/// A comment on a listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    /// Its identifier, for `DELETE /catalog/v1/comments/{id}`.
    pub id: u64,
    /// Who wrote it.
    pub author: Person,
    /// What they wrote.
    pub body: String,
    /// When, in seconds since the Unix epoch.
    pub created: u64,
}

/// The body of `POST /catalog/v1/listings/{bundle}/comments`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewComment {
    /// What to say. See [`crate::text::clean_comment`].
    pub body: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn edits_tell_a_null_category_from_a_missing_one() {
        let edit: ListingEdit = serde_json::from_value(json!({ "title": "x" })).unwrap();
        assert_eq!(edit.category, None);
        let edit: ListingEdit = serde_json::from_value(json!({ "category": null })).unwrap();
        assert_eq!(edit.category, Some(None));
        let edit: ListingEdit = serde_json::from_value(json!({ "category": "models" })).unwrap();
        assert_eq!(edit.category, Some(Some("models".into())));
        assert_eq!(
            serde_json::to_value(&edit).unwrap(),
            json!({ "category": "models" })
        );
    }

    #[test]
    fn queries_default_to_the_first_page() {
        let query: ListingQuery = serde_json::from_value(json!({})).unwrap();
        assert_eq!(query, ListingQuery::default());
    }

    #[test]
    fn votes_are_words() {
        let request: VoteRequest = serde_json::from_value(json!({ "vote": "down" })).unwrap();
        assert_eq!(request.vote, Vote::Down);
        assert!(serde_json::from_value::<VoteRequest>(json!({ "vote": 1 })).is_err());
    }
}
