//! The simulated network: a message-level model (D-069).
//!
//! Each node (peer) has an access link with upload and download capacity, a one-way latency to
//! the core of the network, and a chance of losing a message. Two nodes are as far apart as
//! their two latencies added, and a message between them is lost if either link loses it.
//!
//! **Bulk transfers** (Data and Manifest responses) are flows that share capacity max-min
//! fairly (D-245): each flow gets as much as it can without taking from a flow that has less.
//! A flow uses its sender's upload and its receiver's download, and a relayed flow also its
//! relay's [`Limit`]. Each of these is a *resource* with a capacity. A saturated resource has a
//! *level*, the rate its flows get unless another resource holds them lower; an unsaturated one
//! holds no flow back. A flow's rate is the lowest level among its resources.
//!
//! When a flow starts or ends, the levels of its resources are worked out again, each by
//! filling its capacity given what its flows' other resources allow. A level that changes makes
//! the resources sharing its flows work theirs out again, and so on until nothing changes, which
//! usually stops within a hop or two. A flow finishes once its bytes have gone at its rates, and
//! arrives one [`Network::delay_via`] later.
//!
//! **Small messages** (everything else) are sent ahead of bulk data, as a transport would
//! prioritize its control streams: they take `size ÷ min(upload, download)` without slowing the
//! flows, then one [`Network::delay_via`].
//!
//! **Relayed** flows share a [`Limit`]: the relay forwards a group of flows, such as one
//! direction of one connection, at up to its rate. A relayed message takes an extra one-way
//! latency for its detour ([`Network::delay_via`]).
//!
//! **Loss** never drops a message, since the transport is reliable: each loss resends it after a
//! probe timeout, one round trip plus 25 ms (QUIC's default maximum ACK delay).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
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

/// A cap shared by a group of flows, such as one direction of a relayed connection.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limit {
    /// Names the group. Flows with the same group and sender share the cap.
    pub group: u64,
    /// The group's total rate in bytes per second.
    pub rate: f64,
}

/// A flow, named by the network. IDs are reused once a flow has ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct FlowId(usize);

/// Added to a round trip to make QUIC's probe timeout after a loss.
const MAX_ACK_DELAY: Duration = Duration::from_millis(25);

/// The most times one message is lost in a row, so that a loss of nearly 1 still terminates.
const MAX_LOSSES: u32 = 64;

/// Levels this close, relative to their size, count as unchanged, so that rounding can't keep
/// a change going around a cycle of resources.
const TOLERANCE: f64 = 1e-12;

/// Evaluations of levels per resource in the network, after which one change falls back to
/// filling every resource from scratch. Changes settle long before; this only bounds the work.
const MAX_EVALUATIONS: usize = 16;

#[derive(Debug)]
struct Node {
    link: Link,
    /// Its upload's resource.
    up: usize,
    /// Its download's resource.
    down: usize,
}

/// Something flows share: an upload, a download or a relay's limit.
#[derive(Debug, Default)]
struct Resource {
    capacity: f64,
    /// The flows using it.
    flows: Vec<usize>,
    /// The most any of its flows gets: infinite while it isn't saturated.
    level: f64,
    /// Whether it waits to have its level worked out again.
    queued: bool,
    /// For a relay limit, its sender and group.
    group: Option<(usize, u64)>,
}

#[derive(Debug)]
struct Flow<P> {
    /// The sender's upload and the receiver's download.
    links: [usize; 2],
    /// The relay limit it shares, if any.
    group: Option<usize>,
    /// Bytes left as of `updated`.
    remaining: f64,
    /// Bytes per second since `updated`.
    rate: f64,
    updated: Time,
    /// When it finishes at its current rate, and the stamp ordering finishes at the same time.
    finish: Option<(Time, u64)>,
    payload: P,
}

impl<P> Flow<P> {
    fn resources(&self) -> impl Iterator<Item = usize> + use<P> {
        self.links.into_iter().chain(self.group)
    }
}

