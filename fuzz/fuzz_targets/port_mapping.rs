//! Parses routers' PCP and NAT-PMP responses from arbitrary bytes, and checks that a PCP MAP
//! response carries the fields at the offsets a request puts them.

#![no_main]

use libfuzzer_sys::fuzz_target;
use tempest_net::portmap::wire::{self, PCP_MAP_LEN, PcpResponse};

fuzz_target!(|response: &[u8]| {
    let _ = wire::parse_natpmp(response);
    if let Ok(PcpResponse::Map(map)) = wire::parse_pcp(response) {
        assert!(response.len() >= PCP_MAP_LEN);
        assert_eq!(map.nonce, response[24..36]);
        assert_eq!(map.internal_port.to_be_bytes(), response[40..42]);
    }
});
