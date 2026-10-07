//! Decodes relay datagrams from arbitrary bytes and checks that whatever decodes re-encodes to
//! the same bytes.

#![no_main]

use libfuzzer_sys::fuzz_target;
use tempest_protocol::RelayDatagram;

fuzz_target!(|datagram: &[u8]| {
    if let Ok(decoded) = RelayDatagram::decode(datagram) {
        assert_eq!(decoded.encode(), datagram);
    }
});
