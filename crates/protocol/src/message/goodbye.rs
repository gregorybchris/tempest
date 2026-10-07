//! Goodbye reason codes (`protocol/messages.md`, Closing a connection).
//!
//! A Goodbye is not a stream message: it is QUIC's application CONNECTION_CLOSE frame, whose
//! error code is a [`GoodbyeReason`] and whose reason phrase is a UTF-8 message of at most
//! [`MAX_REASON_MESSAGE_LEN`](super::MAX_REASON_MESSAGE_LEN) bytes.

/// Why a peer closed a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum GoodbyeReason {
    /// Normal close, e.g. the sender no longer needs or serves this bundle.
    Done = 0,
    /// The sender is shutting down.
    ShuttingDown = 1,
    /// The sender isn't participating in the bundle named in Hello.
    UnknownBundle = 2,
    /// Another connection between these peers for this bundle is being kept.
    Duplicate = 3,
    /// The other peer broke a rule in the specification.
    ProtocolViolation = 4,
    /// The other peer sent data that failed verification.
    BadData = 5,
    /// The sender has reached its connection limit.
    TooManyConnections = 6,
}

impl GoodbyeReason {
    /// The reason with QUIC error code `code`. An unknown code means [`GoodbyeReason::Done`].
    pub fn from_code(code: u64) -> Self {
        match code {
            1 => Self::ShuttingDown,
            2 => Self::UnknownBundle,
            3 => Self::Duplicate,
            4 => Self::ProtocolViolation,
            5 => Self::BadData,
            6 => Self::TooManyConnections,
            _ => Self::Done,
        }
    }

    /// The reason's QUIC error code.
    pub fn code(self) -> u64 {
        self as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_round_trip_and_unknown_codes_mean_done() {
        for code in 0..=6 {
            assert_eq!(GoodbyeReason::from_code(code).code(), code);
        }
        assert_eq!(GoodbyeReason::from_code(7), GoodbyeReason::Done);
        assert_eq!(GoodbyeReason::from_code(u64::MAX), GoodbyeReason::Done);
    }
}
