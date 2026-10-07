//! Chunks, chunk trees, roots, proofs and Data verification.
//!
//! See `protocol/content.md` (Chunks, The chunk tree) and `protocol/messages.md` (Request, Data).
//!
//! A file is split into 16 KiB chunks. Their leaf hashes, padded with zero hashes to a power of
//! two, are combined pairwise into a binary tree whose top hash is the file's root. A Request
//! names one aligned node of that tree, and the Data answering it carries the node's chunks plus
//! the sibling hashes up to the root, so it can be verified on its own.

use std::sync::OnceLock;

use crate::hash::{Hash, leaf_hash, node_hash};

/// The size of a chunk in bytes. Every chunk is this size except possibly a file's last.
pub const CHUNK_SIZE: usize = 16 * 1024;

/// The number of chunks in a file of `size` bytes: `max(1, ⌈size / 16 KiB⌉)`.
///
/// An empty file has exactly one, empty, chunk.
pub fn chunk_count(size: u64) -> u64 {
    size.div_ceil(CHUNK_SIZE as u64).max(1)
}

/// The width of a tree over `chunk_count` chunks: the smallest power of two ≥ `chunk_count`.
pub fn tree_width(chunk_count: u64) -> u64 {
    chunk_count.next_power_of_two()
}

/// The height of a tree over `chunk_count` chunks: log₂ of its width.
pub fn tree_height(chunk_count: u64) -> u32 {
    tree_width(chunk_count).trailing_zeros()
}

/// The hash of a subtree of `2^height` padding leaves.
///
/// A padding leaf is 32 zero bytes, not a hash of anything, so these values depend only on the
/// height and are computed once.
pub fn padding_hash(height: u32) -> Hash {
    static TABLE: OnceLock<[Hash; 65]> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut table = [Hash::ZERO; 65];
        for h in 1..table.len() {
            table[h] = node_hash(&table[h - 1], &table[h - 1]);
        }
        table
    });
    table[height as usize]
}

/// The leaf hash of every chunk in `data`, a whole file's contents.
pub fn leaf_hashes(data: &[u8]) -> Vec<Hash> {
    if data.is_empty() {
        return vec![leaf_hash(&[])];
    }
    data.chunks(CHUNK_SIZE).map(leaf_hash).collect()
}

/// The root of a file whose whole contents are `data`.
pub fn root(data: &[u8]) -> Hash {
    let mut hasher = RootHasher::new();
    hasher.update(data);
    hasher.finish()
}

/// Computes a file's root from its contents as they stream in, using memory logarithmic in the
/// file's size.
#[derive(Debug, Clone, Default)]
pub struct RootHasher {
    /// A partial chunk not yet hashed.
    buffer: Vec<u8>,
    /// Roots of complete subtrees, as `(height, hash)`, with strictly decreasing heights.
    stack: Vec<(u32, Hash)>,
    /// The number of bytes seen.
    size: u64,
}

impl RootHasher {
    /// Starts hashing a file.
    pub fn new() -> Self {
        Self::default()
    }

    /// The number of bytes passed to [`update`](Self::update) so far.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// Feeds the next bytes of the file.
    pub fn update(&mut self, mut data: &[u8]) {
        self.size += data.len() as u64;
        while !data.is_empty() {
            let take = (CHUNK_SIZE - self.buffer.len()).min(data.len());
            self.buffer.extend_from_slice(&data[..take]);
            data = &data[take..];
            if self.buffer.len() == CHUNK_SIZE {
                self.push_leaf();
            }
        }
    }

    fn push_leaf(&mut self) {
        let mut hash = leaf_hash(&self.buffer);
        self.buffer.clear();
        let mut height = 0;
        while let Some(&(top_height, left)) = self.stack.last() {
            if top_height != height {
                break;
            }
            self.stack.pop();
            hash = node_hash(&left, &hash);
            height += 1;
        }
        self.stack.push((height, hash));
    }

    /// Finishes the file and returns its root.
    pub fn finish(mut self) -> Hash {
        if !self.buffer.is_empty() || self.stack.is_empty() {
            self.push_leaf();
        }
        // The smallest subtree is padded up to the height of the one before it, then combined,
        // until one hash covers the whole tree.
        let (mut height, mut hash) = self.stack.pop().expect("at least one leaf");
        while let Some((left_height, left)) = self.stack.pop() {
            while height < left_height {
                hash = node_hash(&hash, &padding_hash(height));
                height += 1;
            }
            hash = node_hash(&left, &hash);
            height += 1;
        }
        hash
    }
}

/// One node of a file's chunk tree: the subtree over chunk positions
/// `[index × 2^level, (index + 1) × 2^level)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TreeNode {
    /// The height above the leaves: 0 for a leaf.
    pub level: u32,
    /// The node's index among the nodes of its level, counting from the left.
    pub index: u64,
}

