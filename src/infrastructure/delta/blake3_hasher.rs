//! Parallel BLAKE3 content hasher (Section 13: Integrity Hashing).
//! Uses blake3 crate with rayon feature for multithreaded + SIMD hashing.

use crate::application::ports::ContentHasher;
use crate::domain::{BlockSignature, FileManifest, FileSignature};
use crate::error::{Result, UniFlowError};
use blake3::Hasher;
use rayon::prelude::*;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

const DEFAULT_BLOCK_SIZE: u32 = 4096; // 4KiB blocks (tunable, same as many rsync configs)

pub struct ParallelBlake3Hasher {
    block_size: u32,
}

impl ParallelBlake3Hasher {
    pub fn new() -> Self {
        Self {
            block_size: DEFAULT_BLOCK_SIZE,
        }
    }

    pub fn with_block_size(mut self, size: u32) -> Self {
        self.block_size = size;
        self
    }
}

impl Default for ParallelBlake3Hasher {
    fn default() -> Self {
        Self::new()
    }
}

impl ContentHasher for ParallelBlake3Hasher {
    fn hash_file_parallel(&self, path: &Path) -> Result<[u8; 32]> {
        let mut file = File::open(path)?;
        let mut hasher = Hasher::new();
        // Use blake3's rayon support for parallel hashing of large files
        hasher.update_rayon(&read_all(&mut file)?);
        Ok(*hasher.finalize().as_bytes())
    }

    fn hash_blocks_parallel(&self, path: &Path, block_size: u32) -> Result<FileManifest> {
        let mut file = File::open(path)?;
        let metadata = file.metadata()?;
        let total_size = metadata.len();
        let block_size = if block_size == 0 { self.block_size } else { block_size };

        let num_blocks = ((total_size + block_size as u64 - 1) / block_size as u64) as usize;

        // Read the whole file (or use mmap for very large files in future)
        let mut data = Vec::with_capacity(total_size as usize);
        file.read_to_end(&mut data)?;

        let blocks: Vec<BlockSignature> = (0..num_blocks)
            .into_par_iter()
            .map(|i| {
                let offset = (i as u64) * (block_size as u64);
                let end = std::cmp::min(offset + block_size as u64, total_size);
                let chunk = &data[offset as usize..end as usize];
                let size = (end - offset) as u32;

                // BLAKE3 (SIMD accelerated)
                let hash = blake3::hash(chunk);

                // For weak rolling sum we use a simple fallback (real librsync weak is used in delta phase).
                // This is sufficient for Phase 1 dedup + verification.
                let weak = simple_rolling_checksum(chunk);

                BlockSignature {
                    offset,
                    size,
                    blake3: *hash.as_bytes(),
                    weak,
                }
            })
            .collect();

        // Root hash of the whole file (for quick verification)
        let root_hash = blake3::hash(&data);

        let signature = FileSignature {
            block_size,
            blocks,
            total_size,
        };

        Ok(FileManifest {
            signature,
            root_blake3: *root_hash.as_bytes(),
        })
    }
}

fn read_all(file: &mut File) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    Ok(buf)
}

/// Simple 32-bit rolling checksum (inspired by rsync weak sum).
/// Real librsync weak sum is used when calling the C API for delta matching.
fn simple_rolling_checksum(data: &[u8]) -> u32 {
    let mut s1: u32 = 0;
    let mut s2: u32 = 0;
    for (i, &b) in data.iter().enumerate() {
        s1 = s1.wrapping_add(b as u32);
        s2 = s2.wrapping_add((data.len() - i) as u32 * b as u32);
    }
    s1.wrapping_add(s2 << 16)
}