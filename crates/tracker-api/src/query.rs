//! The query string of `GET /tempest/v1/listings`: what to search for, which category and tags
//! to keep, how to sort and which page. The web UI writes it and the tracker reads it, so the two
//! always agree.
//!
//! A query writes only what differs from its default, so links stay short, and reads
//! `key=value` pairs, percent-encoded, ignoring keys it doesn't know. A value it can't read is an
//! error that names the key.

use std::fmt;

use crate::listing::MAX_PAGE;
use crate::text::{self, MAX_TAGS};

/// The longest search text, in characters.
pub const MAX_SEARCH: usize = 200;

/// Listings on a page unless the query asks for another number.
pub const DEFAULT_LIMIT: u32 = 25;

/// What listings are sorted by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Sort {
    /// How well they match the search text, weighted by how popular they are: their swarm's
    /// health, votes and recent downloads. Without search text, popularity alone.
    #[default]
    Relevance,
    /// The most recently listed first.
    Newest,
    /// The healthiest swarm first: the most complete peers, then the most peers.
    Availability,
    /// The most votes up, less votes down, first.
    Votes,
    /// The most downloads lately first, recent days counting most.
    Trending,
}

impl Sort {
    /// Every sort, in the order pickers show them.
    pub const ALL: [Sort; 5] = [
        Sort::Relevance,
        Sort::Newest,
        Sort::Availability,
        Sort::Votes,
        Sort::Trending,
    ];

    /// Its name in queries.
    pub fn key(self) -> &'static str {
        match self {
            Sort::Relevance => "relevance",
            Sort::Newest => "newest",
            Sort::Availability => "availability",
            Sort::Votes => "votes",
            Sort::Trending => "trending",
        }
    }

    /// The sort named `key`.
    pub fn named(key: &str) -> Option<Sort> {
        Sort::ALL.into_iter().find(|sort| sort.key() == key)
    }
}

/// Which listed bundles to show, and in what order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListingQuery {
    /// Search text, matched against titles, descriptions, tags, content names and categories,
    /// forgiving typos. Empty matches every listing. Key `q`.
    pub q: String,
    /// Only listings in this category. Key `category`.
    pub category: Option<String>,
    /// Only listings with every one of these tags, normalized. Key `tag`, once per tag.
    pub tags: Vec<String>,
    /// What to sort by. Key `sort`.
    pub sort: Sort,
    /// How many matches to skip. Key `offset`. A tracker may serve pages only so deep, such as
    /// the first 10,000 matches, and answer deeper ones with no listings.
    pub offset: u64,
    /// How many to return, from 1 to [`MAX_PAGE`]. Key `limit`.
    pub limit: u32,
}

impl Default for ListingQuery {
    /// The first page of every listing, most relevant first.
    fn default() -> Self {
        Self {
            q: String::new(),
            category: None,
            tags: Vec::new(),
            sort: Sort::Relevance,
            offset: 0,
            limit: DEFAULT_LIMIT,
        }
    }
}

/// Why a query string can't be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryError {
    /// The key whose value is wrong.
    pub key: String,
    /// What is wrong with it.
    pub problem: String,
}

impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.key, self.problem)
    }
}

impl std::error::Error for QueryError {}

fn error(key: &str, problem: impl Into<String>) -> QueryError {
    QueryError {
        key: key.to_owned(),
        problem: problem.into(),
    }
}

impl ListingQuery {
    /// Reads a query string, with or without its leading `?`.
    pub fn parse(query: &str) -> Result<Self, QueryError> {
        let mut out = Self::default();
        for (key, value) in pairs(query) {
            match key.as_str() {
                "q" => {
                    if value.chars().count() > MAX_SEARCH {
                        let problem = format!("is longer than {MAX_SEARCH} characters");
                        return Err(error(&key, problem));
                    }
                    out.q = value;
                }
                "category" => out.category = Some(value).filter(|c| !c.is_empty()),
                "tag" => {
                    let tag = text::normalize_tag(&value).map_err(|e| error(&key, e.0))?;
                    if !out.tags.contains(&tag) {
                        out.tags.push(tag);
                    }
                    if out.tags.len() > MAX_TAGS {
                        return Err(error(&key, format!("at most {MAX_TAGS} tags")));
                    }
                }
                "sort" => {
                    out.sort = Sort::named(&value).ok_or_else(|| {
                        let known: Vec<&str> = Sort::ALL.iter().map(|s| s.key()).collect();
                        error(&key, format!("{value:?} isn't one of {}", known.join(", ")))
                    })?;
                }
                "offset" => out.offset = number(&key, &value)?,
                "limit" => {
                    out.limit = number(&key, &value)?;
                    if !(1..=MAX_PAGE).contains(&out.limit) {
                        return Err(error(&key, format!("must be from 1 to {MAX_PAGE}")));
                    }
                }
                _ => {}
            }
        }
        Ok(out)
    }

