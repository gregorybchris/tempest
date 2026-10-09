//! Request stream tasks: one per request this peer sends, and one per request it answers.

use quinn::{ReadError, RecvStream, SendStream, VarInt, WriteError};
use tempest_core::io::OutRequest;
use tempest_core::{ConnId, RequestId, Response, StreamId};
use tempest_protocol::Message;
use tempest_protocol::message::{
    ManifestMessage, MessageType, Reject, RejectReason, max_data_frame_length,
};
use tokio::sync::{mpsc, oneshot};

use super::bundle::Report;
use super::conn::LEAVE_GRACE;
use crate::endpoint::Connection;
use crate::limit::Throttle;
use crate::storage::{Storage, StorageError};
use crate::stream::{ReadMessageError, read_message, read_message_within, write_message};

/// The application error code on a cancelled request stream. The specification gives none.
const CANCELLED: VarInt = VarInt::from_u32(0);

/// How a request stream is answered.
pub(super) enum Answer {
    /// Data for the chunks `[start, start + count)`, read from storage: `bytes` of them.
    Data {
        storage: Storage,
        start: u32,
        count: u32,
        bytes: u64,
    },
    /// The Manifest.
    Manifest(ManifestMessage),
    /// A ready message, such as a Reject.
    Message(Message),
}

/// A request this peer sends.
pub(super) struct Outgoing {
    pub conn: ConnId,
    pub request: RequestId,
    pub message: OutRequest,
    pub connection: Connection,
    /// Where verified chunks go. Present whenever the session requests chunks.
    pub storage: Option<Storage>,
    /// What Data received draws from.
    pub throttle: Throttle,
    pub inbox: mpsc::Sender<Report>,
}

/// Why a request got no response.
enum Failed {
    Write(WriteError),
    Read(ReadMessageError),
}

impl Outgoing {
    /// Opens a stream, sends the request and reads the response. Data is verified and stored
    /// before it is reported. Sending on `cancel`, or dropping its sender, resets the stream.
    pub(super) async fn run(self, mut cancel: oneshot::Receiver<()>) {
        let opened = tokio::select! {
            biased;
            _ = &mut cancel => return,
            opened = self.connection.quic().open_bi() => opened,
        };
        // The connection task reports the connection's end.
        let Ok((mut send, mut recv)) = opened else {
            return;
        };
        let exchanged = tokio::select! {
            biased;
            _ = &mut cancel => None,
            result = exchange(&mut send, &mut recv, self.message) => Some(result),
        };
        let Some(result) = exchanged else {
            let _ = recv.stop(CANCELLED);
            let _ = send.reset(CANCELLED);
            return;
        };

        let report = match result {
            Ok(Some(message)) => Some(match self.response(message).await {
                Ok(response) => Report::Response {
                    conn: self.conn,
                    request: self.request,
                    response,
                },
                Err(error) => Report::StorageFailed(error),
            }),
            Ok(None) => {
                self.unanswered("a request stream ended without a response")
                    .await
            }
            Err(Failed::Write(WriteError::Stopped(_))) => {
                self.unanswered("a request stream was stopped before it was answered")
                    .await
            }
            Err(Failed::Read(ReadMessageError::Read(ReadError::Reset(_)))) => {
                self.unanswered("a request stream was reset before it was answered")
                    .await
            }
            Err(Failed::Read(
                error @ ReadMessageError::Decode(_) | error @ ReadMessageError::Truncated,
            )) => Some(self.violation(format!("on a request stream: {error}"))),
            // The connection is gone; its task reports that.
            Err(_) => return,
        };
        if let Some(report) = report {
            let _ = self.inbox.send(report).await;
        }
    }

    /// A request left unanswered is a violation, unless the other peer is closing the
    /// connection: it gives up its open requests before its Goodbye, which can follow some
    /// time later, once its last control messages are acknowledged (D-224).
    async fn unanswered(&self, message: &str) -> Option<Report> {
        let closing = tokio::time::timeout(2 * LEAVE_GRACE, self.connection.closed()).await;
        closing.is_err().then(|| self.violation(message))
    }

    /// What the session hears for `message`. Data waits for the speed limits, then is
    /// verified, and stored if it passes.
    async fn response(&self, message: Message) -> Result<Response, StorageError> {
        Ok(match (self.message, message) {
            (OutRequest::Chunks { start, count }, Message::Data(data)) => {
                let Some(storage) = &self.storage else {
                    return Err(StorageError::Stopped);
                };
                self.throttle.download(data.chunks.len() as u64).await;
                match storage.write(start, count, data).await {
                    Ok(_) => Response::Data(Ok(())),
                    Err(StorageError::Verify(error)) => Response::Data(Err(error)),
                    Err(error) => return Err(error),
                }
            }
            // A violation whatever the outcome; the session says so.
            (OutRequest::Manifest, Message::Data(_)) => Response::Data(Ok(())),
            (_, Message::Reject(reject)) => Response::Reject(reject),
            (OutRequest::Manifest, Message::Manifest(manifest)) => Response::Manifest(manifest),
            (_, other) => Response::Other(other),
        })
    }

