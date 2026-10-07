//! Decodes LAN messages from arbitrary datagrams and checks that whatever decodes re-encodes to
//! the same bytes.

#![no_main]

use libfuzzer_sys::fuzz_target;
use tempest_protocol::LanMessage;

fuzz_target!(|datagram: &[u8]| {
    if let Some(message) = LanMessage::decode(datagram) {
        assert_eq!(message.encode().as_slice(), datagram);
    }
});
