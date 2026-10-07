//! The binary primitives every message is built from (`protocol/messages.md`, Encoding).
//!
//! All integers are big-endian. Every value has exactly one valid encoding, and decoding rejects
//! anything else, so any structure has exactly one byte form.

use crate::hash::{HASH_LEN, Hash};

/// Why bytes could not be decoded. Every variant except [`DecodeError::UnknownDatagramKind`] is a
/// protocol violation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    /// The input ended before the value did.
    #[error("unexpected end of input")]
    UnexpectedEnd,
    /// Bytes were left over after the value.
    #[error("{0} unexpected trailing bytes")]
    TrailingBytes(usize),
    /// A `bool` was neither 0 nor 1.
    #[error("invalid bool {0}")]
    InvalidBool(u8),
    /// A `string` was not valid UTF-8.
    #[error("string is not valid UTF-8")]
    InvalidUtf8,
    /// An address had a family other than 4 or 6.
    #[error("unknown address family {0}")]
    UnknownAddressFamily(u8),
    /// An IPv4 address was encoded in IPv4-mapped IPv6 form instead of with family 4.
    #[error("IPv4-mapped IPv6 address must be encoded with family 4")]
    MappedIpv4Address,
    /// A list of ranges was not sorted, had an empty range, or had overlapping or touching ranges.
    #[error("range list is not canonical")]
    InvalidRangeList,
    /// A frame had a message type this protocol doesn't define.
    #[error("unknown message type {0:#04x}")]
    UnknownMessageType(u8),
    /// A manifest had a format version other than 1.
    #[error("unknown manifest version {0}")]
    UnknownManifestVersion(u8),
    /// A manifest entry had an unknown kind.
    #[error("unknown manifest entry kind {0}")]
    UnknownEntryKind(u8),
    /// A manifest decoded but broke a rule.
    #[error("invalid manifest: {0}")]
    InvalidManifest(Box<crate::manifest::ManifestError>),
    /// A message decoded but broke a rule.
    #[error("invalid message: {0}")]
    InvalidMessage(#[from] crate::message::MessageError),
    /// A relay datagram had a kind other than Forward or Forwarded. The receiver drops it; unlike
    /// the other variants, this is not a protocol violation.
    #[error("unknown relay datagram kind {0}")]
    UnknownDatagramKind(u8),
    /// A frame had a length of zero, so no message type.
    #[error("frame has no message type")]
    EmptyFrame,
    /// A frame's length exceeded the limit for its message type.
    #[error("frame length {length} exceeds the limit of {max} for its message type")]
    FrameTooLong {
        /// The frame's declared length.
        length: u32,
        /// The limit for its message type.
        max: u32,
    },
}

/// A value with a wire encoding.
pub trait Encode {
    /// Appends this value's encoding to `w`.
    fn encode(&self, w: &mut Writer);

    /// Returns this value's encoding.
    fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer::new();
        self.encode(&mut w);
        w.into_bytes()
    }
}

/// A value that can be decoded from its wire encoding.
pub trait Decode: Sized {
    /// Reads one value from `r`, leaving any following bytes unread.
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError>;

    /// Decodes a value that must occupy all of `bytes`.
    fn from_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(bytes);
        let value = Self::decode(&mut r)?;
        r.finish()?;
        Ok(value)
    }
}

/// Reads primitives from a byte slice, failing on anything that isn't a valid encoding.
#[derive(Debug, Clone)]
pub struct Reader<'a> {
    buf: &'a [u8],
}

impl<'a> Reader<'a> {
    /// Creates a reader over `buf`.
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf }
    }

    /// The number of bytes not yet read.
    pub fn remaining(&self) -> usize {
        self.buf.len()
    }

    /// Fails if any bytes are left.
    pub fn finish(&self) -> Result<(), DecodeError> {
        match self.buf.len() {
            0 => Ok(()),
            n => Err(DecodeError::TrailingBytes(n)),
        }
    }

    /// Reads exactly `n` bytes.
    pub fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        if n > self.buf.len() {
            return Err(DecodeError::UnexpectedEnd);
        }
        let (head, tail) = self.buf.split_at(n);
        self.buf = tail;
        Ok(head)
    }

    /// Reads all remaining bytes.
    pub fn rest(&mut self) -> &'a [u8] {
        std::mem::take(&mut self.buf)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        let bytes = self.take(N)?;
        Ok(bytes.try_into().expect("take returned N bytes"))
    }

    /// Reads a `u8`.
    pub fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.array::<1>()?[0])
    }

    /// Reads a big-endian `u16`.
    pub fn u16(&mut self) -> Result<u16, DecodeError> {
        Ok(u16::from_be_bytes(self.array()?))
    }

    /// Reads a big-endian `u32`.
    pub fn u32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_be_bytes(self.array()?))
    }

    /// Reads a big-endian `u64`.
    pub fn u64(&mut self) -> Result<u64, DecodeError> {
        Ok(u64::from_be_bytes(self.array()?))
    }

    /// Reads a big-endian two's-complement `i64`.
    pub fn i64(&mut self) -> Result<i64, DecodeError> {
        Ok(i64::from_be_bytes(self.array()?))
    }

    /// Reads a `bool`, which must be 0 or 1.
    pub fn bool(&mut self) -> Result<bool, DecodeError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            other => Err(DecodeError::InvalidBool(other)),
        }
    }

    /// Reads a 32-byte `hash`.
    pub fn hash(&mut self) -> Result<Hash, DecodeError> {
        Ok(Hash::new(self.array::<HASH_LEN>()?))
    }

    /// Reads `bytes`: a `u32` length, then that many bytes.
    pub fn bytes(&mut self) -> Result<&'a [u8], DecodeError> {
        let len = self.u32()?;
        self.take(len as usize)
    }

    /// Reads a `string`: encoded like `bytes`, and must be valid UTF-8.
    pub fn string(&mut self) -> Result<&'a str, DecodeError> {
        std::str::from_utf8(self.bytes()?).map_err(|_| DecodeError::InvalidUtf8)
    }

    /// Reads a `list<T>`: a `u32` count, then that many values.
    pub fn list<T: Decode>(&mut self) -> Result<Vec<T>, DecodeError> {
        let count = self.u32()? as usize;
        // Every listed type takes at least one byte, so a count larger than the remaining input
        // is already invalid. Capping the allocation stops a forged count from reserving memory.
        let mut items = Vec::with_capacity(count.min(self.remaining()));
        for _ in 0..count {
            items.push(T::decode(self)?);
        }
        Ok(items)
    }
}

