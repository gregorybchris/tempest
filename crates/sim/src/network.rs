//! The simulated network: a message-level model (D-069).
//!
//! Each node (peer) has an access link with upload and download capacity, a one-way latency to
//! the core of the network, and a chance of losing a message. Two nodes are as far apart as
//! their two latencies added, and a message between them is lost if either link loses it.
//!
//! **Bulk transfers** (Data and Manifest responses) are flows that share capacity. A flow's rate
//! is the smaller of its sender's upload divided by the sender's flows and its receiver's
//! download divided by the receiver's flows: processor sharing at both ends. When a flow starts
//! or ends, only the flows at its two ends change rate. A flow finishes once its bytes have gone
//! at those rates, and arrives one [`Network::delay`] later.
//!
//! **Small messages** (everything else) are sent ahead of bulk data, as a transport would
//! prioritize its control streams: they take `size ÷ min(upload, download)` without slowing the
//! flows, then one [`Network::delay`].
//!
//! **Loss** never drops a message, since the transport is reliable: each loss resends it after a
//! probe timeout, one round trip plus 25 ms (QUIC's default maximum ACK delay).

use std::time::Duration;

use rand::Rng as _;
use tempest_core::{Rng, Time};

use crate::units::{Fraction, Rate};

/// A node's access link.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Link {
    /// Upload capacity.
    pub up: Rate,
    /// Download capacity.
    pub down: Rate,
    /// One-way latency to the core of the network.
    pub latency: Duration,
    /// The chance that this link loses a message.
    pub loss: Fraction,
}

/// A flow, named by the network. IDs are reused once a flow has ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct FlowId(usize);

/// When a flow will finish at its current rate. Superseded whenever its rate changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Completion {
    /// The flow.
    pub flow: FlowId,
    /// When it finishes.
    pub at: Time,
    /// Identifies this estimate: it is current while [`Network::is_current`] says so.
    pub stamp: u64,
}

/// Added to a round trip to make QUIC's probe timeout after a loss.
const MAX_ACK_DELAY: Duration = Duration::from_millis(25);

/// The most times one message is lost in a row, so that a loss of nearly 1 still terminates.
const MAX_LOSSES: u32 = 64;

#[derive(Debug)]
struct Node {
    link: Link,
    outgoing: Vec<usize>,
    incoming: Vec<usize>,
}

#[derive(Debug)]
struct Flow<P> {
    from: usize,
    to: usize,
    /// Bytes left as of `updated`.
    remaining: f64,
    /// Bytes per second since `updated`.
    rate: f64,
    updated: Time,
    stamp: u64,
    payload: P,
}

/// The simulated network, carrying flows with payloads of type `P`.
#[derive(Debug)]
pub struct Network<P> {
    nodes: Vec<Node>,
    flows: Vec<Option<Flow<P>>>,
    free: Vec<usize>,
    next_stamp: u64,
    completions: Vec<Completion>,
}