/// Why a range of chunks does not name exactly one node of a file's chunk tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidRequest {
    /// `count` was not a power of two.
    #[error("count {0} is not a power of two")]
    CountNotPowerOfTwo(u32),
    /// `start` was not within the file.
    #[error("start is past the file's last chunk")]
    StartPastEnd,
    /// `start`'s position was not a multiple of `count`.
    #[error("start is not aligned to count")]
    Misaligned,
    /// The range extended past the end of the file's tree.
    #[error("range extends past the end of the chunk tree")]
    PastEndOfTree,
}

impl TreeNode {
    /// The node that a Request for chunk positions `[position, position + count)` names, in a
    /// file of `chunk_count` chunks.
    pub fn for_range(chunk_count: u64, position: u32, count: u32) -> Result<Self, InvalidRequest> {
        if !count.is_power_of_two() {
            return Err(InvalidRequest::CountNotPowerOfTwo(count));
        }
        let (position, count) = (u64::from(position), u64::from(count));
        if position >= chunk_count {
            return Err(InvalidRequest::StartPastEnd);
        }
        if position % count != 0 {
            return Err(InvalidRequest::Misaligned);
        }
        if position + count > tree_width(chunk_count) {
            return Err(InvalidRequest::PastEndOfTree);
        }
        Ok(Self {
            level: count.trailing_zeros(),
            index: position / count,
        })
    }
}

/// A file's whole chunk tree, kept in memory so a provider can answer requests with proofs.
///
/// Only nodes over real chunks are stored. Nodes made entirely of padding are computed by
/// [`padding_hash`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkTree {
    /// `levels[0]` holds the leaf hashes; each level above holds their parents; the last holds
    /// the root.
    levels: Vec<Vec<Hash>>,
}

impl ChunkTree {
    /// Builds the tree over a file's leaf hashes.
    ///
    /// # Panics
    ///
    /// If `leaves` is empty: every file has at least one chunk.
    pub fn from_leaves(leaves: Vec<Hash>) -> Self {
        assert!(!leaves.is_empty(), "a file has at least one chunk");
        let height = tree_height(leaves.len() as u64);
        let mut levels = vec![leaves];
        for level in 0..height {
            let pad = padding_hash(level);
            let above = levels[level as usize]
                .chunks(2)
                .map(|pair| node_hash(&pair[0], pair.get(1).unwrap_or(&pad)))
                .collect();
            levels.push(above);
        }
        Self { levels }
    }

    /// Builds the tree over a file whose whole contents are `data`.
    pub fn from_data(data: &[u8]) -> Self {
        Self::from_leaves(leaf_hashes(data))
    }

    /// The file's root.
    pub fn root(&self) -> Hash {
        self.levels.last().expect("levels is never empty")[0]
    }

    /// The number of chunks in the file.
    pub fn chunk_count(&self) -> u64 {
        self.levels[0].len() as u64
    }

    /// The tree's height: log₂ of its width.
    pub fn height(&self) -> u32 {
        self.levels.len() as u32 - 1
    }

    /// The hash of `node`.
    ///
    /// # Panics
    ///
    /// If `node` lies outside the tree.
    pub fn node(&self, node: TreeNode) -> Hash {
        assert!(node.level <= self.height(), "node above the root");
        let level = &self.levels[node.level as usize];
        match usize::try_from(node.index) {
            Ok(i) if i < level.len() => level[i],
            _ => {
                assert!(
                    node.index < 1 << (self.height() - node.level),
                    "node past the end of the tree"
                );
                padding_hash(node.level)
            }
        }
    }

    /// The proof for `node`: the sibling of each node on the path from `node` up to (but not
    /// including) the root, lowest first.
    pub fn proof(&self, node: TreeNode) -> Vec<Hash> {
        (node.level..self.height())
            .map(|level| {
                let index = node.index >> (level - node.level);
                self.node(TreeNode {
                    level,
                    index: index ^ 1,
                })
            })
            .collect()
    }
}

/// Why a Data message failed verification. The requester MUST discard its chunks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum VerifyError {
    /// The requested range was not a valid Request for this file.
    #[error("invalid request: {0}")]
    InvalidRequest(#[from] InvalidRequest),
    /// The proof had the wrong number of hashes.
    #[error("proof has {actual} hashes, expected {expected}")]
    WrongProofLength {
        /// The number the request requires.
        expected: usize,
        /// The number received.
        actual: usize,
    },
    /// The chunks had the wrong total length.
    #[error("chunks are {actual} bytes, expected {expected}")]
    WrongDataLength {
        /// The number of bytes the requested chunks contain.
        expected: u64,
        /// The number received.
        actual: u64,
    },
    /// The chunks and proof did not hash to the file's root.
    #[error("chunks and proof do not produce the file's root")]
    RootMismatch,
}

