//! The queue: which transfers run, given the limits on active downloads and uploads.

use std::collections::BTreeSet;

use serde::Deserialize;
use tempest_protocol::BundleId;

use crate::config::QueueSettings;

/// A transfer that wants to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub id: BundleId,
    /// Its place in the queue: lower runs first.
    pub position: u32,
    /// It has every chunk, so it takes an upload slot rather than a download slot.
    pub complete: bool,
    /// It runs regardless of the limits, and takes no slot.
    pub forced: bool,
}

/// The transfers that should run: every forced candidate, then the first in queue order up to
/// each limit.
pub(crate) fn schedule(candidates: &[Candidate], limits: QueueSettings) -> BTreeSet<BundleId> {
    let mut ordered = candidates.to_vec();
    ordered.sort_by_key(|c| c.position);
    let mut downloads = 0;
    let mut uploads = 0;
    let mut running = BTreeSet::new();
    for candidate in ordered {
        let (count, limit) = match candidate.complete {
            false => (&mut downloads, limits.max_active_downloads),
            true => (&mut uploads, limits.max_active_uploads),
        };
        if candidate.forced {
            running.insert(candidate.id);
        } else if limit.is_none_or(|limit| *count < limit) {
            *count += 1;
            running.insert(candidate.id);
        }
    }
    running
}

/// Where to move a transfer in the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueMove {
    /// First in the queue.
    Top,
    /// One place earlier.
    Up,
    /// One place later.
    Down,
    /// Last in the queue.
    Bottom,
    /// To this place, counting from 0, or last if past the end.
    To(u32),
}

/// Moves `id` within `order`, a queue listed first to last. Returns false if `id` isn't in it.
pub(crate) fn reorder(order: &mut Vec<BundleId>, id: BundleId, to: QueueMove) -> bool {
    let Some(from) = order.iter().position(|&other| other == id) else {
        return false;
    };
    let last = order.len() - 1;
    let target = match to {
        QueueMove::Top => 0,
        QueueMove::Up => from.saturating_sub(1),
        QueueMove::Down => (from + 1).min(last),
        QueueMove::Bottom => last,
        QueueMove::To(place) => (place as usize).min(last),
    };
    let moved = order.remove(from);
    order.insert(target, moved);
    true
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use tempest_protocol::Hash;

    use super::*;

    fn id(n: u8) -> BundleId {
        Hash::new([n; 32])
    }

    fn candidate(n: u8, complete: bool, forced: bool) -> Candidate {
        Candidate {
            id: id(n),
            position: n.into(),
            complete,
            forced,
        }
    }

    fn limits(downloads: Option<u32>, uploads: Option<u32>) -> QueueSettings {
        QueueSettings {
            max_active_downloads: downloads,
            max_active_uploads: uploads,
        }
    }

    #[test]
    fn the_first_in_queue_order_fill_each_limit() {
        let candidates = [
            candidate(3, false, false),
            candidate(1, false, false),
            candidate(2, true, false),
            candidate(4, false, false),
            candidate(5, true, false),
        ];
        let running = schedule(&candidates, limits(Some(2), Some(1)));
        assert_eq!(running, BTreeSet::from([id(1), id(3), id(2)]));
        let running = schedule(&candidates, limits(None, None));
        assert_eq!(running.len(), 5);
    }

    #[test]
    fn forced_transfers_run_and_take_no_slot() {
        let candidates = [
            candidate(1, false, true),
            candidate(2, false, false),
            candidate(3, false, true),
            candidate(4, false, false),
        ];
        let running = schedule(&candidates, limits(Some(1), Some(1)));
        assert_eq!(running, BTreeSet::from([id(1), id(2), id(3)]));
    }

    #[test]
    fn reordering_moves_one_transfer() {
        let mut order: Vec<_> = (0..5).map(id).collect();
        assert!(reorder(&mut order, id(3), QueueMove::Top));
        assert_eq!(order, [id(3), id(0), id(1), id(2), id(4)]);
        assert!(reorder(&mut order, id(3), QueueMove::Up));
        assert_eq!(order[0], id(3), "the first stays first");
        assert!(reorder(&mut order, id(0), QueueMove::Down));
        assert_eq!(order, [id(3), id(1), id(0), id(2), id(4)]);
        assert!(reorder(&mut order, id(3), QueueMove::Bottom));
        assert!(reorder(&mut order, id(4), QueueMove::To(99)));
        assert_eq!(order, [id(1), id(0), id(2), id(3), id(4)]);
        assert!(!reorder(&mut order, id(9), QueueMove::Top));
    }

    proptest! {
        /// Whatever the queue, the limits hold for unforced transfers, every forced one runs,
        /// and a waiting transfer is never ahead of a running one of the same kind.
        #[test]
        fn limits_hold(
            specs in prop::collection::vec((any::<bool>(), any::<bool>()), 0..30),
            downloads in prop::option::of(1u32..5),
            uploads in prop::option::of(1u32..5),
        ) {
            let candidates: Vec<_> = specs
                .iter()
                .enumerate()
                .map(|(n, &(complete, forced))| candidate(n as u8, complete, forced))
                .collect();
            let running = schedule(&candidates, limits(downloads, uploads));
            for complete in [false, true] {
                let limit = if complete { uploads } else { downloads };
                let unforced: Vec<_> = candidates
                    .iter()
                    .filter(|c| c.complete == complete && !c.forced)
                    .collect();
                let active = unforced.iter().filter(|c| running.contains(&c.id)).count();
                prop_assert!(limit.is_none_or(|limit| active <= limit as usize));
                prop_assert_eq!(
                    active,
                    limit.map_or(unforced.len(), |limit| unforced.len().min(limit as usize))
                );
                // The ones running are the first ones.
                for (i, c) in unforced.iter().enumerate() {
                    prop_assert_eq!(running.contains(&c.id), i < active);
                }
            }
            for c in candidates.iter().filter(|c| c.forced) {
                prop_assert!(running.contains(&c.id));
            }
        }
    }
}