impl<P> Default for Network<P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<P> Network<P> {
    /// An empty network.
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            flows: Vec::new(),
            free: Vec::new(),
            next_stamp: 0,
            completions: Vec::new(),
        }
    }

    /// Adds a node and returns its index.
    pub fn add_node(&mut self, link: Link) -> usize {
        self.nodes.push(Node {
            link,
            outgoing: Vec::new(),
            incoming: Vec::new(),
        });
        self.nodes.len() - 1
    }

    /// The one-way latency from `a` to `b`.
    pub fn latency(&self, a: usize, b: usize) -> Duration {
        self.nodes[a].link.latency + self.nodes[b].link.latency
    }

    /// The round-trip time between `a` and `b`.
    pub fn rtt(&self, a: usize, b: usize) -> Duration {
        2 * self.latency(a, b)
    }

    /// The chance that a message from `a` to `b` is lost.
    pub fn loss(&self, a: usize, b: usize) -> f64 {
        let (la, lb) = (self.nodes[a].link.loss.get(), self.nodes[b].link.loss.get());
        1.0 - (1.0 - la) * (1.0 - lb)
    }

    /// The time from a message leaving `a` to its arrival at `b`: the latency, plus a probe
    /// timeout for each time it is lost.
    pub fn delay(&self, a: usize, b: usize, rng: &mut Rng) -> Duration {
        let loss = self.loss(a, b);
        let mut losses = 0;
        while loss > 0.0 && losses < MAX_LOSSES && rng.random::<f64>() < loss {
            losses += 1;
        }
        self.latency(a, b) + losses * (self.rtt(a, b) + MAX_ACK_DELAY)
    }

    /// The time a small message of `bytes` takes to leave `a` for `b`.
    pub fn send_time(&self, a: usize, b: usize, bytes: usize) -> Duration {
        let rate = self.nodes[a]
            .link
            .up
            .get()
            .min(self.nodes[b].link.down.get());
        seconds(bytes as f64 / rate)
    }

    /// Starts a flow of `bytes` from `from` to `to`.
    ///
    /// Its completion, and those of other flows whose rate changed, are then in
    /// [`Network::take_completions`].
    pub fn start(&mut self, now: Time, from: usize, to: usize, bytes: u64, payload: P) -> FlowId {
        let flow = Flow {
            from,
            to,
            remaining: bytes as f64,
            rate: 0.0,
            updated: now,
            stamp: 0,
            payload,
        };
        let index = match self.free.pop() {
            Some(index) => {
                self.flows[index] = Some(flow);
                index
            }
            None => {
                self.flows.push(Some(flow));
                self.flows.len() - 1
            }
        };
        self.nodes[from].outgoing.push(index);
        self.nodes[to].incoming.push(index);
        self.reshare(now, from, to);
        FlowId(index)
    }

    /// Ends a flow, finished or not, and returns its payload. Flows sharing its ends speed up.
    ///
    /// # Panics
    ///
    /// If the flow has already ended.
    pub fn end(&mut self, now: Time, flow: FlowId) -> P {
        let ended = self.flows[flow.0].take().expect("the flow is running");
        self.free.push(flow.0);
        let remove = |list: &mut Vec<usize>| {
            let at = list.iter().position(|&f| f == flow.0).expect("listed");
            list.swap_remove(at);
        };
        remove(&mut self.nodes[ended.from].outgoing);
        remove(&mut self.nodes[ended.to].incoming);
        self.reshare(now, ended.from, ended.to);
        ended.payload
    }

    /// Whether `stamp` is a flow's current completion estimate.
    pub fn is_current(&self, flow: FlowId, stamp: u64) -> bool {
        self.flows
            .get(flow.0)
            .and_then(Option::as_ref)
            .is_some_and(|f| f.stamp == stamp)
    }

    /// The payload of a running flow.
    pub fn payload(&self, flow: FlowId) -> Option<&P> {
        self.flows.get(flow.0)?.as_ref().map(|f| &f.payload)
    }

    /// A running flow's current rate in bytes per second.
    pub fn rate(&self, flow: FlowId) -> Option<f64> {
        self.flows.get(flow.0)?.as_ref().map(|f| f.rate)
    }

    /// Takes the completions to schedule, in the order they were estimated.
    pub fn take_completions(&mut self) -> Vec<Completion> {
        std::mem::take(&mut self.completions)
    }

    /// Recomputes the rate of every flow leaving `from` or entering `to`.
    fn reshare(&mut self, now: Time, from: usize, to: usize) {
        let mut affected: Vec<usize> = self.nodes[from]
            .outgoing
            .iter()
            .chain(&self.nodes[to].incoming)
            .copied()
            .collect();
        affected.sort_unstable();
        affected.dedup();
        for index in affected {
            let flow = self.flows[index].as_ref().expect("listed flows run");
            let (sender, receiver) = (&self.nodes[flow.from], &self.nodes[flow.to]);
            let rate = (sender.link.up.get() / sender.outgoing.len() as f64)
                .min(receiver.link.down.get() / receiver.incoming.len() as f64);
            let flow = self.flows[index].as_mut().expect("listed flows run");
            let elapsed = now.since(flow.updated).as_secs_f64();
            if elapsed > 0.0 {
                flow.remaining = (flow.remaining - flow.rate * elapsed).max(0.0);
            }
            flow.updated = now;
            if flow.stamp != 0 && flow.rate == rate {
                continue;
            }
            flow.rate = rate;
            self.next_stamp += 1;
            flow.stamp = self.next_stamp;
            self.completions.push(Completion {
                flow: FlowId(index),
                at: now + seconds(flow.remaining / rate),
                stamp: flow.stamp,
            });
        }
    }
}

