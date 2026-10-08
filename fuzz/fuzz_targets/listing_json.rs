//! Parses every listing and sign-in request type from arbitrary JSON, checks that whatever parses
//! serializes to JSON that parses back to the same value, and that the text rules hold for
//! whatever text they accept.

#![no_main]

use libfuzzer_sys::fuzz_target;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tempest_tracker_api::account::{AccountEdit, Poll, SignIn, normalize_code};
use tempest_tracker_api::listing::{ListingEdit, NewComment, VoteRequest};
use tempest_tracker_api::text;

fn check<T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(json: &[u8]) {
    if let Ok(value) = serde_json::from_slice::<T>(json) {
        let reserialized = serde_json::to_vec(&value).unwrap();
        assert_eq!(serde_json::from_slice::<T>(&reserialized).unwrap(), value);
    }
}

fuzz_target!(|json: &[u8]| {
    check::<SignIn>(json);
    check::<Poll>(json);
    check::<AccountEdit>(json);
    check::<ListingEdit>(json);
    check::<VoteRequest>(json);
    check::<NewComment>(json);

    if let Ok(text) = std::str::from_utf8(json) {
        let code = normalize_code(text);
        assert_eq!(normalize_code(&code), code);
        if let Ok(tag) = text::normalize_tag(text) {
            assert_eq!(text::normalize_tag(&tag), Ok(tag));
        }
        if let Ok(description) = text::clean_description(text) {
            assert_eq!(text::clean_description(&description), Ok(description));
        }
        if let Ok(title) = text::clean_title(text) {
            assert_eq!(text::clean_title(&title), Ok(title));
        }
    }
});