/// Builds an encoding by appending primitives.
#[derive(Debug, Clone, Default)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    /// Creates an empty writer.
    pub fn new() -> Self {
        Self::default()
    }

    /// The bytes written so far.
    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    /// Appends raw bytes with no length prefix.
    pub fn raw(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Appends a `u8`.
    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    /// Appends a big-endian `u16`.
    pub fn u16(&mut self, v: u16) {
        self.raw(&v.to_be_bytes());
    }

    /// Appends a big-endian `u32`.
    pub fn u32(&mut self, v: u32) {
        self.raw(&v.to_be_bytes());
    }

    /// Appends a big-endian `u64`.
    pub fn u64(&mut self, v: u64) {
        self.raw(&v.to_be_bytes());
    }

    /// Appends a big-endian two's-complement `i64`.
    pub fn i64(&mut self, v: i64) {
        self.raw(&v.to_be_bytes());
    }

    /// Appends a `bool` as 0 or 1.
    pub fn bool(&mut self, v: bool) {
        self.u8(v.into());
    }

    /// Appends a 32-byte `hash`.
    pub fn hash(&mut self, v: &Hash) {
        self.raw(v.as_bytes());
    }

    /// Appends `bytes`: a `u32` length, then the bytes.
    ///
    /// # Panics
    ///
    /// If `v` is 4 GiB or longer, which no Tempest message allows.
    pub fn bytes(&mut self, v: &[u8]) {
        self.u32(u32::try_from(v.len()).expect("bytes longer than u32::MAX"));
        self.raw(v);
    }

    /// Appends a `string`, encoded like `bytes`.
    ///
    /// # Panics
    ///
    /// If `v` is 4 GiB or longer.
    pub fn string(&mut self, v: &str) {
        self.bytes(v.as_bytes());
    }

    /// Appends a `list<T>`: a `u32` count, then each value.
    ///
    /// # Panics
    ///
    /// If `items` has `u32::MAX` or more values.
    pub fn list<T: Encode>(&mut self, items: &[T]) {
        self.u32(u32::try_from(items.len()).expect("list longer than u32::MAX"));
        for item in items {
            item.encode(self);
        }
    }
}

impl Encode for Hash {
    fn encode(&self, w: &mut Writer) {
        w.hash(self);
    }
}

impl Decode for Hash {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        r.hash()
    }
}

impl Encode for String {
    fn encode(&self, w: &mut Writer) {
        w.string(self);
    }
}

impl Decode for String {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        r.string().map(str::to_owned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integers_are_big_endian() {
        let mut w = Writer::new();
        w.u16(0x0102);
        w.u32(0x0304_0506);
        w.u64(0x0708_090a_0b0c_0d0e);
        w.i64(-2);
        assert_eq!(
            w.into_bytes(),
            hex::decode("0102030405060708090a0b0c0d0efffffffffffffffe").unwrap()
        );
    }

    #[test]
    fn bool_must_be_zero_or_one() {
        assert_eq!(Reader::new(&[0]).bool(), Ok(false));
        assert_eq!(Reader::new(&[1]).bool(), Ok(true));
        assert_eq!(Reader::new(&[2]).bool(), Err(DecodeError::InvalidBool(2)));
    }

    #[test]
    fn string_must_be_utf8() {
        let bytes = [0, 0, 0, 2, 0xc3, 0x28];
        assert_eq!(Reader::new(&bytes).string(), Err(DecodeError::InvalidUtf8));
    }

    #[test]
    fn bytes_length_past_end_is_rejected() {
        let bytes = [0, 0, 0, 5, 1, 2];
        assert_eq!(Reader::new(&bytes).bytes(), Err(DecodeError::UnexpectedEnd));
    }

    #[test]
    fn huge_list_count_does_not_allocate() {
        let bytes = [0xff, 0xff, 0xff, 0xff];
        assert_eq!(
            Reader::new(&bytes).list::<Hash>(),
            Err(DecodeError::UnexpectedEnd)
        );
    }

    #[test]
    fn from_bytes_rejects_trailing_bytes() {
        let mut bytes = vec![0; 33];
        bytes[32] = 7;
        assert_eq!(Hash::from_bytes(&bytes), Err(DecodeError::TrailingBytes(1)));
    }
}
