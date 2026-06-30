//! Pure-Rust block-level delta engine (rsync-style: rolling weak checksum +
//! strong BLAKE3), replacing the former `librsync-sys` FFI stub.
//!
//! Why pure-Rust (no new dependency): the classic rsync algorithm is small and
//! the repo already pulls in `blake3` for the strong hash, so we avoid the C
//! `librsync` build dependency entirely while still producing real block-level
//! `Copy`/`Literal` deltas. (`fast_rsync` was considered but rejected: it would
//! add a crate for ~120 lines of well-understood code.)
//!
//! Algorithm (matches `docs/phase1-delta-engine.md`):
//!   * `generate_signature` splits the **basis** (current destination) into fixed
//!     `block_size` blocks; each block gets a weak rolling checksum (fast, 32-bit)
//!     and a strong BLAKE3 (collision-resistant).
//!   * `create_delta` slides a `block_size` window over the **source**, rolling the
//!     weak checksum one byte at a time. On a weak hit it confirms with BLAKE3; a
//!     confirmed match emits `Copy{old_offset,size}` (no bytes moved), otherwise the
//!     unmatched byte joins a `Literal` run. So only changed regions become literals.
//!   * `apply_delta` reconstructs the new file from (old basis + delta) and publishes
//!     it **atomically** (temp file + fsync + rename) — never a partial destination.
//!
//! Memory: this path loads the source and basis into memory (the delta engine is the
//! small-file / on-prem path; large payloads go through `ParallelTransport`, which
//! streams in bounded chunks). Documented trade-off, not a regression.

use crate::domain::{BlockSignature, DeltaChunk, DeltaInstruction, FileSignature};
use crate::error::{Result, UniFlowError};
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

/// Default block size (bytes). Small enough to localise edits, large enough to
/// keep the signature compact. 4 KiB matches the rest of the Phase-1 engine.
const DEFAULT_BLOCK_SIZE: u32 = 4096;

/// Cap on a single `Literal` instruction so a fully-changed large file is split
/// into streamable pieces rather than one giant allocation in the delta.
const MAX_LITERAL_RUN: usize = 1 << 20; // 1 MiB

/// Classic rsync rolling weak checksum (Adler-style, modulus 2^16).
/// `a` = sum of bytes; `b` = weighted sum. Packed as `(b << 16) | a`.
#[derive(Clone, Copy)]
struct RollingChecksum {
    a: u32,
    b: u32,
    len: u32,
}

impl RollingChecksum {
    /// Compute over a full window `data[..]`.
    fn new(data: &[u8]) -> Self {
        let mut a: u32 = 0;
        let mut b: u32 = 0;
        let len = data.len() as u32;
        for (i, &byte) in data.iter().enumerate() {
            a = a.wrapping_add(byte as u32);
            b = b.wrapping_add((len - i as u32) * byte as u32);
        }
        Self { a: a & 0xffff, b: b & 0xffff, len }
    }

    fn digest(&self) -> u32 {
        (self.b << 16) | (self.a & 0xffff)
    }

    /// Roll the window forward by one byte: drop `old` (the byte leaving the
    /// window) and append `new` (the byte entering it). `len` is unchanged.
    fn roll(&mut self, old: u8, new: u8) {
        self.a = self.a.wrapping_add(new as u32).wrapping_sub(old as u32) & 0xffff;
        self.b = self
            .b
            .wrapping_add(self.a)
            .wrapping_sub(self.len * old as u32)
            & 0xffff;
    }
}

fn weak_checksum(data: &[u8]) -> u32 {
    RollingChecksum::new(data).digest()
}

fn strong_hash(data: &[u8]) -> [u8; 32] {
    *blake3::hash(data).as_bytes()
}

/// Generate a delta signature of the **basis** file (the current destination).
/// Returns an empty signature (no blocks) for a missing/zero-byte basis, which
/// makes `create_delta` emit the whole source as literals (a correct full copy).
pub fn generate_signature_librsync(path: &Path) -> Result<FileSignature> {
    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(_) => {
            return Ok(FileSignature {
                block_size: DEFAULT_BLOCK_SIZE,
                blocks: vec![],
                total_size: 0,
            })
        }
    };
    generate_signature_from_bytes(&data, DEFAULT_BLOCK_SIZE)
}

