//! A file's chunk tree as far as this peer knows it, for building proofs.
//!
//! A sharing peer knows every node. A downloading peer learns the leaves of the chunks it stores
//! and the proof hashes that came with them. That is always enough to prove any range it has:
//! the Data that brought a range's chunks also brought the siblings of the range's ancestors.

use tempest_protocol::Hash;
use tempest_protocol::hash::node_hash;
use tempest_protocol::tree::{TreeNode, padding_hash, tree_height};

/// How many nodes of a level share one allocation. Nodes are stored a page at a time, as they
/// become known, so a download's tree takes memory only for what it has learned.
const PAGE: u64 = 1024;

/// The known nodes of one file's chunk tree.
///
/// Only nodes over real chunks are stored. Nodes made entirely of padding are computed by
/// [`padding_hash`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FileTree {
    /// `levels[0]` holds the leaf hashes; each level above holds their parents; the last holds
    /// the root.
    levels: Vec<Level>,
}

/// One level of a tree: `len` nodes, in pages allocated once one of their nodes is known. A
/// page is only allocated to store a node, so trees that know the same nodes are equal.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Level {
    len: u64,
    pages: Vec<Option<Box<[Option<Hash>]>>>,
}

impl Level {
    fn new(len: u64) -> Self {
        Self {
            len,
            pages: vec![None; len.div_ceil(PAGE) as usize],
        }
    }

    /// The node at `index`: `None` past the level's end, `Some(None)` if not known.
    fn get(&self, index: u64) -> Option<Option<Hash>> {
        if index >= self.len {
            return None;
        }
        let page = &self.pages[(index / PAGE) as usize];
        Some(page.as_ref().and_then(|page| page[(index % PAGE) as usize]))
    }

    /// The slot for the node at `index`, allocating its page; `None` past the level's end.
    fn slot(&mut self, index: u64) -> Option<&mut Option<Hash>> {
        if index >= self.len {
            return None;
        }
        let page = self.pages[(index / PAGE) as usize]
            .get_or_insert_with(|| vec![None; PAGE as usize].into_boxed_slice());
        Some(&mut page[(index % PAGE) as usize])
    }

    /// The known nodes and their indexes, in order.
    fn known(&self) -> impl Iterator<Item = (u64, Hash)> + '_ {
        self.pages.iter().enumerate().flat_map(|(p, page)| {
            let first = p as u64 * PAGE;
            page.iter()
                .flat_map(|page| page.iter().enumerate())
                .filter_map(move |(i, hash)| hash.map(|hash| (first + i as u64, hash)))
        })
    }
}

impl FileTree {
    /// The tree of a file of `chunk_count` chunks, knowing only its root.
    pub fn new(chunk_count: u64, root: Hash) -> Self {
        let mut tree = Self::empty(chunk_count);
        tree.set(tree.height(), 0, root);
        tree
    }

    /// The tree of a file of `chunk_count` chunks, knowing nothing.
    fn empty(chunk_count: u64) -> Self {
        let height = tree_height(chunk_count);
        Self {
            levels: (0..=height)
                .map(|level| Level::new(chunk_count.div_ceil(1 << level)))
                .collect(),
        }
    }

    /// The whole tree over a file's leaf hashes.
    pub fn from_leaves(leaves: &[Hash]) -> Self {
        let mut tree = Self::empty(leaves.len() as u64);
        let height = tree.height();
        tree.insert(
            TreeNode {
                level: height,
                index: 0,
            },
            leaves,
            &[],
        );
        tree
    }

    /// The file's root, if known.
    pub fn root(&self) -> Option<Hash> {
        self.get(self.height(), 0)
    }

    fn height(&self) -> u32 {
        self.levels.len() as u32 - 1
    }

    /// The hash of the node at `level` and `index`, if known. Nodes past the file's last chunk
    /// are padding, and always known.
    fn get(&self, level: u32, index: u64) -> Option<Hash> {
        self.levels[level as usize]
            .get(index)
            .unwrap_or(Some(padding_hash(level)))
    }

    fn set(&mut self, level: u32, index: u64, hash: Hash) {
        if let Some(slot) = self.levels[level as usize].slot(index) {
            debug_assert!(slot.is_none_or(|known| known == hash), "conflicting hash");
            *slot = Some(hash);
        }
    }

