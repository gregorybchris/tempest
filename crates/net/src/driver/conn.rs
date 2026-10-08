//! Connection tasks: dialing, the control stream, accepting request streams and reporting the
//! round-trip time.

use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use quinn::{ReadError, RecvStream, SendStream, WriteError};
use tempest_core::io::PathKind;
use tempest_core::{ConnId, StreamId};
use tempest_protocol::message::{GoodbyeReason, PeerRecord};
use tempest_protocol::{Address, Message, PeerId};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Instant, MissedTickBehavior};

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
    let peer = record.peer_id;
    let direct = async {
        for &address in &record.addresses {
            if let Some(opened) = open(&endpoint, address, PathKind::Direct, peer, timeout).await {
                return Some(opened);
            }
        }
        None
    };
    let relayed = async {
        match record.relay {
            Some(address) => open(&endpoint, address, PathKind::Relayed, peer, timeout).await,
            None => None,
        }
    };
    match first_some(direct, relayed).await {
        Some(opened) => report_dialed(opened, &inbox).await,
        None => {
            let _ = inbox.send(Report::DialFailed { peer }).await;
        }
    }
}

/// Dials `peer` at every one of `addresses` at once, to punch through both peers' routers
/// (`protocol/relays.md`, Upgrading to a direct connection). An attempt that fails is started
/// again until `time` runs out or `stop` is set, and every connection that opens is reported
/// like any dial. Attempts already under way when `stop` is set run their course, since the
/// other peer may have completed them.
pub(super) fn punch(
    endpoint: &Endpoint,
    peer: PeerId,
    addresses: Vec<Address>,
    time: Duration,
    inbox: &mpsc::Sender<Report>,
    stop: Arc<AtomicBool>,
) {
    let deadline = Instant::now() + time;
    for address in addresses {
        let (endpoint, inbox, stop) = (endpoint.clone(), inbox.clone(), stop.clone());
        tokio::spawn(async move {
            while !stop.load(Ordering::Relaxed) {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return;
                }
                if let Some(opened) = open(&endpoint, address, PathKind::Direct, peer, left).await {
                    report_dialed(opened, &inbox).await;
                    return;
                }
                // Some attempts fail at once, as for an address of a family without a socket.
                tokio::time::sleep(PUNCH_RETRY.min(left)).await;
            }
        });
    }
}

/// The pause before an attempt to punch through to an address that failed is started again.
const PUNCH_RETRY: Duration = Duration::from_millis(250);

/// A connection and its control stream, opened by dialing.
type Opened = (Connection, SendStream, RecvStream);

/// Dials `peer` at `address` and opens the control stream, all within `timeout`.
async fn open(
    endpoint: &Endpoint,
    address: Address,
    path: PathKind,
    peer: PeerId,
    timeout: Duration,
) -> Option<Opened> {
    let addr = address.socket_addr();
    let attempt = async {
        let connection = match path {
            PathKind::Direct => endpoint.dial(addr, Some(peer)).await,
            PathKind::Relayed => endpoint.dial_relayed(addr, Some(peer)).await,
        }
        .ok()?;
        let (send, recv) = connection.quic().open_bi().await.ok()?;
        Some((connection, send, recv))
    };
    tokio::time::timeout(timeout, attempt).await.ok().flatten()
}

/// Hands a dialed connection to the bundle, or closes it if the bundle has stopped.
async fn report_dialed((connection, send, recv): Opened, inbox: &mpsc::Sender<Report>) {
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

/// How long a closing end waits for the other to acknowledge the end of the control stream,
/// and a receiver waits for the Goodbye after it (D-224).
pub(super) const LEAVE_GRACE: Duration = Duration::from_secs(2);

/// How a connection's task ended.
enum End {
    /// The connection ended, or the bundle closed it.
    Closed,
    /// The other peer finished its control stream, which it does before a Goodbye.
    Finished,
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
    /// Told once the control stream is finished and acknowledged, or the wait is over.
    pub drained: Option<oneshot::Sender<()>>,
}

impl ConnTask {
    /// Runs until the connection ends, the other peer breaks the protocol on the control stream,
    /// or the bundle drops `outbox`'s sender, which it does when it closes the connection.
    pub(super) async fn run(
        mut self,
        send: SendStream,
        recv: RecvStream,
        outbox: mpsc::Receiver<Message>,
    ) {
        let drained = self.drained.take();
        // Each part loops until the connection is done, so none is cancelled partway through a
        // frame.
        let end = tokio::select! {
            end = self.read_control(recv) => end,
            end = write_control(send, outbox, drained) => end,
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
            // A peer that is leaving finishes the stream and closes once it is acknowledged.
            End::Finished => {
                match tokio::time::timeout(LEAVE_GRACE, self.connection.closed()).await {
                    Ok(error) => {
                        let Some(closed) = closed_cause(&error) else {
                            return;
                        };
                        Report::Closed {
                            conn: self.conn,
                            closed,
                        }
                    }
                    Err(_) => Report::Violation {
                        conn: self.conn,
                        message: "the control stream ended".to_owned(),
                    },
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
                Ok(None) => return End::Finished,
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

/// Writes the session's control messages in order. Once the bundle drops its sender, which it
/// does when it closes the connection, writes what is queued, finishes the stream and waits,
/// for a while, for the other peer to acknowledge it, since closing the connection at once
/// would drop what is still unsent (D-224). Then it tells `drained`.
async fn write_control(
    mut send: SendStream,
    mut outbox: mpsc::Receiver<Message>,
    drained: Option<oneshot::Sender<()>>,
) -> End {
    while let Some(message) = outbox.recv().await {
        match write_message(&mut send, &message).await {
            Ok(()) => {}
            Err(WriteError::Stopped(_)) => {
                return End::Violation("the control stream was stopped".to_owned());
            }
            Err(_) => break,
        }
    }
    if send.finish().is_ok() {
        let _ = tokio::time::timeout(LEAVE_GRACE, send.stopped()).await;
    }
    if let Some(drained) = drained {
        let _ = drained.send(());
    }
    End::Closed
}