    fn violation(&self, message: impl Into<String>) -> Report {
        Report::Violation {
            conn: self.conn,
            message: message.into(),
        }
    }
}

/// Sends `request`, finishes the sending side, and reads the response, allowing Data only the
/// size the request can need.
async fn exchange(
    send: &mut SendStream,
    recv: &mut RecvStream,
    request: OutRequest,
) -> Result<Option<Message>, Failed> {
    write_message(send, &request.message())
        .await
        .map_err(Failed::Write)?;
    // Fails only if already finished.
    let _ = send.finish();
    let data_limit = match request {
        OutRequest::Chunks { count, .. } => max_data_frame_length(count),
        OutRequest::Manifest => 0,
    };
    let limit = |message_type: MessageType| match message_type {
        MessageType::Data => data_limit,
        other => other.max_frame_length(),
    };
    read_message_within(recv, limit).await.map_err(Failed::Read)
}

/// A request the other peer opened a stream for.
pub(super) struct Incoming {
    pub conn: ConnId,
    pub stream: StreamId,
    /// What Data sent draws from.
    pub throttle: Throttle,
    pub inbox: mpsc::Sender<Report>,
}

impl Incoming {
    /// Reads the request, hands it to the session and sends its answer. Stops as soon as the
    /// other peer resets the stream.
    pub(super) async fn run(self, mut send: SendStream, mut recv: RecvStream) {
        let message = match read_message(&mut recv).await {
            Ok(Some(message)) => message,
            Ok(None) => {
                return self
                    .report(self.violation("a request stream had no request"))
                    .await;
            }
            // Reset before the request arrived, or the connection is gone.
            Err(ReadMessageError::Read(_)) => return,
            Err(error) => {
                let message = format!("on a request stream: {error}");
                return self.report(self.violation(message)).await;
            }
        };

        let (answer, answered) = oneshot::channel();
        let request = Report::Request {
            conn: self.conn,
            stream: self.stream,
            message,
            answer,
        };
        // A request given up on is reset, never finished: a finished stream with no response
        // would read as a malformed answer.
        if self.inbox.send(request).await.is_err() {
            let _ = send.reset(CANCELLED);
            return;
        }
        // Resolves when the requester stops the stream, or the connection ends.
        let stopped = send.stopped();
        tokio::pin!(stopped);
        let answer = tokio::select! {
            biased;
            _ = &mut stopped => return self.abandon(&mut send).await,
            // A dropped sender means the session closed the connection.
            answer = answered => match answer {
                Ok(answer) => answer,
                Err(_) => {
                    let _ = send.reset(CANCELLED);
                    return;
                }
            },
        };

        // Data and the Manifest wait for the speed limits before they are built, so a throttled
        // connection doesn't hold whole responses in memory while it waits.
        let (response, sent) = match answer {
            Answer::Message(message) => (message, false),
            Answer::Manifest(manifest) => {
                let bytes = manifest.manifest.len() as u64;
                tokio::select! {
                    biased;
                    _ = &mut stopped => return self.abandon(&mut send).await,
                    () = self.throttle.upload(bytes) => {}
                }
                (Message::Manifest(manifest), true)
            }
            Answer::Data {
                storage,
                start,
                count,
                bytes,
            } => {
                let read = async {
                    self.throttle.upload(bytes).await;
                    storage.read(start, count).await
                };
                let read = tokio::select! {
                    biased;
                    _ = &mut stopped => return self.abandon(&mut send).await,
                    read = read => read,
                };
                match read {
                    Ok(data) => (Message::Data(data), true),
                    Err(error) => {
                        // Storage can't back what the session promised: refuse, and free the
                        // upload slot without counting the bytes as sent.
                        let reason = match error {
                            StorageError::InvalidRequest(_) => RejectReason::Invalid,
                            _ => RejectReason::DontHave,
                        };
                        let reject = Message::Reject(Reject {
                            reason,
                            message: String::new(),
                        });
                        let _ = write_message(&mut send, &reject).await;
                        let _ = send.finish();
                        return self.report(self.reset()).await;
                    }
                }
            }
        };
        let written = tokio::select! {
            biased;
            _ = &mut stopped => false,
            written = write_message(&mut send, &response) => written.is_ok(),
        };
        if !written {
            return self.abandon(&mut send).await;
        }
        let _ = send.finish();
        if sent {
            let sent = Report::DataSent {
                conn: self.conn,
                stream: self.stream,
            };
            self.report(sent).await;
        }
        drop(recv);
    }

    /// Gives up on a stream the other peer stopped, or whose connection is gone, answering
    /// STOP_SENDING with RESET_STREAM so the stream's slot under the limit is freed.
    async fn abandon(&self, send: &mut SendStream) {
        let _ = send.reset(CANCELLED);
        self.report(self.reset()).await;
    }

    fn reset(&self) -> Report {
        Report::RequestReset {
            conn: self.conn,
            stream: self.stream,
        }
    }

    fn violation(&self, message: impl Into<String>) -> Report {
        Report::Violation {
            conn: self.conn,
            message: message.into(),
        }
    }

    async fn report(&self, report: Report) {
        // A bundle that has stopped no longer cares.
        let _ = self.inbox.send(report).await;
    }
}