fn generate_signature_from_bytes(data: &[u8], block_size: u32) -> Result<FileSignature> {
    let bs = block_size as usize;
    let mut blocks = Vec::new();
    let mut offset = 0usize;
    while offset < data.len() {
        let end = (offset + bs).min(data.len());
        let block = &data[offset..end];
        blocks.push(BlockSignature {
            offset: offset as u64,
            size: block.len() as u32,
            blake3: strong_hash(block),
            weak: weak_checksum(block),
        });
        offset = end;
    }
    Ok(FileSignature {
        block_size,
        blocks,
        total_size: data.len() as u64,
    })
}

/// Index a signature by weak checksum → list of (block) signatures, so the
/// rolling scan can confirm a candidate with the strong hash in O(1) average.
fn index_signature(sig: &FileSignature) -> HashMap<u32, Vec<&BlockSignature>> {
    let mut map: HashMap<u32, Vec<&BlockSignature>> = HashMap::new();
    for b in &sig.blocks {
        map.entry(b.weak).or_default().push(b);
    }
    map
}

/// Compute the delta of `source` against the basis `sig`. Unchanged regions
/// become `Copy` (zero bytes moved); changed regions become `Literal`.
pub fn create_delta_librsync(source: &Path, sig: &FileSignature) -> Result<Vec<DeltaChunk>> {
    let data = std::fs::read(source)
        .map_err(|e| UniFlowError::Transport(format!("delta: cannot read source: {e}")))?;

    let mut instructions: Vec<DeltaInstruction> = Vec::new();

    // No basis blocks → the whole source is new. Emit literals in bounded runs.
    if sig.blocks.is_empty() {
        push_literals(&mut instructions, &data);
        return Ok(vec![DeltaChunk { instructions, source_offset: 0 }]);
    }

    let bs = sig.block_size as usize;
    let index = index_signature(sig);

    let mut literal_start = 0usize; // start of the pending literal run
    let mut pos = 0usize;

    if data.len() >= bs {
        let mut roll = RollingChecksum::new(&data[0..bs]);
        loop {
            let mut matched: Option<&BlockSignature> = None;
            if let Some(cands) = index.get(&roll.digest()) {
                let window = &data[pos..pos + bs];
                let strong = strong_hash(window);
                matched = cands
                    .iter()
                    .copied()
                    .find(|b| b.size as usize == bs && b.blake3 == strong);
            }

            if let Some(block) = matched {
                // Flush the literal run preceding this match, then a Copy.
                if literal_start < pos {
                    push_literals(&mut instructions, &data[literal_start..pos]);
                }
                instructions.push(DeltaInstruction::Copy {
                    old_offset: block.offset,
                    size: block.size,
                });
                pos += bs;
                literal_start = pos;
                if pos + bs > data.len() {
                    break;
                }
                roll = RollingChecksum::new(&data[pos..pos + bs]);
            } else {
                // Advance one byte, rolling the window.
                if pos + bs >= data.len() {
                    break;
                }
                let old = data[pos];
                let new = data[pos + bs];
                roll.roll(old, new);
                pos += 1;
            }
        }
    }

    // Trailing bytes (shorter than a block, or the final unmatched tail) → literal.
    if literal_start < data.len() {
        push_literals(&mut instructions, &data[literal_start..]);
    }

    Ok(vec![DeltaChunk { instructions, source_offset: 0 }])
}

/// Split `bytes` into `MAX_LITERAL_RUN`-bounded `Literal` instructions.
fn push_literals(out: &mut Vec<DeltaInstruction>, bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    for piece in bytes.chunks(MAX_LITERAL_RUN) {
        out.push(DeltaInstruction::Literal { data: piece.to_vec() });
    }
}

