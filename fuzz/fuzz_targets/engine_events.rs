//! Decodes arbitrary bytes as the engine's event stream, cut into two pieces at an arbitrary
//! point, and checks that the cut changes nothing and that every event's data either decodes or
//! is refused without panicking.

#![no_main]

use libfuzzer_sys::fuzz_target;
use tempest_engine::api::client::ApiEvent;
use tempest_engine::api::sse::SseDecoder;

fuzz_target!(|input: (u16, &[u8])| {
    let (cut, bytes) = input;
    let Ok(whole) = SseDecoder::new().feed(bytes) else {
        return;
    };
    let cut = usize::from(cut) % (bytes.len() + 1);
    let mut decoder = SseDecoder::new();
    let mut pieces = decoder.feed(&bytes[..cut]).unwrap();
    pieces.extend(decoder.feed(&bytes[cut..]).unwrap());
    assert_eq!(pieces, whole);
    for event in &whole {
        let _ = ApiEvent::decode(event);
    }
});
