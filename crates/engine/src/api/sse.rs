//! Decoding a `text/event-stream`, as the HTML standard's server-sent events define it, for
//! clients of `GET /v1/events`.

/// The longest line the decoder accepts. A snapshot of every transfer is one line, so this is
/// generous; it only stops a misbehaving server from exhausting memory.
pub const MAX_LINE: usize = 64 * 1024 * 1024;

/// One event from the stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    /// The event's name: its `event` field, or `message` without one.
    pub name: String,
    /// Its `data` fields, joined by newlines.
    pub data: String,
    /// Its own `id` field, if it had one.
    pub id: Option<String>,
}

/// Why the stream can't be decoded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("an event stream line is longer than {MAX_LINE} bytes")]
pub struct LineTooLong;

/// Turns the stream's bytes, in pieces of any size, into events.
#[derive(Debug, Default)]
pub struct SseDecoder {
    /// The current line, until its end arrives.
    line: Vec<u8>,
    /// The last piece ended in `\r`, so a `\n` starting the next one ends no line.
    after_cr: bool,
    name: String,
    data: String,
    has_data: bool,
    id: Option<String>,
    last_event_id: Option<String>,
}

impl SseDecoder {
    /// A decoder at the start of a stream.
    pub fn new() -> Self {
        Self::default()
    }

    /// The stream's last event ID: the latest `id` field, which outlasts the event it came
    /// with. A client that reconnects sends it as `Last-Event-ID`.
    pub fn last_event_id(&self) -> Option<&str> {
        self.last_event_id.as_deref()
    }

    /// Decodes the next piece of the stream, returning the events it completes.
    pub fn feed(&mut self, bytes: &[u8]) -> Result<Vec<SseEvent>, LineTooLong> {
        let mut events = Vec::new();
        for &byte in bytes {
            let after_cr = std::mem::replace(&mut self.after_cr, false);
            match byte {
                b'\n' if after_cr => {}
                b'\r' | b'\n' => {
                    self.after_cr = byte == b'\r';
                    let line = std::mem::take(&mut self.line);
                    events.extend(self.line_ended(&String::from_utf8_lossy(&line)));
                }
                _ if self.line.len() >= MAX_LINE => return Err(LineTooLong),
                _ => self.line.push(byte),
            }
        }
        Ok(events)
    }

    fn line_ended(&mut self, line: &str) -> Option<SseEvent> {
        if line.is_empty() {
            return self.dispatch();
        }
        if line.starts_with(':') {
            return None;
        }
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line, ""),
        };
        match field {
            "event" => value.clone_into(&mut self.name),
            "data" => {
                if self.has_data {
                    self.data.push('\n');
                }
                self.data.push_str(value);
                self.has_data = true;
            }
            // The standard ignores IDs containing NUL.
            "id" if !value.contains('\0') => {
                self.id = Some(value.to_owned());
                self.last_event_id = Some(value.to_owned());
            }
            _ => {}
        }
        None
    }

    fn dispatch(&mut self) -> Option<SseEvent> {
        let name = std::mem::take(&mut self.name);
        let data = std::mem::take(&mut self.data);
        let id = self.id.take();
        if !std::mem::take(&mut self.has_data) {
            return None;
        }
        Some(SseEvent {
            name: if name.is_empty() {
                "message".to_owned()
            } else {
                name
            },
            data,
            id,
        })
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn event(name: &str, data: &str, id: Option<&str>) -> SseEvent {
        SseEvent {
            name: name.to_owned(),
            data: data.to_owned(),
            id: id.map(str::to_owned),
        }
    }

    fn decode(stream: &[u8]) -> Vec<SseEvent> {
        SseDecoder::new().feed(stream).unwrap()
    }

    #[test]
    fn events_have_a_name_data_and_id() {
        let stream = b": keep-alive\n\nid: 7\nevent: transfer\ndata: {\"a\":1}\n\nevent: snapshot\ndata:x\n\n";
        assert_eq!(
            decode(stream),
            [
                event("transfer", "{\"a\":1}", Some("7")),
                event("snapshot", "x", None)
            ]
        );
    }

    #[test]
    fn data_lines_join_and_unnamed_events_are_messages() {
        assert_eq!(
            decode(b"data: one\ndata:  two\ndata\n\n"),
            [event("message", "one\n two\n", None)]
        );
    }

    #[test]
    fn events_without_data_and_unfinished_events_are_not_dispatched() {
        assert_eq!(decode(b"event: a\nid: 3\n\ndata: half"), []);
    }

    #[test]
    fn the_last_event_id_outlasts_its_event() {
        let mut decoder = SseDecoder::new();
        decoder.feed(b"id: 4\ndata: a\n\ndata: b\n\n").unwrap();
        assert_eq!(decoder.last_event_id(), Some("4"));
        decoder.feed(b"id: 5\0\ndata: c\n\n").unwrap();
        assert_eq!(
            decoder.last_event_id(),
            Some("4"),
            "IDs with NUL are ignored"
        );
    }

    #[test]
    fn every_line_ending_counts_once() {
        let expected = [event("message", "a", None), event("message", "b", None)];
        assert_eq!(decode(b"data: a\r\n\r\ndata: b\r\r"), expected);
        // A CRLF split between pieces is still one line ending.
        let mut decoder = SseDecoder::new();
        let mut events = decoder.feed(b"data: a\r").unwrap();
        events.extend(decoder.feed(b"\n\r").unwrap());
        events.extend(decoder.feed(b"\ndata: b\n\n").unwrap());
        assert_eq!(events, expected);
    }

    proptest! {
        /// However the stream is cut into pieces, it decodes to the same events.
        #[test]
        fn pieces_decode_like_the_whole(
            lines in prop::collection::vec("(data|event|id|:)?(: ?[a-z0-9 ]{0,6})?", 0..20),
            endings in prop::collection::vec(prop::sample::select(vec!["\n", "\r", "\r\n"]), 20),
            cuts in prop::collection::vec(any::<prop::sample::Index>(), 0..6),
        ) {
            let stream: String = lines
                .iter()
                .zip(&endings)
                .map(|(line, ending)| format!("{line}{ending}"))
                .collect();
            let bytes = stream.as_bytes();
            let mut cuts: Vec<usize> = cuts.iter().map(|cut| cut.index(bytes.len() + 1)).collect();
            cuts.sort_unstable();
            let mut decoder = SseDecoder::new();
            let mut events = Vec::new();
            let mut start = 0;
            for cut in cuts.into_iter().chain([bytes.len()]) {
                events.extend(decoder.feed(&bytes[start..cut]).unwrap());
                start = cut;
            }
            prop_assert_eq!(events, decode(bytes));
        }
    }
}