/// Apply a delta to `dest`, reconstructing from the old basis (current `dest`
/// contents) + the delta, and publishing the result **atomically**.
/// Returns the number of bytes in the reconstructed file.
///
/// `resume_from` is advisory: if the destination already byte-matches the
/// reconstruction we skip the rewrite (idempotent), otherwise we always produce
/// the full, byte-exact result via temp-file + fsync + rename (never partial).
pub fn apply_delta_librsync(dest: &Path, delta: &[DeltaChunk], resume_from: u64) -> Result<u64> {
    // Capture the basis BEFORE we touch the destination (Copy reads from it).
    let basis = std::fs::read(dest).unwrap_or_default();

    let mut output: Vec<u8> = Vec::new();
    for chunk in delta {
        for instr in &chunk.instructions {
            match instr {
                DeltaInstruction::Copy { old_offset, size } => {
                    let start = *old_offset as usize;
                    let end = start
                        .checked_add(*size as usize)
                        .ok_or_else(|| UniFlowError::Transport("delta: copy size overflow".into()))?;
                    if end > basis.len() {
                        return Err(UniFlowError::Transport(format!(
                            "delta: copy [{start}..{end}] out of basis bounds ({})",
                            basis.len()
                        )));
                    }
                    output.extend_from_slice(&basis[start..end]);
                }
                DeltaInstruction::Literal { data } => output.extend_from_slice(data),
            }
        }
    }

    let total = output.len() as u64;

    // Resume short-circuit: destination already correct.
    if resume_from >= total && basis == output {
        return Ok(total);
    }

    // Atomic publish: write to a temp sibling, fsync, then rename over dest.
    let tmp = dest.with_extension("uniflow_delta_tmp");
    {
        let mut f = std::fs::File::create(&tmp)
            .map_err(|e| UniFlowError::Transport(format!("delta: temp create failed: {e}")))?;
        f.write_all(&output)
            .map_err(|e| UniFlowError::Transport(format!("delta: temp write failed: {e}")))?;
        f.sync_all()
            .map_err(|e| UniFlowError::Transport(format!("delta: fsync failed: {e}")))?;
    }
    std::fs::rename(&tmp, dest)
        .map_err(|e| UniFlowError::Transport(format!("delta: atomic rename failed: {e}")))?;

    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delta_literal_bytes(delta: &[DeltaChunk]) -> usize {
        delta
            .iter()
            .flat_map(|c| &c.instructions)
            .map(|i| match i {
                DeltaInstruction::Literal { data } => data.len(),
                DeltaInstruction::Copy { .. } => 0,
            })
            .sum()
    }

    #[test]
    fn rolling_checksum_matches_full_recompute_after_roll() {
        let data = b"the quick brown fox jumps over the lazy dog";
        let bs = 8usize;
        let mut roll = RollingChecksum::new(&data[0..bs]);
        for pos in 0..(data.len() - bs) {
            assert_eq!(roll.digest(), weak_checksum(&data[pos..pos + bs]));
            roll.roll(data[pos], data[pos + bs]);
        }
        assert_eq!(
            roll.digest(),
            weak_checksum(&data[data.len() - bs..]),
            "rolled checksum must equal a fresh compute of the final window"
        );
    }

    #[test]
    fn small_edit_moves_far_fewer_bytes_than_full_copy() {
        // Basis = 64 KiB of structured data; source = basis with a 16-byte edit.
        let mut basis = vec![0u8; 64 * 1024];
        for (i, b) in basis.iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        let mut source = basis.clone();
        for b in source.iter_mut().skip(30_000).take(16) {
            *b = 0xAA;
        }

        let sig = generate_signature_from_bytes(&basis, DEFAULT_BLOCK_SIZE).unwrap();
        // Reuse create_delta via a temp source file.
        let dir = std::env::temp_dir().join("uniflow_sandbox").join("delta_test");
        std::fs::create_dir_all(&dir).unwrap();
        let src_path = dir.join("src_small_edit.bin");
        std::fs::write(&src_path, &source).unwrap();

        let delta = create_delta_librsync(&src_path, &sig).unwrap();
        let literal = delta_literal_bytes(&delta);

        // Only the changed block(s) should be literal; far less than a full copy.
        assert!(
            literal < source.len() / 4,
            "expected delta to move <25% of the file, moved {literal} of {}",
            source.len()
        );
        let _ = std::fs::remove_file(&src_path);
    }

    #[test]
    fn apply_reconstructs_byte_exact() {
        let dir = std::env::temp_dir().join("uniflow_sandbox").join("delta_test");
        std::fs::create_dir_all(&dir).unwrap();
        let basis_path = dir.join("apply_basis.bin");
        let src_path = dir.join("apply_src.bin");

        let mut basis = vec![0u8; 20_000];
        for (i, b) in basis.iter_mut().enumerate() {
            *b = (i % 97) as u8;
        }
        let mut source = basis.clone();
        source.extend_from_slice(b"APPENDED-NEW-TAIL-DATA");
        for b in source.iter_mut().skip(5000).take(8) {
            *b = 0xFF;
        }

        std::fs::write(&basis_path, &basis).unwrap();
        std::fs::write(&src_path, &source).unwrap();

        let sig = generate_signature_librsync(&basis_path).unwrap();
        let delta = create_delta_librsync(&src_path, &sig).unwrap();
        let written = apply_delta_librsync(&basis_path, &delta, 0).unwrap();

        let result = std::fs::read(&basis_path).unwrap();
        assert_eq!(written as usize, source.len());
        assert_eq!(result, source, "reconstruction must be byte-exact");

        let _ = std::fs::remove_file(&basis_path);
        let _ = std::fs::remove_file(&src_path);
    }
}
