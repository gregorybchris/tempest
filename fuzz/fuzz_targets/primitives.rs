//! Decodes the shared wire structures from arbitrary bytes and checks that whatever decodes
//! re-encodes to the same bytes.

#![no_main]

use libfuzzer_sys::fuzz_target;
use tempest_protocol::{Address, Decode, Encode, Hash, RangeList, Reader};

fuzz_target!(|data: &[u8]| {
    if let Ok(address) = Address::from_bytes(data) {
        assert_eq!(address.to_bytes(), data);
    }
    if let Ok(ranges) = RangeList::from_bytes(data) {
        assert_eq!(ranges.to_bytes(), data);
    }
    let _ = Reader::new(data).list::<Hash>();
    let _ = Reader::new(data).list::<String>();
});
