//! Parses relay identities from arbitrary text and checks that whatever parses formats to text
//! that parses back to the same value.

#![no_main]

use libfuzzer_sys::fuzz_target;
use tempest_protocol::RelayIdentity;

fuzz_target!(|text: &str| {
    if let Ok(identity) = text.parse::<RelayIdentity>() {
        assert_eq!(identity.to_string().parse::<RelayIdentity>(), Ok(identity));
    }
});
