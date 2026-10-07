//! Verifies arbitrary Data against arbitrary file sizes and requests.

#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use tempest_protocol::Hash;
use tempest_protocol::tree::verify_data;

#[derive(Debug, Arbitrary)]
struct Input<'a> {
    size: u64,
    root: [u8; 32],
    position: u32,
    count: u32,
    proof: Vec<[u8; 32]>,
    chunks: &'a [u8],
}

fuzz_target!(|input: Input<'_>| {
    let proof: Vec<Hash> = input.proof.into_iter().map(Hash::new).collect();
    let _ = verify_data(
        input.size,
        &Hash::new(input.root),
        input.position,
        input.count,
        &proof,
        input.chunks,
    );
});