/// The simulated network, carrying flows with payloads of type `P`.
#[derive(Debug)]
pub struct Network<P> {
    nodes: Vec<Node>,
    /// Nodes' uploads and downloads, and relay limits.
    resources: Vec<Resource>,
    free_resources: Vec<usize>,
    /// Relay limits' resources, by sender and group.
    groups: BTreeMap<(usize, u64), usize>,
    flows: Vec<Option<Flow<P>>>,
    free: Vec<usize>,
    next_stamp: u64,
    /// Every running flow's finish, earliest first.
    finishes: BTreeSet<(Time, u64, usize)>,
    /// Resources waiting to have their levels worked out again, in order.
    work: VecDeque<usize>,
    /// Scratch space for what a resource's flows may take.
    caps: Vec<f64>,
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
            resources: Vec::new(),
            free_resources: Vec::new(),
            groups: BTreeMap::new(),
            flows: Vec::new(),
            free: Vec::new(),
            next_stamp: 0,
            finishes: BTreeSet::new(),
            work: VecDeque::new(),
            caps: Vec::new(),
        }
    }

    /// Adds a node and returns its index.
    pub fn add_node(&mut self, link: Link) -> usize {
        let [up, down] = [link.up, link.down].map(|capacity| {
            self.resources.push(Resource {
                capacity: capacity.get(),
                level: f64::INFINITY,
                ..Resource::default()
            });
            self.resources.len() - 1
        });
        self.nodes.push(Node { link, up, down });
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
    #[cfg(test)]
    fn delay(&self, a: usize, b: usize, rng: &mut Rng) -> Duration {
        self.delay_via(a, b, Duration::ZERO, rng)
    }

    /// The time from a message leaving `a` to its arrival at `b` on a path that is `detour`
    /// longer each way, such as through a relay: the latency, plus a probe timeout for each time
    /// it is lost.
    pub fn delay_via(&self, a: usize, b: usize, detour: Duration, rng: &mut Rng) -> Duration {
        let loss = self.loss(a, b);
        let mut losses = 0;
        while loss > 0.0 && losses < MAX_LOSSES && rng.random::<f64>() < loss {
            losses += 1;
        }
        let latency = self.latency(a, b).saturating_add(detour);
        let timeout = latency.saturating_mul(2).saturating_add(MAX_ACK_DELAY);
        latency.saturating_add(timeout.saturating_mul(losses))
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
    #[cfg(test)]
    fn start(&mut self, now: Time, from: usize, to: usize, bytes: u64, payload: P) -> FlowId {
        self.start_limited(now, from, to, bytes, None, payload)
    }

    /// Starts a flow of `bytes` from `from` to `to`, sharing `limit` if it has one. Its finish,
    /// and those of other flows whose rate changed, are then in [`Network::next_finish`].
    pub fn start_limited(
        &mut self,
        now: Time,
        from: usize,
        to: usize,
        bytes: u64,
        limit: Option<Limit>,
        payload: P,
    ) -> FlowId {
        let group = limit.map(|limit| self.group(from, limit));
        let flow = Flow {
            links: [self.nodes[from].up, self.nodes[to].down],
            group,
            remaining: bytes as f64,
            rate: 0.0,
            updated: now,
            finish: None,
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
        let resources: Vec<usize> = self.flow(index).resources().collect();
        for &resource in &resources {
            self.resources[resource].flows.push(index);
        }
        self.reshare(now, resources);
        FlowId(index)
    }

    /// Ends a flow, finished or not, and returns its payload. Flows it held back speed up.
    ///
    /// # Panics
    ///
    /// If the flow has already ended.
    pub fn end(&mut self, now: Time, flow: FlowId) -> P {
        let ended = self.flows[flow.0].take().expect("the flow is running");
        self.free.push(flow.0);
        if let Some((at, stamp)) = ended.finish {
            self.finishes.remove(&(at, stamp, flow.0));
        }
        let resources: Vec<usize> = ended.resources().collect();
        for &resource in &resources {
            self.leave(resource, flow.0);
        }
        self.reshare(now, resources);
        ended.payload
    }

    /// Lifts the limit from `from`'s flows in `group`, as when a relayed connection goes direct.
    pub fn unlimit(&mut self, now: Time, from: usize, group: u64) {
        let Some(resource) = self.groups.remove(&(from, group)) else {
            return;
        };
        let flows = std::mem::take(&mut self.resources[resource].flows);
        self.free_resources.push(resource);
        let mut dirty = Vec::new();
        for &index in &flows {
            let flow = self.flows[index].as_mut().expect("listed flows run");
            flow.group = None;
            dirty.extend(flow.links);
        }
        // Working out the links' levels again updates every flow on them, these included.
        self.reshare(now, dirty);
    }

    /// The flow that finishes first at the current rates, and when.
    pub fn next_finish(&self) -> Option<(Time, FlowId)> {
        self.finishes
            .first()
            .map(|&(at, _, index)| (at, FlowId(index)))
    }

    /// A running flow's current rate in bytes per second.
    #[cfg(test)]
    fn rate(&self, flow: FlowId) -> Option<f64> {
        self.flows.get(flow.0)?.as_ref().map(|f| f.rate)
    }

    fn flow(&self, index: usize) -> &Flow<P> {
        self.flows[index].as_ref().expect("listed flows run")
    }

    /// The resource for `from`'s flows in `limit`'s group.
    fn group(&mut self, from: usize, limit: Limit) -> usize {
        if let Some(&resource) = self.groups.get(&(from, limit.group)) {
            return resource;
        }
        let fresh = Resource {
            capacity: limit.rate,
            level: f64::INFINITY,
            group: Some((from, limit.group)),
            ..Resource::default()
        };
        let resource = match self.free_resources.pop() {
            Some(resource) => {
                self.resources[resource] = fresh;
                resource
            }
            None => {
                self.resources.push(fresh);
                self.resources.len() - 1
            }
        };
        self.groups.insert((from, limit.group), resource);
        resource
    }

    /// Takes flow `index` off `resource`, forgetting the resource if it was a relay limit that
    /// no flow uses any more.
    fn leave(&mut self, resource: usize, index: usize) {
        let found = &mut self.resources[resource];
        let at = found
            .flows
            .iter()
            .position(|&f| f == index)
            .expect("listed");
        found.flows.swap_remove(at);
        if let (true, Some(key)) = (found.flows.is_empty(), found.group) {
            self.groups.remove(&key);
            self.free_resources.push(resource);
        }
    }

    /// Works out the levels of `dirty` resources again, and of every resource their changes
    /// reach, then updates the rates of the flows involved.
    fn reshare(&mut self, now: Time, dirty: Vec<usize>) {
        for resource in dirty {
            self.enqueue(resource);
        }
        let budget = MAX_EVALUATIONS * self.resources.len() + 64;
        let mut evaluations = 0;
        let mut touched = Vec::new();
        while let Some(resource) = self.work.pop_front() {
            self.resources[resource].queued = false;
            evaluations += 1;
            if evaluations > budget {
                self.fill_all();
                let all: Vec<usize> = (0..self.flows.len())
                    .filter(|&f| self.flows[f].is_some())
                    .collect();
                self.update_rates(now, all);
                return;
            }
            touched.extend_from_slice(&self.resources[resource].flows);
            let level = self.level(resource);
            let old = self.resources[resource].level;
            if same(old, level) {
                continue;
            }
            self.resources[resource].level = level;
            for i in 0..self.resources[resource].flows.len() {
                let flow = self.flow(self.resources[resource].flows[i]);
                let ([up, down], group) = (flow.links, flow.group);
                for other in [Some(up), Some(down), group].into_iter().flatten() {
                    if other != resource && affects(old, level, self.resources[other].level) {
                        self.enqueue(other);
                    }
                }
            }
        }
        touched.sort_unstable();
        touched.dedup();
        self.update_rates(now, touched);
    }

    fn enqueue(&mut self, resource: usize) {
        if !self.resources[resource].queued {
            self.resources[resource].queued = true;
            self.work.push_back(resource);
        }
    }

    /// `resource`'s level given its flows' other resources' levels: the rate at which its
    /// capacity fills when each flow takes up to what its other resources allow.
    fn level(&mut self, resource: usize) -> f64 {
        let capacity = self.resources[resource].capacity;
        if capacity.is_infinite() {
            return f64::INFINITY;
        }
        let mut caps = std::mem::take(&mut self.caps);
        caps.clear();
        caps.extend(self.resources[resource].flows.iter().map(|&index| {
            self.flow(index)
                .resources()
                .filter(|&r| r != resource)
                .map(|r| self.resources[r].level)
                .fold(f64::INFINITY, f64::min)
        }));
        let level = fill(capacity, &mut caps);
        self.caps = caps;
        level
    }

    /// Sets every resource's level from scratch.
    fn fill_all(&mut self) {
        let levels = self.filled_levels();
        for (resource, level) in self.resources.iter_mut().zip(levels) {
            resource.level = level;
        }
    }

    /// Every resource's level by progressive filling: raise every flow's rate together, and
    /// freeze the flows of each resource as it saturates. It costs time proportional to the
    /// resources squared, so it is only a fallback.
    fn filled_levels(&self) -> Vec<f64> {
        let mut levels = vec![f64::INFINITY; self.resources.len()];
        let mut left: Vec<f64> = self.resources.iter().map(|r| r.capacity).collect();
        let mut open: Vec<usize> = self.resources.iter().map(|r| r.flows.len()).collect();
        let mut frozen = vec![false; self.flows.len()];
        loop {
            let next = (0..self.resources.len())
                .filter(|&r| open[r] > 0 && left[r].is_finite())
                .map(|r| (left[r].max(0.0) / open[r] as f64, r))
                .min_by(|a, b| a.0.total_cmp(&b.0));
            let Some((share, resource)) = next else {
                return levels;
            };
            levels[resource] = share;
            for &index in &self.resources[resource].flows {
                if std::mem::replace(&mut frozen[index], true) {
                    continue;
                }
                for r in self.flow(index).resources() {
                    left[r] -= share;
                    open[r] -= 1;
                }
            }
        }
    }

    /// Sets each of `flows` to the lowest level among its resources, rescheduling its finish if
    /// its rate changed.
    fn update_rates(&mut self, now: Time, flows: Vec<usize>) {
        for index in flows {
            let rate = self
                .flow(index)
                .resources()
                .map(|r| self.resources[r].level)
                .fold(f64::INFINITY, f64::min);
            let flow = self.flows[index].as_mut().expect("listed flows run");
            let elapsed = now.since(flow.updated).as_secs_f64();
            if elapsed > 0.0 {
                flow.remaining = (flow.remaining - flow.rate * elapsed).max(0.0);
            }
            flow.updated = now;
            if flow.finish.is_some() && flow.rate == rate {
                continue;
            }
            if let Some((at, stamp)) = flow.finish {
                self.finishes.remove(&(at, stamp, index));
            }
            flow.rate = rate;
            self.next_stamp += 1;
            let at = now + seconds(flow.remaining / rate);
            flow.finish = Some((at, self.next_stamp));
            self.finishes.insert((at, self.next_stamp, index));
        }
    }
}

/// The level at which `capacity` fills when each flow takes up to its entry in `caps`:
/// infinite if they all fit. Sorts `caps`.
fn fill(capacity: f64, caps: &mut [f64]) -> f64 {
    caps.sort_unstable_by(f64::total_cmp);
    let mut left = capacity;
    for (i, &cap) in caps.iter().enumerate() {
        let share = left / (caps.len() - i) as f64;
        if cap > share {
            return share.max(0.0);
        }
        left -= cap;
    }
    f64::INFINITY
}

/// Whether a resource's level going from `old` to `new` can change the level of another
/// resource at `level` that shares one of its flows.
///
/// A level is where its resource's capacity fills when each flow takes up to what its other
/// resources allow, so a flow's allowance changes nothing while it stays at or above the level
/// both before and after. An unsaturated resource (an infinite level) holds no flow back, and
/// stays unsaturated when its flows are allowed less.
fn affects(old: f64, new: f64, level: f64) -> bool {
    match level.is_finite() {
        true => old.min(new) < level,
        false => new > old,
    }
}

/// Whether two levels are the same within [`TOLERANCE`].
fn same(a: f64, b: f64) -> bool {
    a == b || (a.is_finite() && b.is_finite() && (a - b).abs() <= TOLERANCE * a.max(b))
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

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0)
    }

    /// Runs the network until every flow has finished, returning each flow's finish time by its
    /// payload.
    fn drain(net: &mut Network<u32>, mut now: Time) -> BTreeMap<u32, Time> {
        let mut done = BTreeMap::new();
        while let Some((next, flow)) = net.next_finish() {
            assert!(next >= now, "time runs forward");
            now = next;
            let payload = net.end(now, flow);
            done.insert(payload, now);
        }
        done
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
    fn capacity_one_flow_cannot_use_goes_to_the_others() {
        let mut net = Network::new();
        let a = net.add_node(link(3e6, 1e9));
        let slow = net.add_node(link(1e9, 5e5));
        let fast = net.add_node(link(1e9, 1e9));
        let x = net.start(Time::ZERO, a, slow, 1_000_000, 0);
        let y = net.start(Time::ZERO, a, fast, 1_000_000, 1);
        // Splitting a's upload evenly would leave 1 MB/s of it unused.
        assert_eq!(net.rate(x), Some(5e5));
        assert_eq!(net.rate(y), Some(2.5e6));
    }

    #[test]
    fn a_change_reaches_flows_beyond_its_ends() {
        // a → b and c → b share b's download; c → d shares c's upload with c → b. When a's
        // flow ends, b's download frees up, c → b speeds up and c → d slows down.
        let mut net = Network::new();
        let a = net.add_node(link(1e9, 1e9));
        let b = net.add_node(link(1e9, 2e6));
        let c = net.add_node(link(3e6, 1e9));
        let d = net.add_node(link(1e9, 1e9));
        let ab = net.start(Time::ZERO, a, b, 1_000_000, 0);
        let cb = net.start(Time::ZERO, c, b, 1_000_000, 1);
        let cd = net.start(Time::ZERO, c, d, 1_000_000, 2);
        assert_eq!(net.rate(ab), Some(1e6));
        assert_eq!(net.rate(cb), Some(1e6));
        assert_eq!(net.rate(cd), Some(2e6));
        net.end(at(0.1), ab);
        assert_eq!(net.rate(cb), Some(1.5e6));
        assert_eq!(net.rate(cd), Some(1.5e6));
    }

    #[test]
    fn a_late_flow_slows_the_first() {
        let mut net = Network::new();
        let a = net.add_node(link(1e6, 1e9));
        let b = net.add_node(link(1e9, 1e9));
        let first = net.start(Time::ZERO, a, b, 1_000_000, 0);
        assert_eq!(net.next_finish(), Some((at(1.0), first)));
        net.start(at(0.5), a, b, 1_000_000, 1);
        assert_eq!(net.finishes.len(), 2, "one finish per flow, none stale");
        let done = drain(&mut net, at(0.5));
        // 0.5 MB left at 0.5 MB/s.
        assert_eq!(done[&0], at(1.5));
        assert_eq!(done[&1], at(2.0));
    }

    #[test]
    fn a_limit_is_shared_by_its_group_until_lifted() {
        let mut net = Network::new();
        let a = net.add_node(link(1e7, 1e9));
        let b = net.add_node(link(1e9, 1e9));
        let c = net.add_node(link(1e9, 1e9));
        let limit = Some(Limit {
            group: 7,
            rate: 1e6,
        });
        let x = net.start_limited(Time::ZERO, a, b, 1_000_000, limit, 0);
        let y = net.start_limited(Time::ZERO, a, b, 1_000_000, limit, 1);
        let z = net.start(Time::ZERO, a, c, 10_000_000, 2);
        assert_eq!(net.rate(x), Some(5e5), "the group splits its cap");
        assert_eq!(net.rate(y), Some(5e5));
        assert_eq!(net.rate(z), Some(9e6), "others take the rest of the link");
        // After 1 s, each limited flow has 0.5 MB left; lifted, they share a's upload.
        net.unlimit(at(1.0), a, 7);
        assert!(close(net.rate(x).unwrap(), 1e7 / 3.0));
        assert!(net.groups.is_empty());
        let done = drain(&mut net, at(1.0));
        assert!(done[&0] < at(1.2), "{:?}", done[&0]);
    }

    #[test]
    fn a_limit_is_forgotten_with_its_last_flow() {
        let mut net = Network::new();
        let a = net.add_node(link(1e7, 1e9));
        let b = net.add_node(link(1e9, 1e9));
        let limit = Some(Limit {
            group: 3,
            rate: 1e6,
        });
        let x = net.start_limited(Time::ZERO, a, b, 1_000, limit, 0);
        net.end(at(0.1), x);
        assert!(net.groups.is_empty());
        // A new group reuses the freed resource.
        let before = net.resources.len();
        net.start_limited(at(0.1), a, b, 1_000, limit, 1);
        assert_eq!(net.resources.len(), before);
    }

    #[test]
    fn a_detour_lengthens_delays_and_probe_timeouts() {
        let mut net: Network<()> = Network::new();
        let mut lossy = link(1e6, 1e6);
        lossy.loss = Fraction::new(0.999).unwrap();
        let a = net.add_node(lossy);
        let b = net.add_node(link(1e6, 1e6));
        let mut rng = tempest_core::Rng::seed_from_u64(3);
        let detour = Duration::from_millis(40);
        let delay = net.delay_via(a, b, detour, &mut rng);
        let latency = net.latency(a, b) + detour;
        let losses = (delay - latency).as_nanos() / (2 * latency + MAX_ACK_DELAY).as_nanos();
        assert!(losses > 0);
        assert_eq!(
            delay,
            latency + losses as u32 * (2 * latency + MAX_ACK_DELAY)
        );
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
        /// Random flows, some relayed, between random nodes: after every change each flow's rate
        /// is the one progressive filling from scratch gives, no node sends or receives faster
        /// than its link, and every flow finishes no sooner than its bytes allow.
        #[test]
        fn sharing_is_max_min_fair(
            links in prop::collection::vec((1u32..100, 1u32..100), 2..7),
            flows in prop::collection::vec(
                (0usize..7, 0usize..7, 1u64..5_000_000, 0u64..3_000, prop::option::of((0u64..3, 1u32..50))),
                1..30,
            ),
            lift in prop::option::of((0usize..7, 0u64..3, 0u64..3_000)),
        ) {
            let mut net = Network::new();
            for &(up, down) in &links {
                net.add_node(link(f64::from(up) * 1e5, f64::from(down) * 1e5));
            }
            let n = links.len();
            let mut flows: Vec<(usize, usize, u64, Time, Option<Limit>)> = flows
                .into_iter()
                .map(|(a, b, bytes, ms, limit)| {
                    let limit = limit.map(|(group, rate)| Limit { group, rate: f64::from(rate) * 1e5 });
                    (a % n, b % n, bytes, at(ms as f64 / 1000.0), limit)
                })
                .collect();
            flows.sort_by_key(|f| f.3);
            let mut lift = lift.map(|(node, group, ms)| (node % n, group, at(ms as f64 / 1000.0)));

            let mut started = 0;
            let mut now = Time::ZERO;
            let mut finished = 0;
            loop {
                let next_done = net.next_finish();
                let next_start = flows.get(started).map(|f| f.3);
                match (next_done, next_start, lift) {
                    (_, _, Some((node, group, when)))
                        if next_start.is_none_or(|s| when <= s)
                            && next_done.is_none_or(|(d, _)| when <= d) =>
                    {
                        now = now.max(when);
                        net.unlimit(now, node, group);
                        lift = None;
                    }
                    (None, None, _) => break,
                    (Some((done, flow)), start, _) if start.is_none_or(|s| done <= s) => {
                        prop_assert!(done >= now);
                        now = done;
                        let index = net.end(now, flow) as usize;
                        let (a, b, bytes, begun, _) = flows[index];
                        let fastest = (links[a].0.min(links[b].1)) as f64 * 1e5;
                        let shortest = seconds(bytes as f64 / fastest);
                        prop_assert!(now.since(begun) + Duration::from_nanos(2) >= shortest);
                        finished += 1;
                    }
                    (_, Some(start), _) => {
                        now = start;
                        let (a, b, bytes, _, limit) = flows[started];
                        net.start_limited(now, a, b, bytes, limit, started as u32);
                        started += 1;
                    }
                    (Some(_), None, _) => unreachable!(),
                }
                let running: Vec<(usize, usize, f64, f64)> = {
                    let levels = net.filled_levels();
                    net.flows
                        .iter()
                        .flatten()
                        .map(|f| {
                            let fair = f.resources().map(|r| levels[r]).fold(f64::INFINITY, f64::min);
                            let from = (0..n).find(|&i| net.nodes[i].up == f.links[0]).unwrap();
                            let to = (0..n).find(|&i| net.nodes[i].down == f.links[1]).unwrap();
                            (from, to, f.rate, fair)
                        })
                        .collect()
                };
                prop_assert_eq!(net.finishes.len(), running.len(), "one finish per flow");
                for &(_, _, rate, fair) in &running {
                    prop_assert!(close(rate, fair), "rate {} but max-min fair {}", rate, fair);
                }
                for (node, &(up, down)) in links.iter().enumerate() {
                    let out: f64 = running.iter().filter(|f| f.0 == node).map(|f| f.2).sum();
                    let inn: f64 = running.iter().filter(|f| f.1 == node).map(|f| f.2).sum();
                    prop_assert!(out <= f64::from(up) * 1e5 * (1.0 + 1e-9));
                    prop_assert!(inn <= f64::from(down) * 1e5 * (1.0 + 1e-9));
                }
            }
            prop_assert_eq!(finished, flows.len());
        }
    }
}