/// `secs` seconds, rounded up to the nanosecond so that a flow never finishes early.
fn seconds(secs: f64) -> Duration {
    if secs <= 0.0 {
        return Duration::ZERO;
    }
    let nanos = (secs * 1e9).ceil();
    Duration::from_nanos(if nanos < u64::MAX as f64 {
        nanos as u64
    } else {
        u64::MAX
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use proptest::prelude::*;
    use rand::SeedableRng;

    use super::*;

    fn link(up: f64, down: f64) -> Link {
        Link {
            up: Rate::bytes_per_second(up),
            down: Rate::bytes_per_second(down),
            latency: Duration::from_millis(10),
            loss: Fraction::default(),
        }
    }

    fn at(secs: f64) -> Time {
        Time::ZERO + Duration::from_secs_f64(secs)
    }

    /// Runs the network until every flow has finished, returning each flow's finish time by its
    /// payload.
    fn drain(net: &mut Network<u32>, mut now: Time) -> BTreeMap<u32, Time> {
        let mut pending: Vec<Completion> = Vec::new();
        let mut done = BTreeMap::new();
        loop {
            pending.extend(net.take_completions());
            pending.retain(|c| net.is_current(c.flow, c.stamp));
            let Some(next) = pending.iter().min_by_key(|c| (c.at, c.stamp)).copied() else {
                return done;
            };
            assert!(next.at >= now, "time runs forward");
            now = next.at;
            let payload = net.end(now, next.flow);
            done.insert(payload, now);
        }
    }

    #[test]
    fn a_lone_flow_runs_at_the_narrower_end() {
        let mut net = Network::new();
        let a = net.add_node(link(1e6, 1e9));
        let b = net.add_node(link(1e9, 4e5));
        net.start(Time::ZERO, a, b, 1_000_000, 0);
        assert_eq!(drain(&mut net, Time::ZERO)[&0], at(2.5));
    }

    #[test]
    fn flows_share_an_upload_and_speed_up_when_one_ends() {
        let mut net = Network::new();
        let a = net.add_node(link(1e6, 1e9));
        let b = net.add_node(link(1e9, 1e9));
        let c = net.add_node(link(1e9, 1e9));
        net.start(Time::ZERO, a, b, 1_000_000, 0);
        net.start(Time::ZERO, a, c, 500_000, 1);
        let done = drain(&mut net, Time::ZERO);
        // Both run at 0.5 MB/s until the small one ends at 1 s; the big one then has 0.5 MB
        // left at 1 MB/s.
        assert_eq!(done[&1], at(1.0));
        assert_eq!(done[&0], at(1.5));
    }

    #[test]
    fn flows_share_a_download() {
        let mut net = Network::new();
        let a = net.add_node(link(1e9, 1e9));
        let b = net.add_node(link(1e9, 1e9));
        let c = net.add_node(link(1e9, 1e6));
        let x = net.start(Time::ZERO, a, c, 1_000_000, 0);
        let y = net.start(Time::ZERO, b, c, 1_000_000, 1);
        assert_eq!(net.rate(x), Some(5e5));
        assert_eq!(net.rate(y), Some(5e5));
        let done = drain(&mut net, Time::ZERO);
        assert_eq!(done[&0], at(2.0));
        assert_eq!(done[&1], at(2.0));
    }

    #[test]
    fn a_late_flow_slows_the_first() {
        let mut net = Network::new();
        let a = net.add_node(link(1e6, 1e9));
        let b = net.add_node(link(1e9, 1e9));
        let first = net.start(Time::ZERO, a, b, 1_000_000, 0);
        let estimate = net.take_completions();
        assert_eq!(estimate[0].at, at(1.0));
        net.start(at(0.5), a, b, 1_000_000, 1);
        assert!(!net.is_current(first, estimate[0].stamp));
        let done = drain(&mut net, at(0.5));
        // 0.5 MB left at 0.5 MB/s.
        assert_eq!(done[&0], at(1.5));
        assert_eq!(done[&1], at(2.0));
    }

    #[test]
    fn unlimited_links_finish_at_once() {
        let mut net = Network::new();
        let free = Link {
            up: Rate::UNLIMITED,
            down: Rate::UNLIMITED,
            latency: Duration::ZERO,
            loss: Fraction::default(),
        };
        let a = net.add_node(free);
        let b = net.add_node(free);
        net.start(at(3.0), a, b, 1 << 30, 0);
        net.start(at(3.0), a, b, 1 << 30, 1);
        let done = drain(&mut net, at(3.0));
        assert_eq!(done[&0], at(3.0));
        assert_eq!(done[&1], at(3.0));
        assert_eq!(net.send_time(a, b, 1000), Duration::ZERO);
    }

    #[test]
    fn latency_adds_up_and_small_messages_take_their_size() {
        let mut net: Network<()> = Network::new();
        let a = net.add_node(link(1e6, 1e6));
        let mut far = link(1e9, 2e6);
        far.latency = Duration::from_millis(70);
        let b = net.add_node(far);
        assert_eq!(net.latency(a, b), Duration::from_millis(80));
        assert_eq!(net.rtt(b, a), Duration::from_millis(160));
        let mut rng = tempest_core::Rng::seed_from_u64(1);
        assert_eq!(net.delay(a, b, &mut rng), Duration::from_millis(80));
        assert_eq!(net.send_time(a, b, 1000), Duration::from_millis(1));
        assert_eq!(net.send_time(b, a, 1000), Duration::from_millis(1));
    }

    #[test]
    fn loss_delays_by_probe_timeouts() {
        let mut net: Network<()> = Network::new();
        let mut lossy = link(1e6, 1e6);
        lossy.loss = Fraction::new(0.5).unwrap();
        let a = net.add_node(lossy);
        let b = net.add_node(link(1e6, 1e6));
        assert_eq!(net.loss(a, b), 0.5);
        let mut rng = tempest_core::Rng::seed_from_u64(7);
        let base = net.latency(a, b);
        let timeout = net.rtt(a, b) + MAX_ACK_DELAY;
        let samples = 20_000;
        let mut resends = 0u32;
        for _ in 0..samples {
            let extra = net.delay(a, b, &mut rng) - base;
            let n = (extra.as_nanos() / timeout.as_nanos()) as u32;
            assert_eq!(extra, n * timeout, "delays are whole probe timeouts");
            resends += n;
        }
        // Geometric with p = 0.5: one resend per message on average.
        let mean = f64::from(resends) / f64::from(samples);
        assert!((mean - 1.0).abs() < 0.05, "mean resends {mean}");
    }

    proptest! {
        /// Random flows between random nodes: no node ever sends or receives faster than its
        /// link, flows at a common bottleneck get equal rates, and every flow finishes no sooner
        /// than its bytes allow.
        #[test]
        fn sharing_respects_capacity(
            links in prop::collection::vec((1u32..100, 1u32..100), 2..6),
            flows in prop::collection::vec((0usize..6, 0usize..6, 1u64..5_000_000, 0u64..3_000), 1..25),
        ) {
            let mut net = Network::new();
            for &(up, down) in &links {
                net.add_node(link(f64::from(up) * 1e5, f64::from(down) * 1e5));
            }
            let n = links.len();
            let mut flows: Vec<(usize, usize, u64, Time)> = flows
                .into_iter()
                .map(|(a, b, bytes, ms)| (a % n, b % n, bytes, at(ms as f64 / 1000.0)))
                .collect();
            flows.sort_by_key(|f| f.3);

            let mut pending: Vec<Completion> = Vec::new();
            let mut started = 0;
            let mut now = Time::ZERO;
            let mut finished = 0;
            loop {
                pending.extend(net.take_completions());
                pending.retain(|c| net.is_current(c.flow, c.stamp));
                let next_done = pending.iter().min_by_key(|c| (c.at, c.stamp)).copied();
                let next_start = flows.get(started).map(|f| f.3);
                match (next_done, next_start) {
                    (None, None) => break,
                    (Some(done), start) if start.is_none_or(|s| done.at <= s) => {
                        prop_assert!(done.at >= now);
                        now = done.at;
                        let index = net.end(now, done.flow) as usize;
                        let (a, b, bytes, begun) = flows[index];
                        let fastest = (links[a].0.min(links[b].1)) as f64 * 1e5;
                        let shortest = seconds(bytes as f64 / fastest);
                        prop_assert!(now.since(begun) + Duration::from_nanos(2) >= shortest);
                        finished += 1;
                    }
                    (_, Some(start)) => {
                        now = start;
                        let (a, b, bytes, _) = flows[started];
                        net.start(now, a, b, bytes, started as u32);
                        started += 1;
                    }
                    (Some(_), None) => unreachable!(),
                }
                // Capacity and equal shares, over the flows running now.
                let running: Vec<(usize, usize, f64)> = net
                    .flows
                    .iter()
                    .flatten()
                    .map(|f| (f.from, f.to, f.rate))
                    .collect();
                for (node, &(up, down)) in links.iter().enumerate() {
                    let out: f64 = running.iter().filter(|f| f.0 == node).map(|f| f.2).sum();
                    let inn: f64 = running.iter().filter(|f| f.1 == node).map(|f| f.2).sum();
                    prop_assert!(out <= f64::from(up) * 1e5 * (1.0 + 1e-9));
                    prop_assert!(inn <= f64::from(down) * 1e5 * (1.0 + 1e-9));
                }
                for f in &running {
                    for g in &running {
                        if f.0 == g.0 && f.1 == g.1 {
                            prop_assert_eq!(f.2, g.2, "flows on the same path share equally");
                        }
                    }
                }
            }
            prop_assert_eq!(finished, flows.len());
        }
    }
}
