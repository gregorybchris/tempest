//! Parses every tracker API type from arbitrary JSON and checks that whatever parses serializes to
//! JSON that parses back to the same value.

#![no_main]

use libfuzzer_sys::fuzz_target;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tempest_protocol::tracker::{AnnounceRequest, AnnounceResponse, BundleStats, ErrorResponse};

fn check<T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(json: &[u8]) {
    if let Ok(value) = serde_json::from_slice::<T>(json) {
        let reserialized = serde_json::to_vec(&value).unwrap();
        assert_eq!(serde_json::from_slice::<T>(&reserialized).unwrap(), value);
    }
}

fuzz_target!(|json: &[u8]| {
    check::<AnnounceRequest>(json);
    check::<AnnounceResponse>(json);
    check::<BundleStats>(json);
    check::<ErrorResponse>(json);
});
