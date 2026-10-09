//! What the detail views show from inside the selected transfer: its files, peers and chunk map,
//! fetched from the engine while a view shows them (D-193).

use std::mem::discriminant;

use tempest_api::inspect::{ChunkMapInfo, FileInfo, PeerInfo};
use tempest_protocol::BundleId;

use super::Reply;

/// Something to fetch from inside a transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Look {
    /// Its files.
    Files,
    /// Its connections.
    Peers,
    /// Its chunk map, in at most this many cells.
    Chunks(u32),
}

/// What has been fetched, or why it couldn't be.
pub type Fetched<T> = Option<Result<T, String>>;

/// What has been fetched from inside one transfer.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Inside {
    id: Option<BundleId>,
    /// Its files, once fetched.
    pub files: Fetched<Vec<FileInfo>>,
    /// Its connections, once fetched.
    pub peers: Fetched<Vec<PeerInfo>>,
    /// Its chunk map, once fetched.
    pub chunks: Fetched<ChunkMapInfo>,
    /// What has been asked for and not answered yet.
    pending: Vec<Look>,
}

impl Inside {
    /// What has been fetched from `id`; nothing if it is another transfer's.
    pub fn of(&self, id: BundleId) -> Option<&Self> {
        (self.id == Some(id)).then_some(self)
    }

    /// Notes that `look` is being fetched from `id`, forgetting another transfer's. Returns
    /// false if the same kind of fetch is already under way, so there's no need for another.
    pub fn start(&mut self, id: BundleId, look: Look) -> bool {
        if self.id != Some(id) {
            *self = Self {
                id: Some(id),
                ..Self::default()
            };
        }
        if self.is_pending(look) {
            return false;
        }
        self.pending.push(look);
        true
    }

    /// Takes in the answer to fetching `look` from `id`.
    pub fn receive(&mut self, id: BundleId, look: Look, result: Result<Reply, String>) {
        if self.id != Some(id) {
            return;
        }
        self.pending
            .retain(|pending| discriminant(pending) != discriminant(&look));
        match (look, result) {
            (Look::Files, Ok(Reply::Files(files))) => self.files = Some(Ok(files)),
            (Look::Peers, Ok(Reply::Peers(peers))) => self.peers = Some(Ok(peers)),
            (Look::Chunks(_), Ok(Reply::Chunks(map))) => self.chunks = Some(Ok(map)),
            (Look::Files, Err(error)) => self.files = Some(Err(error)),
            (Look::Peers, Err(error)) => self.peers = Some(Err(error)),
            (Look::Chunks(_), Err(error)) => self.chunks = Some(Err(error)),
            (_, Ok(_)) => {}
        }
    }

    fn is_pending(&self, look: Look) -> bool {
        self.pending
            .iter()
            .any(|pending| discriminant(pending) == discriminant(&look))
    }
}

#[cfg(test)]
mod tests {
    use tempest_protocol::Hash;

    use super::*;

    #[test]
    fn one_fetch_of_a_kind_at_a_time_for_the_transfer_shown() {
        let (a, b) = (Hash::new([1; 32]), Hash::new([2; 32]));
        let mut inside = Inside::default();
        assert!(inside.start(a, Look::Chunks(10)));
        assert!(!inside.start(a, Look::Chunks(20)), "already fetching");
        assert!(inside.start(a, Look::Peers));
        inside.receive(a, Look::Chunks(10), Err("gone".to_owned()));
        assert_eq!(inside.chunks, Some(Err("gone".to_owned())));
        assert!(inside.start(a, Look::Chunks(20)));

        // Another transfer starts afresh, and a late answer about the first is dropped.
        assert!(inside.start(b, Look::Peers));
        inside.receive(a, Look::Files, Ok(Reply::Files(Vec::new())));
        assert_eq!(inside.of(b).unwrap().files, None);
        assert!(inside.of(a).is_none());
        inside.receive(b, Look::Peers, Ok(Reply::Peers(Vec::new())));
        assert_eq!(inside.peers, Some(Ok(Vec::new())));
    }
}
