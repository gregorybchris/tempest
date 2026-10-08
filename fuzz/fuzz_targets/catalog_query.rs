//! Parses arbitrary text as the catalog's listing query string, and checks that whatever parses
//! writes a query string that parses back to the same query, within the query's limits.

#![no_main]

use libfuzzer_sys::fuzz_target;
use tempest_catalog_api::listing::MAX_PAGE;
use tempest_catalog_api::query::{ListingQuery, MAX_SEARCH};
use tempest_catalog_api::text::{self, MAX_TAGS};

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(query) = ListingQuery::parse(text) else {
        return;
    };
    assert_eq!(ListingQuery::parse(&query.to_query()), Ok(query.clone()));
    assert!(query.q.chars().count() <= MAX_SEARCH);
    assert!((1..=MAX_PAGE).contains(&query.limit));
    assert!(query.tags.len() <= MAX_TAGS);
    for tag in &query.tags {
        assert_eq!(text::normalize_tag(tag).as_ref(), Ok(tag));
    }
});
