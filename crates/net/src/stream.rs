//! Reading and writing framed messages on QUIC streams (`protocol/messages.md`, Framing).
//!
//! A frame's length is checked against its type's limit as soon as the five-byte header has
//! arrived, so the other side can't make this side buffer more than the limit allows. Peer
//! connections and relay connections share the framing but not the message types.

use quinn::{ReadError, ReadExactError, RecvStream, SendStream, WriteError};
use tempest_protocol::frame::HEADER_LEN;
use tempest_protocol::message::{MessageType, split_message_frame};
use tempest_protocol::relay::split_relay_frame;
use tempest_protocol::{DecodeError, Message, RelayMessage};

/// Why a message could not be read.
#[derive(Debug, thiserror::Error)]
pub enum ReadMessageError {
    /// The stream or its connection failed.
    #[error(transparent)]
    Read(#[from] ReadError),
    /// The stream finished partway through a frame.
    #[error("stream ended inside a frame")]
    Truncated,
    /// The frame was not a valid message: a protocol violation.
    #[error(transparent)]
    Decode(#[from] DecodeError),
}

impl From<ReadExactError> for ReadMessageError {
    fn from(error: ReadExactError) -> Self {
        match error {
            ReadExactError::FinishedEarly(_) => Self::Truncated,
            ReadExactError::ReadError(e) => Self::Read(e),
        }
    }
}

/// Reads the next message, allowing each type its limit from the specification.
///
/// Returns `None` if the stream finished cleanly before another frame began.
pub async fn read_message(recv: &mut RecvStream) -> Result<Option<Message>, ReadMessageError> {
    read_message_within(recv, MessageType::max_frame_length).await
}

/// Reads the next message, allowing each type the frame length `limit` gives it, such as
/// [`max_data_frame_length`](tempest_protocol::message::max_data_frame_length) for the `count`
/// a Request asked for.
///
/// Returns `None` if the stream finished cleanly before another frame began.
pub async fn read_message_within(
    recv: &mut RecvStream,
    limit: impl Fn(MessageType) -> u32,
) -> Result<Option<Message>, ReadMessageError> {
    let check = |header: &[u8; HEADER_LEN]| {
        // Checks the type and the specification's limit.
        split_message_frame(header)?;
        let message_type = MessageType::from_code(header[4]).expect("checked above");
        Ok(limit(message_type).min(message_type.max_frame_length()))
    };
    let Some(frame) = read_frame(recv, check).await? else {
        return Ok(None);
    };
    let (frame, _) = split_message_frame(&frame)?.expect("the whole frame was read");
    Ok(Some(Message::decode(frame)?))
}

/// Writes one message as a frame.
pub async fn write_message(send: &mut SendStream, message: &Message) -> Result<(), WriteError> {
    send.write_all(&message.encode_frame()).await
}

/// Reads the next relay control message (`protocol/relays.md`) from a relay connection's control
/// stream.
///
/// Returns `None` if the stream finished cleanly before another frame began.
pub async fn read_relay_message(
    recv: &mut RecvStream,
) -> Result<Option<RelayMessage>, ReadMessageError> {
    let check = |header: &[u8; HEADER_LEN]| {
        // Checks the type and its limit; a header alone is never a whole frame.
        split_relay_frame(header)?;
        Ok(u32::MAX)
    };
    let Some(frame) = read_frame(recv, check).await? else {
        return Ok(None);
    };
    let (frame, _) = split_relay_frame(&frame)?.expect("the whole frame was read");
    Ok(Some(RelayMessage::decode(frame)?))
}

/// Writes one relay control message as a frame.
pub async fn write_relay_message(
    send: &mut SendStream,
    message: &RelayMessage,
) -> Result<(), WriteError> {
    send.write_all(&message.encode_frame()).await
}

/// Reads one whole frame. `check` sees the header first and returns the largest `length`
/// allowed beyond the type's own limit, or fails.
async fn read_frame(
    recv: &mut RecvStream,
    check: impl FnOnce(&[u8; HEADER_LEN]) -> Result<u32, DecodeError>,
) -> Result<Option<Vec<u8>>, ReadMessageError> {
    let mut header = [0; HEADER_LEN];
    let mut filled = 0;
    while filled < HEADER_LEN {
        match recv.read(&mut header[filled..]).await? {
            Some(n) => filled += n,
            None if filled == 0 => return Ok(None),
            None => return Err(ReadMessageError::Truncated),
        }
    }

    let max = check(&header)?;
    let length = u32::from_be_bytes(header[..4].try_into().expect("4 bytes"));
    if length > max {
        return Err(DecodeError::FrameTooLong { length, max }.into());
    }

    let mut frame = vec![0; 4 + length as usize];
    frame[..HEADER_LEN].copy_from_slice(&header);
    recv.read_exact(&mut frame[HEADER_LEN..]).await?;
    Ok(Some(frame))
}
