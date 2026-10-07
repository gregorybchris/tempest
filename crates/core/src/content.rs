//! An in-memory bundle store, for tests and the simulator.
//!
//! A driver uses it to build the Data a session asks it to send, and to keep the chunks of Data
//! that passed verification. It holds every file's chunk tree from the start, so a peer that has
//! only some chunks can still build proofs. A real client keeps the proof hashes it received
//! instead.

use std::sync::Arc;

use tempest_protocol::message::Data;
use tempest_protocol::tree::{CHUNK_SIZE, ChunkTree, data_len};
use tempest_protocol::{ChunkLayout, Encode, Entry, FileInfo, Manifest};

/// The files of one bundle, as one peer holds them.
#[derive(Debug, Clone)]
pub struct MemoryContent {
    manifest: Arc<Manifest>,
    layout: ChunkLayout,
    trees: Arc<Vec<ChunkTree>>,
    /// Each file's bytes, in layout order. Chunks not yet stored are zero.
    files: Vec<Vec<u8>>,
}

impl MemoryContent {
    /// A bundle named `name` holding `files`, each a path and its contents, with every chunk
    /// stored.
    ///
    /// # Panics
    ///
    /// If the files don't make a valid manifest.
    pub fn new(name: &str, files: Vec<(String, Vec<u8>)>) -> Self {
        let mut files = files;
        files.sort_by(|a, b| a.0.cmp(&b.0));
        let trees: Vec<ChunkTree> = files.iter().map(|(_, d)| ChunkTree::from_data(d)).collect();
        let entries = files
            .iter()
            .zip(&trees)
            .map(|((path, data), tree)| {
                let info = FileInfo {
                    size: data.len() as u64,
                    root: tree.root(),
                    executable: false,
                    modified: 0,
                };
                Entry::file(path.clone(), info)
            })
            .collect();
        let manifest = Manifest::new(name, "", 0, "tempest-core", entries).expect("valid files");
        Self {
            layout: ChunkLayout::new(&manifest),
            manifest: Arc::new(manifest),
            trees: Arc::new(trees),
            files: files.into_iter().map(|(_, data)| data).collect(),
        }
    }

    /// The same bundle with no chunks stored.
    pub fn empty_copy(&self) -> Self {
        Self {
            manifest: self.manifest.clone(),
            layout: self.layout.clone(),
            trees: self.trees.clone(),
            files: self.files.iter().map(|f| vec![0; f.len()]).collect(),
        }
    }

    /// The manifest.
    pub fn manifest(&self) -> &Arc<Manifest> {
        &self.manifest
    }

    /// The manifest's encoding.
    pub fn manifest_bytes(&self) -> Vec<u8> {
        self.manifest.to_bytes()
    }

    /// Each file's bytes, in manifest order.
    pub fn files(&self) -> &[Vec<u8>] {
        &self.files
    }

    /// Builds Data for a valid Request for `[start, start + count)` from the stored bytes.
    ///
    /// # Panics
    ///
    /// If the request is invalid.
    pub fn data(&self, start: u32, count: u32) -> Data {
        let (file, node) = self
            .layout
            .request_node(start, count)
            .expect("valid request");
        let position = start - self.layout.files()[file].first_chunk;
        let (from, len) = self.byte_range(file, position, count);
        Data {
            proof: self.trees[file].proof(node),
            chunks: self.files[file][from..from + len].to_vec(),
        }
    }

    /// Stores the chunks of verified Data answering a Request for `[start, start + count)`.
    ///
    /// # Panics
    ///
    /// If the request is invalid or the Data has the wrong length.
    pub fn write(&mut self, start: u32, count: u32, data: &Data) {
        let (file, _) = self
            .layout
            .request_node(start, count)
            .expect("valid request");
        let position = start - self.layout.files()[file].first_chunk;
        let (from, len) = self.byte_range(file, position, count);
        self.files[file][from..from + len].copy_from_slice(&data.chunks);
    }

    fn byte_range(&self, file: usize, position: u32, count: u32) -> (usize, usize) {
        let size = self.files[file].len() as u64;
        let from = (u64::from(position) * CHUNK_SIZE as u64).min(size);
        (from as usize, data_len(size, position, count) as usize)
    }
}
