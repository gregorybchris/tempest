//! Keeping the session's connections useful: the connection limit (D-064), peer exchange, and
//! when to stop sharing (D-065, D-066).

use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::time::Duration;

use rand::seq::IteratorRandom;
use tempest_protocol::Message;
use tempest_protocol::message::{GoodbyeReason, MAX_PEER_RECORDS, PeerRecord, Peers};

use super::Session;
use crate::event::{EventKind, StopReason};
use crate::io::ConnId;
use crate::time::Time;

impl Session {
    /// The connection to drop first to make room: an idle one, preferring to keep low-latency
    /// peers. A connection is idle if no Data went either way for `connections.idle_after` and
    /// no request is open in either direction.
    fn idlest(&self, now: Time) -> Option<ConnId> {
        let idle_after = self.config.connections.idle_after;
        self.conns
            .iter()
            .filter(|(_, c)| {
                c.is_open()
                    && c.exchanged_at + idle_after <= now
                    && c.chunk_requests().next().is_none()
                    && c.incoming.is_empty()
            })
            .max_by_key(|&(&id, c)| {
                // Slowest first, with an unknown round-trip time counted as slowest.
                let rtt = c.rtt.get().unwrap_or(Duration::MAX);
                (rtt, Reverse(c.exchanged_at), Reverse(id))
            })
            .map(|(&id, _)| id)
    }

    /// Drops an idle connection to make room for a new one, without reacting to the removal
    /// yet. Returns whether one was dropped.
    pub(super) fn make_room(&mut self, now: Time) -> bool {
        let Some(id) = self.idlest(now) else {
            return false;
        };
        let message = "dropping an idle connection to make room".to_owned();
        self.close_quietly(now, id, GoodbyeReason::TooManyConnections, message)
    }

    /// At the connection limit with peers waiting to be dialed, drops an idle connection every
    /// review interval so that new peers get a chance.
    pub(super) fn review_connections(&mut self, now: Time) {
        if self.complete || now < self.next_conn_review {
            return;
        }
        self.next_conn_review = now + self.config.connections.review_interval;
        if !self.at_connection_limit() {
            return;
        }
        let waiting = self.dial_candidates().any(|(_, k)| k.next_attempt <= now);
        if let Some(id) = self.idlest(now).filter(|_| waiting) {
            let message = "dropping an idle connection to try another peer".to_owned();
            self.close(now, id, GoodbyeReason::TooManyConnections, message);
        }
    }

    /// Sends Peers on each open connection where it is due: up to 50 of the other connected
    /// peers, chosen at random, at most once per `connections.peer_exchange_interval`.
    pub(super) fn share_peers(&mut self, now: Time) {
        let due: Vec<ConnId> = self
            .conns
            .iter()
            .filter(|(_, c)| c.next_peers.is_some_and(|t| t <= now))
            .map(|(&id, _)| id)
            .collect();
        let interval = self.config.connections.peer_exchange_interval;
        for id in due {
            let recipient = self.conns[&id].peer();
            let others: BTreeMap<_, _> = self
                .conns
                .values()
                .filter(|c| c.is_open() && c.peer() != recipient)
                .filter_map(|c| {
                    let hello = c.hello.as_ref()?;
                    let record = PeerRecord {
                        peer_id: c.peer(),
                        addresses: hello.addresses.clone(),
                        relay: hello.relay,
                    };
                    Some((c.peer(), record))
                })
                .collect();
            let peers = others
                .into_values()
                .choose_multiple(&mut self.rng, MAX_PEER_RECORDS);
            self.conns.get_mut(&id).expect("listed").next_peers = Some(now + interval);
            if peers.is_empty() {
                continue;
            }
            let count = peers.len() as u32;
            self.send(id, Message::Peers(Peers { peers }));
            let kind = EventKind::PeersShared {
                conn: id,
                peer: recipient,
                count,
            };
            self.emit(now, kind);
        }
    }

    /// Forgets a random known peer that isn't connected or being dialed, to make room for
    /// another. Returns whether there was one.
    pub(super) fn forget_a_known_peer(&mut self) -> bool {
        let candidates: Vec<_> = self.dial_candidates().map(|(&peer, _)| peer).collect();
        let Some(peer) = candidates.into_iter().choose(&mut self.rng) else {
            return false;
        };
        self.known.remove(&peer);
        true
    }

    /// Tracks since when no connected peer has wanted anything from this peer.
    pub(super) fn update_idle(&mut self, now: Time) {
        let wanted = self.conns.values().any(|c| c.is_open() && c.they_want);
        if wanted {
            self.idle_since = None;
        } else if self.idle_since.is_none() {
            self.idle_since = Some(now);
        }
    }

    /// Whether the public-swarm lingering rule applies: this peer downloaded the bundle and the
    /// user didn't choose to keep sharing (D-065).
    fn lingers(&self) -> bool {
        let linger = &self.config.lingering;
        self.complete && !self.started_complete && !linger.keep_sharing && !linger.direct_send
    }

    /// When lingering ends for lack of interest, if no peer wants anything now.
    pub(super) fn linger_deadline(&self) -> Option<Time> {
        let since = self.idle_since.filter(|_| self.lingers())?;
        Some(since + self.config.lingering.idle_timeout)
    }

    /// Stops sharing once lingering is over, or once a direct send is delivered.
    pub(super) fn check_linger(&mut self, now: Time) {
        if self.stopped || !self.complete || self.config.lingering.keep_sharing {
            return;
        }
        let linger = &self.config.lingering;
        let reason = if linger.direct_send {
            let waiting = self.conns.values().any(|c| c.is_open() && !c.is_complete());
            (self.delivered && !waiting).then_some(StopReason::Delivered)
        } else if !self.lingers() {
            None
        } else if self.uploaded as f64 >= linger.ratio * self.downloaded as f64 {
            Some(StopReason::RatioReached)
        } else if self.linger_deadline().is_some_and(|t| t <= now) {
            Some(StopReason::NobodyWants)
        } else {
            None
        };
        if let Some(reason) = reason {
            self.emit(now, EventKind::StoppedSharing { reason });
            self.stop(now, GoodbyeReason::Done, "finished sharing");
        }
    }
}
