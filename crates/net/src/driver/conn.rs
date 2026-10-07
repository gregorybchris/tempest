//! Connection tasks: dialing, the control stream, accepting request streams and reporting the
//! round-trip time.

use std::future::Future;
use std::pin::pin;
use std::time::Duration;

use quinn::{ReadError, RecvStream, SendStream, WriteError};
use tempest_core::io::PathKind;
use tempest_core::{ConnId, StreamId};
use tempest_protocol::message::{GoodbyeReason, PeerRecord};
use tempest_protocol::{Address, Message};
use tokio::sync::mpsc;
use tokio::time::MissedTickBehavior;

use super::bundle::Report;
use super::request::Incoming;
use crate::endpoint::{Connection, Endpoint, closed_cause};
use crate::limit::Throttle;
use crate::stream::{ReadMessageError, read_message, write_message};

/// Dials `record`, accepting only its peer ID, and opens the control stream on the first
/// connection that succeeds. Its direct addresses are tried in turn while its relay address is
/// tried alongside (`protocol/connections.md`, Reaching peers behind routers).
pub(super) async fn dial(
    endpoint: Endpoint,
    record: PeerRecord,
    timeout: Duration,
    inbox: mpsc::Sender<Report>,
) {
    let attempt = |address: Address, path: PathKind| {
        let endpoint = &endpoint;
        async move {
            let addr = address.socket_addr();
            let attempt = async {
                let connection = match path {
                    PathKind::Direct => endpoint.dial(addr, Some(record.peer_id)).await,
                    PathKind::Relayed => endpoint.dial_relayed(addr, Some(record.peer_id)).await,
                }
                .ok()?;
                let (send, recv) = connection.quic().open_bi().await.ok()?;
                Some((connection, send, recv))
            };
            tokio::time::timeout(timeout, attempt).await.ok().flatten()
        }
    };
    let direct = async {
        for &address in &record.addresses {
            if let Some(opened) = attempt(address, PathKind::Direct).await {
                return Some(opened);
            }
        }
        None
    };
    let relayed = async {
        match record.relay {
            Some(address) => attempt(address, PathKind::Relayed).await,
            None => None,
        }
    };
    let Some((connection, send, recv)) = first_some(direct, relayed).await else {
        let _ = inbox
            .send(Report::DialFailed {
                peer: record.peer_id,
            })
            .await;
        return;
    };
    let report = Report::Dialed {
        connection: connection.clone(),
        send,
        recv,
    };
    if inbox.send(report).await.is_err() {
        connection.close(GoodbyeReason::ShuttingDown, "");
    }
}

/// The first of `a` and `b` to finish with something, or `None` if neither does. The other is
/// dropped, which closes a connection it was opening.
async fn first_some<T>(
    a: impl Future<Output = Option<T>>,
    b: impl Future<Output = Option<T>>,
) -> Option<T> {
    let (mut a, mut b) = (pin!(a), pin!(b));
    let (mut a_done, mut b_done) = (false, false);
    loop {
        tokio::select! {
            found = &mut a, if !a_done => match found {
                Some(found) => return Some(found),
                None => a_done = true,
            },
            found = &mut b, if !b_done => match found {
                Some(found) => return Some(found),
                None => b_done = true,
            },
            else => return None,
        }
    }
}

/// How a connection's task ended.
enum End {
    /// The connection ended, or the bundle closed it.
    Closed,
    /// The other peer broke the protocol.
    Violation(String),
}

/// One connection's task.
pub(super) struct ConnTask {
    pub conn: ConnId,
    pub connection: Connection,
    pub inbox: mpsc::Sender<Report>,
    pub rtt_interval: Duration,
    /// What Data sent on request streams draws from.
    pub throttle: Throttle,
}

impl ConnTask {
    /// Runs until the connection ends, the other peer breaks the protocol on the control stream,
    /// or the bundle drops `outbox`'s sender, which it does when it closes the connection.
    pub(super) async fn run(
        self,
        send: SendStream,
        recv: RecvStream,
        outbox: mpsc::Receiver<Message>,
    ) {
        // Each part loops until the connection is done, so none is cancelled partway through a
        // frame.
        let end = tokio::select! {
            end = self.read_control(recv) => end,
            end = write_control(send, outbox) => end,
            end = self.accept_requests() => end,
            end = self.report_rtt() => end,
        };
        let report = match end {
            End::Violation(message) => Report::Violation {
                conn: self.conn,
                message,
            },
            End::Closed => {
                let error = self.connection.closed().await;
                // Nothing to report if this side closed it.
                let Some(closed) = closed_cause(&error) else {
                    return;
                };
                Report::Closed {
                    conn: self.conn,
                    closed,
                }
            }
        };
        let _ = self.inbox.send(report).await;
    }

    async fn read_control(&self, mut recv: RecvStream) -> End {
        loop {
            match read_message(&mut recv).await {
                Ok(Some(message)) => {
                    let report = Report::Control {
                        conn: self.conn,
                        message,
                    };
                    if self.inbox.send(report).await.is_err() {
                        return End::Closed;
                    }
                }
                Ok(None) => return End::Violation("the control stream ended".to_owned()),
                Err(ReadMessageError::Read(ReadError::Reset(_))) => {
                    return End::Violation("the control stream was reset".to_owned());
                }
                Err(ReadMessageError::Read(_)) => return End::Closed,
                Err(error) => return End::Violation(format!("on the control stream: {error}")),
            }
        }
    }

    async fn accept_requests(&self) -> End {
        loop {
            let Ok((send, recv)) = self.connection.quic().accept_bi().await else {
                return End::Closed;
            };
            let task = Incoming {
                conn: self.conn,
                stream: StreamId(recv.id().into()),
                throttle: self.throttle.clone(),
                inbox: self.inbox.clone(),
            };
            tokio::spawn(task.run(send, recv));
        }
    }

    async fn report_rtt(&self) -> End {
        let mut interval = tokio::time::interval(self.rtt_interval);
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            let report = Report::Rtt {
                conn: self.conn,
                rtt: self.connection.rtt(),
            };
            if self.inbox.send(report).await.is_err() {
                return End::Closed;
            }
        }
    }
}

/// Writes the session's control messages in order.
async fn write_control(mut send: SendStream, mut outbox: mpsc::Receiver<Message>) -> End {
    while let Some(message) = outbox.recv().await {
        match write_message(&mut send, &message).await {
            Ok(()) => {}
            Err(WriteError::Stopped(_)) => {
                return End::Violation("the control stream was stopped".to_owned());
            }
            Err(_) => break,
        }
    }
    End::Closed
}
