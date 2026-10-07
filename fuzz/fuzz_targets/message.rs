//! Decodes peer messages from arbitrary stream bytes and checks that whatever decodes re-encodes
//! to the same frame, apart from Hello's ignored tail and an unknown Reject reason code.

#![no_main]

use libfuzzer_sys::fuzz_target;
use tempest_protocol::frame::HEADER_LEN;
use tempest_protocol::message::{Message, RejectReason, split_message_frame};

fuzz_target!(|data: &[u8]| {
    let mut buf = data;
    while let Ok(Some((frame, used))) = split_message_frame(buf) {
        let original = &buf[..used];
        buf = &buf[used..];
        let Ok(message) = Message::decode(frame) else {
            continue;
        };
        let encoded = message.encode_frame();
        match &message {
            Message::Hello(_) => {
                assert_eq!(encoded[4], original[4]);
                assert!(original[HEADER_LEN..].starts_with(&encoded[HEADER_LEN..]));
            }
            Message::Reject(reject) => {
                let code = u16::from_be_bytes([original[5], original[6]]);
                assert_eq!(RejectReason::from_code(code), reject.reason);
                assert_eq!(encoded[..5], original[..5]);
                assert_eq!(encoded[7..], original[7..]);
            }
            _ => assert_eq!(encoded, original),
        }
        let (frame, _) = split_message_frame(&encoded).unwrap().unwrap();
        assert_eq!(Message::decode(frame), Ok(message));
    }
});