    /// Records verified Data for `node`: the leaf hashes of its chunks that exist, and its proof.
    /// Every node that can now be computed is.
    pub fn insert(&mut self, node: TreeNode, leaves: &[Hash], proof: &[Hash]) {
        let first = node.index << node.level;
        for (i, &leaf) in leaves.iter().enumerate() {
            self.set(0, first + i as u64, leaf);
        }
        for (k, &sibling) in proof.iter().enumerate() {
            self.set(node.level + k as u32, (node.index >> k) ^ 1, sibling);
        }
        // Recompute the parents of everything that changed, level by level. Above the node, the
        // span is the node's ancestor, whose sibling came in the proof.
        let last = first + leaves.len().max(1) as u64 - 1;
        for level in 0..self.height() {
            for parent in (first >> (level + 1))..=(last >> (level + 1)) {
                let left = self.get(level, 2 * parent);
                let right = self.get(level, 2 * parent + 1);
                if let (Some(left), Some(right)) = (left, right) {
                    self.set(level + 1, parent, node_hash(&left, &right));
                }
            }
        }
    }

    /// The known nodes that can't be computed from known children: every known leaf, and above
    /// them every known node missing a child. [`from_known`](Self::from_known) rebuilds the tree
    /// from them.
    pub fn known_nodes(&self) -> Vec<(TreeNode, Hash)> {
        let mut known = Vec::new();
        for (level, nodes) in self.levels.iter().enumerate() {
            let level = level as u32;
            for (index, hash) in nodes.known() {
                let computable = level > 0
                    && self.get(level - 1, 2 * index).is_some()
                    && self.get(level - 1, 2 * index + 1).is_some();
                if !computable {
                    known.push((TreeNode { level, index }, hash));
                }
            }
        }
        known
    }

    /// The tree of a file of `chunk_count` chunks with root `root`, knowing `nodes` and every
    /// node they let it compute. `None` if a node lies outside the tree or the nodes contradict
    /// each other or the root.
    pub fn from_known(chunk_count: u64, root: Hash, nodes: &[(TreeNode, Hash)]) -> Option<Self> {
        let mut tree = Self::new(chunk_count, root);
        for &(node, hash) in nodes {
            let slot = tree.levels.get_mut(node.level as usize)?.slot(node.index)?;
            if slot.is_some_and(|known| known != hash) {
                return None;
            }
            *slot = Some(hash);
        }
        for level in 0..tree.height() {
            // Only the parent of a known node can be computed.
            let mut parents: Vec<u64> = tree.levels[level as usize]
                .known()
                .map(|(index, _)| index / 2)
                .collect();
            parents.dedup();
            for parent in parents {
                let left = tree.get(level, 2 * parent);
                let right = tree.get(level, 2 * parent + 1);
                if let (Some(left), Some(right)) = (left, right) {
                    let hash = node_hash(&left, &right);
                    let slot = (tree.levels[level as usize + 1].slot(parent))
                        .expect("a node's parent is in the tree");
                    if slot.is_some_and(|known| known != hash) {
                        return None;
                    }
                    *slot = Some(hash);
                }
            }
        }
        // Each node must connect to the root: with every sibling on its path known, each of its
        // ancestors was computed from it and checked above. A node without that path could be
        // anything.
        nodes
            .iter()
            .all(|&(node, _)| tree.proof(node).is_some())
            .then_some(tree)
    }

    /// The leaf hash of chunk `position`, if known.
    pub fn leaf(&self, position: u64) -> Option<Hash> {
        self.levels[0].get(position).flatten()
    }