    /// Writes the query string, without the `?`, leaving out defaults.
    pub fn to_query(&self) -> String {
        let default = Self::default();
        let mut pairs = Vec::new();
        if !self.q.is_empty() {
            pairs.push(("q", self.q.clone()));
        }
        if let Some(category) = &self.category {
            pairs.push(("category", category.clone()));
        }
        for tag in &self.tags {
            pairs.push(("tag", tag.clone()));
        }
        if self.sort != default.sort {
            pairs.push(("sort", self.sort.key().to_owned()));
        }
        if self.offset != default.offset {
            pairs.push(("offset", self.offset.to_string()));
        }
        if self.limit != default.limit {
            pairs.push(("limit", self.limit.to_string()));
        }
        pairs
            .iter()
            .map(|(key, value)| format!("{key}={}", encode(value)))
            .collect::<Vec<_>>()
            .join("&")
    }
}

fn number<T: std::str::FromStr>(key: &str, value: &str) -> Result<T, QueryError> {
    value
        .parse()
        .map_err(|_| error(key, format!("{value:?} isn't a whole number in range")))
}

/// `text` with every byte but the unreserved ones (RFC 3986) percent-encoded.
pub fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// `text` with percent-escapes decoded and `+` read as a space. Escapes that aren't two hex
/// digits stay as they are, and bytes that aren't UTF-8 become U+FFFD.
pub fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let escape = match bytes.get(i + 1..i + 3) {
            Some(&[high, low]) if bytes[i] == b'%' => hex(high).zip(hex(low)),
            _ => None,
        };
        match (bytes[i], escape) {
            (_, Some((high, low))) => {
                out.push(high << 4 | low);
                i += 2;
            }
            (b'+', None) => out.push(b' '),
            (byte, None) => out.push(byte),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(byte: u8) -> Option<u8> {
    (byte as char).to_digit(16).map(|d| d as u8)
}

/// The decoded `key=value` pairs of `query`, with or without its leading `?`. A pair without
/// `=` has an empty value.
fn pairs(query: &str) -> Vec<(String, String)> {
    query
        .strip_prefix('?')
        .unwrap_or(query)
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (decode(key), decode(value))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn defaults_write_nothing() {
        assert_eq!(ListingQuery::default().to_query(), "");
        assert_eq!(ListingQuery::parse("").unwrap(), ListingQuery::default());
        assert_eq!(ListingQuery::parse("?").unwrap(), ListingQuery::default());
    }

    #[test]
    fn reads_every_key() {
        let query =
            ListingQuery::parse("?q=open+weather%20data&category=datasets&tag=Climate&tag=geo&sort=trending&offset=50&limit=10&x=y")
                .unwrap();
        assert_eq!(
            query,
            ListingQuery {
                q: "open weather data".into(),
                category: Some("datasets".into()),
                tags: vec!["climate".into(), "geo".into()],
                sort: Sort::Trending,
                offset: 50,
                limit: 10,
            }
        );
        assert_eq!(
            query.to_query(),
            "q=open%20weather%20data&category=datasets&tag=climate&tag=geo&sort=trending&offset=50&limit=10"
        );
    }

    #[test]
    fn refuses_bad_values_by_key() {
        let key = |query: &str| ListingQuery::parse(query).unwrap_err().key;
        assert_eq!(key("sort=best"), "sort");
        assert_eq!(key("limit=0"), "limit");
        assert_eq!(key(&format!("limit={}", MAX_PAGE + 1)), "limit");
        assert_eq!(key("offset=-1"), "offset");
        assert_eq!(key("tag=c%2B%2B"), "tag");
        assert_eq!(key(&format!("q={}", "x".repeat(MAX_SEARCH + 1))), "q");
        let many: String = (0..=MAX_TAGS).map(|i| format!("tag=t{i}&")).collect();
        assert_eq!(key(&many), "tag");
    }

    #[test]
    fn repeated_tags_count_once() {
        let query = ListingQuery::parse("tag=ML&tag=ml&category=").unwrap();
        assert_eq!(query.tags, ["ml"]);
        assert_eq!(query.category, None);
    }

    fn listing_query() -> impl Strategy<Value = ListingQuery> {
        (
            "\\PC{0,40}",
            proptest::option::of("[a-z]{1,10}|\\PC{1,10}"),
            proptest::collection::vec("[a-z0-9]{1,8}(-[a-z0-9]{1,8})?", 0..=MAX_TAGS),
            0..Sort::ALL.len(),
            any::<u64>(),
            1..=MAX_PAGE,
        )
            .prop_map(|(q, category, tags, sort, offset, limit)| {
                let mut unique = Vec::new();
                for tag in tags {
                    if !unique.contains(&tag) {
                        unique.push(tag);
                    }
                }
                ListingQuery {
                    q,
                    category,
                    tags: unique,
                    sort: Sort::ALL[sort],
                    offset,
                    limit,
                }
            })
    }

    proptest! {
        #[test]
        fn any_query_round_trips(query in listing_query()) {
            prop_assert_eq!(ListingQuery::parse(&query.to_query()).unwrap(), query);
        }

        #[test]
        fn any_text_decodes_back_from_its_encoding(text in ".*") {
            prop_assert_eq!(decode(&encode(&text)), text);
        }

        #[test]
        fn whatever_parses_round_trips(text in ".*") {
            if let Ok(query) = ListingQuery::parse(&text) {
                prop_assert_eq!(ListingQuery::parse(&query.to_query()).unwrap(), query);
            }
        }
    }
}
