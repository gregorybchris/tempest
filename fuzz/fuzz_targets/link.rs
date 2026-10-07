//! Parses links from arbitrary text and checks that whatever parses formats to a link that parses
//! back to the same value.

#![no_main]

use libfuzzer_sys::fuzz_target;
use tempest_protocol::Link;

fuzz_target!(|text: &str| {
    if let Ok(link) = text.parse::<Link>() {
        assert_eq!(link.to_string().parse::<Link>(), Ok(link));
    }
});
