//! Decodes relay control messages from arbitrary stream bytes and checks that whatever decodes
//! re-encodes to the same frame, apart from an unknown RelayReject reason code.

#![no_main]

use libfuzzer_sys::fuzz_target;
use tempest_protocol::RelayMessage;
use tempest_protocol::relay::{RelayRejectReason, split_relay_frame};

fuzz_target!(|data: &[u8]| {
    let mut buf = data;
    while let Ok(Some((frame, used))) = split_relay_frame(buf) {
        let original = &buf[..used];
        buf = &buf[used..];
        let Ok(message) = RelayMessage::decode(frame) else {
            continue;
        };
        let encoded = message.encode_frame();
        if let RelayMessage::RelayReject(reject) = &message {
            let code = u16::from_be_bytes([original[5], original[6]]);
            assert_eq!(RelayRejectReason::from_code(code), reject.reason);
            assert_eq!(encoded[..5], original[..5]);
            assert_eq!(encoded[7..], original[7..]);
        } else {
            assert_eq!(encoded, original);
        }
        let (frame, _) = split_relay_frame(&encoded).unwrap().unwrap();
        assert_eq!(RelayMessage::decode(frame), Ok(message));
    }
});
