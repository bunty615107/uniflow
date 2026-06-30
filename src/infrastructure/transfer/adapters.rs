//! Heterogeneous endpoint adapters (Deliverable 3).
//!
//! The parallel core reads from a [`ChunkSource`] and writes to a [`ChunkSink`].
//! These two traits ARE the protocol/transport boundary a future Flutter/Swift/Kotlin
//! client would drive (see `docs/client-contract.md`): random-access, offset-addressed,
//! integrity-checked chunks. Local FS adapters are provided here; cloud (rclone) and
//! P2P (iroh/QUIC) adapters implement the same two traits.
//!
//! Positioned reads/writes (`pread`/`pwrite`) let many workers hit distinct,
//! non-overlapping regions of one file concurrently without a shared cursor — the
//! key to saturating an NVMe/NIC in parallel. Large source files are memory-mapped
//! when the OS supports it, with a buffered-read fallback (graceful degradation).

use crate::error::{Result, UniFlowError};
use std::fs::{File, OpenOptions};
use std::path::Path;
use std::sync::Arc;

#[cfg(unix)]
use std::os::unix::fs::FileExt;
#[cfg(windows)]
use std::os::windows::fs::FileExt;

/// Files at/above this size are memory-mapped for reads when possible.
const MMAP_THRESHOLD: u64 = 8 * 1024 * 1024;

/// Cross-platform positioned read. Does not move any file cursor (safe to call
/// concurrently from many threads on the same handle).
fn pread(f: &File, mut offset: u64, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        let n = {
            #[cfg(unix)]
            {
                f.read_at(&mut buf[filled..], offset)?
            }
            #[cfg(windows)]
            {
                f.seek_read(&mut buf[filled..], offset)?
            }
        };
        if n == 0 {
            break; // EOF
        }
        filled += n;
        offset += n as u64;
    }
    Ok(filled)
}

/// Cross-platform positioned write (concurrent-safe at distinct offsets).
fn pwrite(f: &File, mut offset: u64, buf: &[u8]) -> std::io::Result<()> {
    let mut written = 0;
    while written < buf.len() {
        let n = {
            #[cfg(unix)]
            {
                f.write_at(&buf[written..], offset)?
            }
            #[cfg(windows)]
            {
                f.seek_write(&buf[written..], offset)?
            }
        };
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "pwrite wrote 0 bytes",
            ));
        }
        written += n;
        offset += n as u64;
    }
    Ok(())
}

/// Random-access source of bytes (the "read" side of the transport boundary).
pub trait ChunkSource: Send + Sync {
    fn len(&self) -> u64;
    /// Returns true when the source holds zero bytes.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Fill `buf` from `offset`, returning bytes read (< buf.len() only at EOF).
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize>;
}

/// Random-access sink for bytes (the "write" side of the transport boundary).
pub trait ChunkSink: Send + Sync {
    fn write_at(&self, offset: u64, data: &[u8]) -> Result<()>;
    /// Durably flush all writes (called once before atomic publish).
    fn sync(&self) -> Result<()>;
}

/// Local filesystem source: mmap for large files, positioned reads otherwise.
pub struct LocalFileSource {
    file: Arc<File>,
    len: u64,
    mmap: Option<memmap2::Mmap>,
}

impl LocalFileSource {
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        // mmap large files for zero-copy reads; fall back silently on failure.
        let mmap = if len >= MMAP_THRESHOLD {
            // SAFETY: the source file is opened read-only for the duration of the
            // transfer; we never mutate it. A concurrent external truncation is the
            // only hazard and would surface as a read error, not UB in our access.
            unsafe { memmap2::Mmap::map(&file).ok() }
        } else {
            None
        };
        Ok(Self { file: Arc::new(file), len, mmap })
    }
}

impl ChunkSource for LocalFileSource {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        if let Some(m) = &self.mmap {
            let start = offset as usize;
            if start >= m.len() {
                return Ok(0);
            }
            let end = (start + buf.len()).min(m.len());
            let n = end - start;
            buf[..n].copy_from_slice(&m[start..end]);
            Ok(n)
        } else {
            Ok(pread(&self.file, offset, buf)?)
        }
    }
}

/// Local filesystem sink writing to a pre-sized file via positioned writes.
pub struct LocalFileSink {
    file: Arc<File>,
}

impl LocalFileSink {
    /// Create/truncate `path` and pre-allocate `total_len` so workers can write any
    /// offset concurrently. Used on the destination's temp file (atomic publish).
    pub fn create(path: &Path, total_len: u64) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        file.set_len(total_len)?;
        Ok(Self { file: Arc::new(file) })
    }

    /// Reopen an existing temp file for resume (do NOT truncate).
    pub fn open_existing(path: &Path, total_len: u64) -> Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        if file.metadata()?.len() != total_len {
            file.set_len(total_len)?;
        }
        Ok(Self { file: Arc::new(file) })
    }
}

impl ChunkSink for LocalFileSink {
    fn write_at(&self, offset: u64, data: &[u8]) -> Result<()> {
        pwrite(&self.file, offset, data).map_err(UniFlowError::Io)
    }
    fn sync(&self) -> Result<()> {
        self.file.sync_all()?;
        Ok(())
    }
}
