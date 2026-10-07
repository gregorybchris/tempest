//! The simulated bundle: its manifest, and the size of every message that carries its chunks.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempest_core::content::MemoryContent;
use tempest_protocol::message::{Data, MAX_PROOF_LEN, ManifestMessage};
use tempest_protocol::tree::{data_len, tree_height};
use tempest_protocol::{ChunkLayout, Encode, Entry, FileInfo, Hash, Manifest, Message, tree};

use crate::units::ByteSize;

/// The bundle's files, as a scenario file lists them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleSpec {
    /// The files, in groups of equal size.
    pub files: Vec<FileSpec>,
}

/// A group of files of one size.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileSpec {
    /// Each file's size.
    pub size: ByteSize,
    /// How many files. Default 1.
    #[serde(default = "one")]
    pub count: u32,
}

fn one() -> u32 {
    1
}

/// The most files a simulated bundle may have.
const MAX_FILES: u64 = 100_000;

/// The bundle every peer in a run shares.
#[derive(Debug, Clone)]
pub struct Bundle {
    manifest: Arc<Manifest>,
    manifest_frame: usize,
    layout: ChunkLayout,
    sizes: Vec<u64>,
    /// The frame length of Data with each proof length and no chunks.
    data_frame_base: Vec<usize>,
}

impl BundleSpec {
    /// The file sizes, in manifest order.
    fn sizes(&self) -> Result<Vec<u64>, String> {
        let count: u64 = self.files.iter().map(|f| u64::from(f.count)).sum();
        if count == 0 {
            return Err("needs at least one file".to_owned());
        }
        if count > MAX_FILES {
            return Err(format!(
                "{count} files is more than the limit of {MAX_FILES}"
            ));
        }
        Ok(self
            .files
            .iter()
            .flat_map(|f| std::iter::repeat_n(f.size.0, f.count as usize))
            .collect())
    }

    /// The bundle with made-up roots, for virtual payloads.
    pub fn build(&self) -> Result<Bundle, String> {
        let sizes = self.sizes()?;
        let entries = sizes
            .iter()
            .enumerate()
            .map(|(i, &size)| {
                let root = if size == 0 {
                    tree::root(&[])
                } else {
                    Hash::new(Sha256::digest(format!("tempest-sim file {i}")).into())
                };
                let info = FileInfo {
                    size,
                    root,
                    executable: false,
                    modified: 0,
                };
                Entry::file(file_name(i), info)
            })
            .collect();
        let manifest =
            Manifest::new(NAME, "", 0, "tempest-sim", entries).map_err(|e| e.to_string())?;
        Bundle::new(Arc::new(manifest))
    }

    /// The bundle with real bytes, made the same way on every run.
    pub fn build_real(&self) -> Result<(Bundle, MemoryContent), String> {
        let files = self
            .sizes()?
            .into_iter()
            .enumerate()
            .map(|(i, size)| (file_name(i), file_bytes(i as u64, size as usize)))
            .collect();
        let content = MemoryContent::new(NAME, files);
        Ok((Bundle::new(content.manifest().clone())?, content))
    }
}

const NAME: &str = "simulated";

fn file_name(index: usize) -> String {
    format!("file-{index:05}")
}

/// Bytes that look random but are the same on every run.
fn file_bytes(seed: u64, len: usize) -> Vec<u8> {
    let mut state = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}

impl Bundle {
    fn new(manifest: Arc<Manifest>) -> Result<Self, String> {
        let layout = ChunkLayout::new(&manifest);
        if layout.chunk_count() == 0 {
            return Err("has no chunks".to_owned());
        }
        let sizes = manifest
            .entries()
            .iter()
            .filter_map(|e| e.as_file().map(|f| f.size))
            .collect();
        let manifest_frame = Message::Manifest(ManifestMessage {
            manifest: manifest.to_bytes(),
        })
        .encode_frame()
        .len();
        let data_frame_base = (0..=MAX_PROOF_LEN as usize)
            .map(|proof| {
                Message::Data(Data {
                    proof: vec![Hash::ZERO; proof],
                    chunks: Vec::new(),
                })
                .encode_frame()
                .len()
            })
            .collect();
        Ok(Self {
            manifest,
            manifest_frame,
            layout,
            sizes,
            data_frame_base,
        })
    }

    /// The manifest.
    pub fn manifest(&self) -> &Arc<Manifest> {
        &self.manifest
    }

    /// The number of chunks.
    pub fn chunk_count(&self) -> u32 {
        self.layout.chunk_count()
    }

    /// The total size of the files.
    pub fn total_size(&self) -> u64 {
        self.sizes.iter().sum()
    }

    /// The length of a Manifest message's frame.
    pub fn manifest_frame_len(&self) -> usize {
        self.manifest_frame
    }

    /// The payload bytes and frame length of Data answering a valid Request for
    /// `[start, start + count)`.
    ///
    /// # Panics
    ///
    /// If the request is invalid, which a session never asks to answer.
    pub fn data_len(&self, start: u32, count: u32) -> (u64, usize) {
        let (file, node) = self
            .layout
            .request_node(start, count)
            .expect("sessions answer only valid requests");
        let chunks = &self.layout.files()[file];
        let size = self.sizes[file];
        let payload = data_len(size, start - chunks.first_chunk, count);
        let proof = tree_height(chunks.chunk_count.into()) - node.level;
        (
            payload,
            self.data_frame_base[proof as usize] + payload as usize,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(sizes: &[(u64, u32)]) -> BundleSpec {
        BundleSpec {
            files: sizes
                .iter()
                .map(|&(size, count)| FileSpec {
                    size: ByteSize(size),
                    count,
                })
                .collect(),
        }
    }

    #[test]
    fn data_lengths_match_real_encodings() {
        let spec = spec(&[(100_000, 1), (16_384 * 4, 2), (0, 1), (1, 1)]);
        let (bundle, content) = spec.build_real().unwrap();
        let virtual_bundle = spec.build().unwrap();
        for (start, count) in [
            (0, 1),
            (0, 4),
            (4, 2),
            (6, 1),
            (7, 4),
            (11, 4),
            (15, 1),
            (17, 1),
        ] {
            if bundle.layout.request_node(start, count).is_err() {
                continue;
            }
            let data = content.data(start, count);
            let frame = Message::Data(data.clone()).encode_frame().len();
            assert_eq!(
                bundle.data_len(start, count),
                (data.chunks.len() as u64, frame)
            );
            assert_eq!(
                virtual_bundle.data_len(start, count),
                bundle.data_len(start, count)
            );
        }
    }

    #[test]
    fn real_bundles_are_the_same_every_time() {
        let spec = spec(&[(50_000, 2)]);
        let (a, _) = spec.build_real().unwrap();
        let (b, _) = spec.build_real().unwrap();
        assert_eq!(a.manifest().bundle_id(), b.manifest().bundle_id());
        assert_eq!(spec.build().unwrap().chunk_count(), 8);
    }

    #[test]
    fn empty_bundles_are_refused() {
        assert!(spec(&[]).build().is_err());
        assert!(spec(&[(1, 0)]).build().is_err());
        assert_eq!(spec(&[(0, 2)]).build().unwrap().chunk_count(), 2);
    }
}
