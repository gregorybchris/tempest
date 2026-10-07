//! Splits peer message frames from arbitrary stream bytes.

#![no_main]

use libfuzzer_sys::fuzz_target;
use tempest_protocol::message::{MessageType, split_message_frame};

fuzz_target!(|data: &[u8]| {
    let mut buf = data;
    while let Ok(Some((frame, used))) = split_message_frame(buf) {
        let message_type = MessageType::from_code(frame.message_type).unwrap();
        assert!(frame.body.len() < message_type.max_frame_length() as usize);
        buf = &buf[used..];
    }
});