/// The number of bytes that Data for chunk positions `[position, position + count)` of a file of
/// `size` bytes carries: every requested chunk that exists, concatenated.
pub fn data_len(size: u64, position: u32, count: u32) -> u64 {
    let chunk = CHUNK_SIZE as u64;
    let start = u64::from(position) * chunk;
    let end = (u64::from(position) + u64::from(count)) * chunk;
    end.min(size).saturating_sub(start)
}

/// Verifies Data received for chunk positions `[position, position + count)` of a file with the
/// given `size` and `root`, both taken from the manifest.
///
/// `position` is the range's start within the file (its bundle chunk index minus the file's first
/// chunk), as in `protocol/messages.md` (Verifying Data).
pub fn verify_data(
    size: u64,
    root: &Hash,
    position: u32,
    count: u32,
    proof: &[Hash],
    chunks: &[u8],
) -> Result<(), VerifyError> {
    let chunk_count = chunk_count(size);
    let node = TreeNode::for_range(chunk_count, position, count)?;

    // 1. Check the proof's length and the chunks' total length.
    let expected_proof = (tree_height(chunk_count) - node.level) as usize;
    if proof.len() != expected_proof {
        return Err(VerifyError::WrongProofLength {
            expected: expected_proof,
            actual: proof.len(),
        });
    }
    let expected_len = data_len(size, position, count);
    if chunks.len() as u64 != expected_len {
        return Err(VerifyError::WrongDataLength {
            expected: expected_len,
            actual: chunks.len() as u64,
        });
    }

    // 2. Hash each chunk into a leaf. An empty file's one chunk is empty.
    // 3. Combine the leaves into the range's node. Positions past the file's last chunk are
    //    padding leaves, so the node is the subtree over the real leaves, padded on the right up
    //    to the node's level.
    let leaves = leaf_hashes(chunks);
    let subtree = ChunkTree::from_leaves(leaves);
    let mut running = subtree.root();
    for level in subtree.height()..node.level {
        running = node_hash(&running, &padding_hash(level));
    }

    // 4. Combine the running hash with each proof hash, on the side given by the node's index.
    let mut index = node.index;
    for sibling in proof {
        running = if index % 2 == 0 {
            node_hash(&running, sibling)
        } else {
            node_hash(sibling, &running)
        };
        index /= 2;
    }

    // 5. The result must be the file's root.
    if running == *root {
        Ok(())
    } else {
        Err(VerifyError::RootMismatch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_counts() {
        assert_eq!(chunk_count(0), 1);
        assert_eq!(chunk_count(1), 1);
        assert_eq!(chunk_count(16_384), 1);
        assert_eq!(chunk_count(16_385), 2);
        assert_eq!(tree_width(3), 4);
        assert_eq!(tree_height(1), 0);
        assert_eq!(tree_height(3), 2);
    }

    #[test]
    fn padding_hashes_build_on_each_other() {
        assert_eq!(padding_hash(0), Hash::ZERO);
        assert_eq!(padding_hash(1), node_hash(&Hash::ZERO, &Hash::ZERO));
        assert_eq!(
            padding_hash(2),
            node_hash(&padding_hash(1), &padding_hash(1))
        );
    }

    #[test]
    fn invalid_ranges_are_rejected() {
        // 3 chunks, width 4.
        assert_eq!(
            TreeNode::for_range(3, 0, 3),
            Err(InvalidRequest::CountNotPowerOfTwo(3))
        );
        assert_eq!(
            TreeNode::for_range(3, 0, 0),
            Err(InvalidRequest::CountNotPowerOfTwo(0))
        );
        assert_eq!(
            TreeNode::for_range(3, 3, 1),
            Err(InvalidRequest::StartPastEnd)
        );
        assert_eq!(
            TreeNode::for_range(3, 1, 2),
            Err(InvalidRequest::Misaligned)
        );
        assert_eq!(
            TreeNode::for_range(3, 0, 8),
            Err(InvalidRequest::PastEndOfTree)
        );
        assert_eq!(
            TreeNode::for_range(3, 2, 2),
            Ok(TreeNode { level: 1, index: 1 })
        );
    }

    #[test]
    fn data_len_omits_missing_chunks() {
        assert_eq!(data_len(0, 0, 1), 0);
        assert_eq!(data_len(40_000, 2, 2), 40_000 - 2 * 16_384);
        assert_eq!(data_len(40_000, 0, 4), 40_000);
        assert_eq!(data_len(40_000, 0, 1), 16_384);
    }
}