    /// The proof for `node`: the sibling of each node on the path from `node` up to (but not
    /// including) the root, lowest first. `None` if one isn't known.
    pub fn proof(&self, node: TreeNode) -> Option<Vec<Hash>> {
        (node.level..self.height())
            .map(|level| self.get(level, (node.index >> (level - node.level)) ^ 1))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use tempest_protocol::hash::leaf_hash;
    use tempest_protocol::tree::{ChunkTree, tree_width};

    use super::*;

    fn leaves(count: u64) -> Vec<Hash> {
        (0..count).map(|i| leaf_hash(&i.to_le_bytes())).collect()
    }

    /// Every valid node of a file of `count` chunks.
    fn nodes(count: u64) -> Vec<TreeNode> {
        let height = tree_height(count);
        (0..=height)
            .flat_map(|level| {
                (0..tree_width(count) >> level)
                    .map(move |index| TreeNode { level, index })
                    .filter(move |node| node.index << node.level < count)
            })
            .collect()
    }

    #[test]
    fn a_whole_tree_matches_the_protocol() {
        for count in [1, 2, 3, 5, 8, 13] {
            let leaves = leaves(count);
            let reference = ChunkTree::from_leaves(leaves.clone());
            let tree = FileTree::from_leaves(&leaves);
            assert_eq!(tree.root(), Some(reference.root()));
            for node in nodes(count) {
                assert_eq!(tree.proof(node), Some(reference.proof(node)), "{node:?}");
            }
        }
    }

    #[test]
    fn an_empty_file_has_one_leaf() {
        let tree = FileTree::from_leaves(&[leaf_hash(&[])]);
        assert_eq!(tree.root(), Some(leaf_hash(&[])));
        assert_eq!(tree.proof(TreeNode { level: 0, index: 0 }), Some(vec![]));
    }

    #[test]
    fn contradicting_nodes_are_refused() {
        let leaves = leaves(4);
        let reference = ChunkTree::from_leaves(leaves.clone());
        let full = FileTree::from_leaves(&leaves);
        let mut known = full.known_nodes();
        assert_eq!(known.len(), 4, "only the leaves are kept");
        assert!(FileTree::from_known(4, reference.root(), &known).is_some());
        // A wrong leaf no longer hashes to the root.
        known[0].1 = leaf_hash(b"other");
        assert_eq!(FileTree::from_known(4, reference.root(), &known), None);
        // A node outside the tree.
        let outside = [(TreeNode { level: 0, index: 9 }, leaves[0])];
        assert_eq!(FileTree::from_known(4, reference.root(), &outside), None);
        let above = [(TreeNode { level: 7, index: 0 }, leaves[0])];
        assert_eq!(FileTree::from_known(4, reference.root(), &above), None);
        // A leaf whose sibling isn't known can't be checked against the root, whatever it is.
        let orphan = [(TreeNode { level: 0, index: 0 }, leaf_hash(b"other"))];
        assert_eq!(FileTree::from_known(4, reference.root(), &orphan), None);
    }

    #[test]
    fn a_large_tree_allocates_only_what_it_knows() {
        // A file of 2^30 chunks (16 TiB): full levels would take tens of gigabytes.
        let count = 1u64 << 30;
        let leaves = leaves(2);
        // The first two leaves, then every sibling on the way up stands for unknown chunks.
        let proof: Vec<Hash> = (1..30).map(|level| leaf_hash(&[level as u8])).collect();
        let root = (proof.iter()).fold(node_hash(&leaves[0], &leaves[1]), |node, sibling| {
            node_hash(&node, sibling)
        });
        let mut tree = FileTree::new(count, root);
        let pages = |tree: &FileTree| -> usize {
            (tree.levels.iter())
                .map(|level| level.pages.iter().flatten().count())
                .sum()
        };
        assert_eq!(pages(&tree), 1, "only the root's page");
        tree.insert(TreeNode { level: 1, index: 0 }, &leaves, &proof);
        assert_eq!(tree.leaf(1), Some(leaves[1]));
        // A page per level for the inserted leaves, their ancestors and the proof's siblings.
        assert!(pages(&tree) <= 31, "{} pages", pages(&tree));
    }

    proptest! {
        /// After inserting any sequence of verified ranges, a downloader can prove exactly the
        /// ranges whose chunks it has, with the same proofs as the full tree.
        #[test]
        fn learned_proofs_match_the_full_tree(
            count in 1u64..40,
            picks in prop::collection::vec(any::<prop::sample::Index>(), 0..12),
        ) {
            let leaves = leaves(count);
            let reference = ChunkTree::from_leaves(leaves.clone());
            let all = nodes(count);
            let mut tree = FileTree::new(count, reference.root());
            let mut have = vec![false; count as usize];
            for pick in picks {
                let node = *pick.get(&all);
                let first = (node.index << node.level) as usize;
                let end = (first + (1 << node.level)).min(count as usize);
                tree.insert(node, &leaves[first..end], &reference.proof(node));
                have[first..end].fill(true);
            }
            for node in all {
                let first = (node.index << node.level) as usize;
                let end = (first + (1 << node.level)).min(count as usize);
                if have[first..end].iter().all(|&h| h) {
                    prop_assert_eq!(tree.proof(node), Some(reference.proof(node)));
                }
            }
            prop_assert_eq!(tree.root(), Some(reference.root()));

            // The nodes it keeps rebuild exactly the same tree.
            let rebuilt = FileTree::from_known(count, reference.root(), &tree.known_nodes());
            prop_assert_eq!(rebuilt, Some(tree));
        }
    }
}
