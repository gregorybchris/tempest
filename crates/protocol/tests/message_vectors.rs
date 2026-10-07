//! The framed Hello test vector from `protocol/messages.md`.

use tempest_protocol::message::{Hello, Message, split_message_frame};
use tempest_protocol::{Address, Hash};

const FRAME: &str = "0000004d01207855f013f5a4066c9987ee3fa4b0ae63c8eeee02035634587286457cf5b85a000000000000000d74656d706573742f302e312e3000000040000000200000000104cb007105115100000000";

fn hello() -> Message {
    Hello {
        bundle_id: "207855f013f5a4066c9987ee3fa4b0ae63c8eeee02035634587286457cf5b85a"
            .parse::<Hash>()
            .unwrap(),
        capabilities: vec![],
        client: "tempest/0.1.0".into(),
        max_request_chunks: 64,
        max_open_requests: 32,
        addresses: vec![Address::from(
            "203.0.113.5:4433".parse::<std::net::SocketAddr>().unwrap(),
        )],
        relay: None,
    }
    .into()
}

#[test]
fn framed_hello_encodes_exactly() {
    let bytes = hello().encode_frame();
    assert_eq!(bytes.len(), 81);
    assert_eq!(hex::encode(bytes), FRAME);
}

#[test]
fn framed_hello_decodes_exactly() {
    let bytes = hex::decode(FRAME).unwrap();
    let (frame, used) = split_message_frame(&bytes).unwrap().unwrap();
    assert_eq!(used, 81);
    assert_eq!(Message::decode(frame), Ok(hello()));
}
