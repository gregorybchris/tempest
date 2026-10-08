//! Framing of messages on streams, with size limits (`protocol/messages.md`, Framing).
//!
//! ```text
//! Frame {
//!   length: u32   // number of bytes that follow (type + body)
//!   type:   u8    // message type
//!   body:   …     // fields of that message type, in order
//! }
//! ```
//!
//! Framing is shared by peer connections and relay connections, which number their message types
//! separately. So the functions here take the limit for each type code from the caller.

use crate::wire::{DecodeError, Writer};

/// The length of a frame's header: the `u32` length and the `u8` type.
pub const HEADER_LEN: usize = 5;

/// One frame, borrowed from the bytes it was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame<'a> {
    /// The message type code.
    pub message_type: u8,
    /// The message's fields.
    pub body: &'a [u8],
}

/// Reads one frame from the front of `buf`, a stream's received bytes.
///
/// `max_length` gives the largest allowed `length` for a message type code, or fails for an
/// unknown type. The limit is checked as soon as the header has arrived, before the body.
///
/// Returns `Ok(None)` if `buf` doesn't yet hold the whole frame, or the frame and the number of
/// bytes it occupies.
pub fn split_frame(
    buf: &[u8],
    max_length: impl FnOnce(u8) -> Result<u32, DecodeError>,
) -> Result<Option<(Frame<'_>, usize)>, DecodeError> {
    let Some(header) = buf.first_chunk::<HEADER_LEN>() else {
        if buf.len() >= 4 && buf[..4] == [0; 4] {
            return Err(DecodeError::EmptyFrame);
        }
        return Ok(None);
    };
    let length = u32::from_be_bytes(header[..4].try_into().expect("4 bytes"));
    if length == 0 {
        return Err(DecodeError::EmptyFrame);
    }
    let message_type = header[4];
    let max = max_length(message_type)?;
    if length > max {
        return Err(DecodeError::FrameTooLong { length, max });
    }
    let end = 4 + length as usize;
    Ok(buf.get(HEADER_LEN..end).map(|body| {
        let frame = Frame { message_type, body };
        (frame, end)
    }))
}

/// Encodes a frame around a message body.
///
/// # Panics
///
/// If `body` is 4 GiB or longer, which no message allows.
pub fn encode_frame(message_type: u8, body: &[u8]) -> Vec<u8> {
    encode_frame_with(message_type, body.len(), |w| w.raw(body))
}

/// Encodes a frame whose body `write_body` writes in place, so the body is never copied.
/// `body_len` is the expected body length, used only to size the buffer.
///
/// # Panics
///
/// If the body is 4 GiB or longer, which no message allows.
pub fn encode_frame_with(
    message_type: u8,
    body_len: usize,
    write_body: impl FnOnce(&mut Writer),
) -> Vec<u8> {
    let mut w = Writer::with_capacity(HEADER_LEN + body_len);
    // The length is filled in once the body is written.
    w.u32(0);
    w.u8(message_type);
    write_body(&mut w);
    let mut frame = w.into_bytes();
    let length = u32::try_from(frame.len() - 4).expect("frame longer than u32::MAX");
    frame[..4].copy_from_slice(&length.to_be_bytes());
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limit(code: u8) -> Result<u32, DecodeError> {
        match code {
            1 => Ok(10),
            _ => Err(DecodeError::UnknownMessageType(code)),
        }
    }

    #[test]
    fn splits_a_complete_frame() {
        let mut bytes = encode_frame(1, b"abc");
        bytes.push(0xff);
        let (frame, used) = split_frame(&bytes, limit).unwrap().unwrap();
        assert_eq!(frame.message_type, 1);
        assert_eq!(frame.body, b"abc");
        assert_eq!(used, 8);
    }

    #[test]
    fn waits_for_more_bytes() {
        let bytes = encode_frame(1, b"abc");
        for end in 0..bytes.len() {
            assert_eq!(split_frame(&bytes[..end], limit), Ok(None));
        }
    }

    #[test]
    fn checks_the_limit_before_the_body_arrives() {
        let header = [0, 0, 0, 12, 1];
        assert_eq!(
            split_frame(&header, limit),
            Err(DecodeError::FrameTooLong {
                length: 12,
                max: 10
            })
        );
    }

    #[test]
    fn rejects_unknown_types_and_empty_frames() {
        assert_eq!(
            split_frame(&[0, 0, 0, 1, 9], limit),
            Err(DecodeError::UnknownMessageType(9))
        );
        assert_eq!(
            split_frame(&[0, 0, 0, 0], limit),
            Err(DecodeError::EmptyFrame)
        );
    }
}
