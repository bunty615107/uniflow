//! UniFlow P2P session protocol (Module 03).
//!
//! This is the real, integrity-checked transfer protocol a peer speaks over a QUIC
//! bi-directional stream (see `docs/client-contract.md`). It is deliberately generic
//! over Tokio's async read/write traits, so the exact same code path is exercised by:
//!   * an in-memory `tokio::io::duplex` in CI (deterministic, no sockets), and
//!   * a real iroh/QUIC stream in the loopback integration test and in production.
//!
//! ## Wire handshake (after the caller has exchanged the op byte)
//! ```text
//! sender  → receiver:  total_len u64, crypto u8            (0=none, 1=AES-GCM, 2=ChaCha20)
//! receiver → sender:   resume u64                          (highest contiguous byte it holds)
//! sender  → receiver:  data frames for [resume, total_len) (see frame.rs)
//! sender  → receiver:  trailer frame (whole-file BLAKE3 root)
//! receiver → sender:   status u8 (0=ok), root [u8;32]      (its own recomputed root)
//! ```
//!
//! ## Guarantees (the non-negotiables from the blueprint)
//! * **Integrity** — every chunk carries the BLAKE3 of its plaintext and is verified
//!   after decrypt+decompress; the whole file is re-verified against the trailer root.
//! * **Atomicity** — the receiver writes to a `*.uniflow-tmp` file and only renames it
//!   into place after the end-to-end root matches; the destination is never partial.
//! * **Resume** — the receiver advertises the highest contiguous byte it already holds
//!   (from a checkpoint sidecar) and the sender resends only beyond it.
//! * **Confidentiality** — when a shared key is configured the payload is AEAD-encrypted
//!   end-to-end; an intermediary sees only ciphertext (zero-knowledge preserved).
//! * **Graceful degradation** — compression and encryption are optional per the plan /
//!   key availability; a peer that can do neither still completes with integrity intact.

use crate::error::{Result, UniFlowError};
use crate::infrastructure::security::ClientSideEncryption;
use crate::infrastructure::transfer::adapters::{ChunkSink, ChunkSource, LocalFileSink};
use crate::infrastructure::transfer::frame::{
    read_frame, write_frame, FrameHeader, FLAG_COMPRESSED, FLAG_ENCRYPTED,
};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tracing::info;

/// Protocol magic + version, written by the initiator before the op byte.
pub const MAGIC: [u8; 4] = *b"UFP1";
/// Op: the initiator pushes a file to the peer (peer receives).
pub const OP_PUSH: u8 = 1;
/// Op: the initiator pulls a file from the peer (peer sends).
pub const OP_PULL: u8 = 2;

const CRYPTO_NONE: u8 = 0;
const CRYPTO_AES: u8 = 1;
const CRYPTO_CHACHA: u8 = 2;

/// Persist the resume checkpoint at most every this many received bytes (bounds sidecar IO).
const CKPT_FLUSH_EVERY: u64 = 8 * 1024 * 1024;
/// Block size used for the streaming whole-file root hash passes.
const ROOT_HASH_BLOCK: usize = 1024 * 1024;

/// A shared AEAD configuration for the wire (both peers must agree on key + cipher).
pub struct WireCrypto {
    pub enc: ClientSideEncryption,
    pub use_chacha: bool,
}

impl WireCrypto {
    fn crypto_byte(&self) -> u8 {
        if self.use_chacha {
            CRYPTO_CHACHA
        } else {
            CRYPTO_AES
        }
    }
}

/// Write the session preamble: MAGIC + op byte. The initiator calls this; the acceptor
/// reads it with [`read_op`] to decide which role to play.
pub async fn write_op<W: AsyncWrite + Unpin>(send: &mut W, op: u8) -> Result<()> {
    let mut buf = [0u8; 5];
    buf[..4].copy_from_slice(&MAGIC);
    buf[4] = op;
    send.write_all(&buf).await.map_err(UniFlowError::Io)?;
    Ok(())
}

/// Read the session preamble and return the op byte (validates MAGIC).
pub async fn read_op<R: AsyncRead + Unpin>(recv: &mut R) -> Result<u8> {
    let mut buf = [0u8; 5];
    recv.read_exact(&mut buf).await.map_err(UniFlowError::Io)?;
    if buf[..4] != MAGIC {
        return Err(UniFlowError::Transport("bad P2P session magic".into()));
    }
    Ok(buf[4])
}

/// SENDER role: stream `source` to the peer with per-chunk + end-to-end integrity.
/// Returns the (whole-file BLAKE3 root, bytes_in_file).
pub async fn run_sender<W, R>(
    send: &mut W,
    recv: &mut R,
    source: &dyn ChunkSource,
    chunk_size: u64,
    comp_level: Option<i32>,
    crypto: Option<&WireCrypto>,
) -> Result<([u8; 32], u64)>
where
    W: AsyncWrite + Unpin,
    R: AsyncRead + Unpin,
{
    let total_len = source.len();
    let crypto_byte = crypto.map(|c| c.crypto_byte()).unwrap_or(CRYPTO_NONE);

    // Handshake: announce size + cipher, then learn the receiver's resume point.
    let mut hello = [0u8; 9];
    hello[..8].copy_from_slice(&total_len.to_le_bytes());
    hello[8] = crypto_byte;
    send.write_all(&hello).await.map_err(UniFlowError::Io)?;

    let mut rb = [0u8; 8];
    recv.read_exact(&mut rb).await.map_err(UniFlowError::Io)?;
    let resume = u64::from_le_bytes(rb).min(total_len);

    let chunk_size = chunk_size.max(1);
    let mut buf = vec![0u8; chunk_size as usize];
    let mut offset = resume;
    while offset < total_len {
        let size = (total_len - offset).min(chunk_size) as usize;
        let n = source.read_at(offset, &mut buf[..size])?;
        if n == 0 {
            break; // defensive: never spin on a short source
        }
        let plain = &buf[..n];
        let blake3 = *blake3::hash(plain).as_bytes();

        // --- wire encode: [compress] → [encrypt] ---
        let mut flags = 0u8;
        let compressed = match comp_level {
            Some(level) => {
                flags |= FLAG_COMPRESSED;
                zstd::bulk::compress(plain, level).map_err(UniFlowError::Io)?
            }
            None => plain.to_vec(),
        };
        let (wire, nonce) = match crypto {
            Some(c) => {
                flags |= FLAG_ENCRYPTED;
                let (ct, nonce) = c.enc.encrypt(&compressed, c.use_chacha)?;
                (ct, Some(nonce))
            }
            None => (compressed, None),
        };

        let header = FrameHeader::data(offset, n as u32, wire.len() as u32, flags, nonce, blake3);
        write_frame(send, &header, &wire).await?;
        offset += n as u64;
    }

    // End-to-end root over the FULL source (independent of the resume point).
    let root = full_root_source(source, chunk_size)?;
    write_frame(send, &FrameHeader::trailer(root), &[]).await?;
    send.flush().await.map_err(UniFlowError::Io)?;

    // Acknowledgement: status byte + the receiver's recomputed root.
    let mut status = [0u8; 1];
    recv.read_exact(&mut status).await.map_err(UniFlowError::Io)?;
    let mut their_root = [0u8; 32];
    recv.read_exact(&mut their_root).await.map_err(UniFlowError::Io)?;
    if status[0] != 0 {
        return Err(UniFlowError::Transport(
            "peer receiver reported a transfer failure".into(),
        ));
    }
    if their_root != root {
        return Err(UniFlowError::Internal(
            "end-to-end BLAKE3 root the receiver acked does not match the source".into(),
        ));
    }
    Ok((root, total_len))
}

/// RECEIVER role: accept a framed file from the peer and publish it atomically at
/// `dst_path`. Returns the (whole-file BLAKE3 root, bytes_written). `key` must be
/// present iff the sender announces encryption.
pub async fn run_receiver<W, R>(
    send: &mut W,
    recv: &mut R,
    dst_path: &Path,
    key: Option<[u8; 32]>,
    job_checkpoint: Option<u64>,
) -> Result<([u8; 32], u64)>
where
    W: AsyncWrite + Unpin,
    R: AsyncRead + Unpin,
{
    // Handshake: read size + cipher choice.
    let mut hello = [0u8; 9];
    recv.read_exact(&mut hello).await.map_err(UniFlowError::Io)?;
    let total_len = u64::from_le_bytes(hello[..8].try_into().unwrap());
    let crypto_byte = hello[8];

    let crypto = match crypto_byte {
        CRYPTO_NONE => None,
        CRYPTO_AES | CRYPTO_CHACHA => {
            let key = key.ok_or_else(|| {
                UniFlowError::Config(
                    "peer requested an encrypted P2P transfer but no shared key is configured"
                        .into(),
                )
            })?;
            Some(WireCrypto {
                enc: ClientSideEncryption::new(key),
                use_chacha: crypto_byte == CRYPTO_CHACHA,
            })
        }
        other => {
            return Err(UniFlowError::Transport(format!(
                "unknown wire crypto id {other} from peer"
            )))
        }
    };

    let temp_path = dst_path.with_extension("uniflow-tmp");
    let ckpt_path = dst_path.with_extension("uniflow-ckpt");
    let resume = read_checkpoint(&ckpt_path)
        .max(job_checkpoint.unwrap_or(0))
        .min(total_len);

    let sink = if resume > 0 && temp_path.exists() {
        info!(resume, "p2p receiver resuming from checkpoint");
        LocalFileSink::open_existing(&temp_path, total_len)?
    } else {
        LocalFileSink::create(&temp_path, total_len)?
    };

    // Tell the sender where to resume from.
    send.write_all(&resume.to_le_bytes())
        .await
        .map_err(UniFlowError::Io)?;

    // Receive frames until the trailer.
    let mut contiguous = resume;
    let mut last_flushed = resume;
    let claimed_root: [u8; 32] = loop {
        let (header, payload) = read_frame(recv)
            .await?
            .ok_or_else(|| UniFlowError::Transport("p2p stream ended before trailer".into()))?;

        if header.is_trailer() {
            break header.blake3;
        }

        // --- wire decode: [decrypt] → [decompress] ---
        let decrypted = if header.is_encrypted() {
            let c = crypto.as_ref().ok_or_else(|| {
                UniFlowError::Transport("encrypted frame but no cipher negotiated".into())
            })?;
            let nonce = header
                .nonce
                .ok_or_else(|| UniFlowError::Transport("encrypted frame missing nonce".into()))?;
            c.enc.decrypt(&payload, &nonce, c.use_chacha)?
        } else {
            payload
        };
        let recovered = if header.is_compressed() {
            zstd::bulk::decompress(&decrypted, header.plain_len as usize)
                .map_err(UniFlowError::Io)?
        } else {
            decrypted
        };

        if recovered.len() as u32 != header.plain_len {
            return Err(UniFlowError::Transport(format!(
                "frame plain_len {} != recovered {} at offset {}",
                header.plain_len,
                recovered.len(),
                header.offset
            )));
        }
        // Per-chunk integrity across the full round trip.
        if *blake3::hash(&recovered).as_bytes() != header.blake3 {
            return Err(UniFlowError::Internal(format!(
                "per-chunk BLAKE3 mismatch at offset {}",
                header.offset
            )));
        }

        sink.write_at(header.offset, &recovered)?;

        // Single ordered stream ⇒ frames arrive contiguously from `resume`.
        if header.offset == contiguous {
            contiguous += recovered.len() as u64;
            if contiguous - last_flushed >= CKPT_FLUSH_EVERY {
                let _ = std::fs::write(&ckpt_path, contiguous.to_string());
                last_flushed = contiguous;
            }
        }
    };

    // Durably flush, verify end-to-end, then publish or fail loudly.
    sink.sync()?;
    let actual_root = full_root_path(&temp_path)?;
    if actual_root != claimed_root {
        let _ = std::fs::remove_file(&temp_path);
        // Tell the sender we failed before returning.
        let _ = ack(send, 1, &actual_root).await;
        return Err(UniFlowError::Internal(
            "end-to-end BLAKE3 verification failed; destination not published".into(),
        ));
    }

    atomic_publish(&temp_path, dst_path)?;
    let _ = std::fs::remove_file(&ckpt_path);
    ack(send, 0, &actual_root).await?;
    send.flush().await.map_err(UniFlowError::Io)?;

    Ok((actual_root, total_len))
}

async fn ack<W: AsyncWrite + Unpin>(send: &mut W, status: u8, root: &[u8; 32]) -> Result<()> {
    let mut buf = [0u8; 33];
    buf[0] = status;
    buf[1..].copy_from_slice(root);
    send.write_all(&buf).await.map_err(UniFlowError::Io)?;
    Ok(())
}

fn read_checkpoint(ckpt_path: &Path) -> u64 {
    std::fs::read_to_string(ckpt_path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

/// Streaming BLAKE3 root over a `ChunkSource` (the whole file, in order).
fn full_root_source(source: &dyn ChunkSource, chunk_size: u64) -> Result<[u8; 32]> {
    let total = source.len();
    let mut h = blake3::Hasher::new();
    let mut off = 0u64;
    let mut buf = vec![0u8; chunk_size.max(1) as usize];
    while off < total {
        let size = (total - off).min(chunk_size.max(1)) as usize;
        let n = source.read_at(off, &mut buf[..size])?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        off += n as u64;
    }
    Ok(*h.finalize().as_bytes())
}

/// Streaming BLAKE3 root over a file on disk (the receiver's whole temp file).
fn full_root_path(path: &Path) -> Result<[u8; 32]> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; ROOT_HASH_BLOCK];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(*h.finalize().as_bytes())
}

/// Atomic-on-completion publish: replace the destination in one rename.
fn atomic_publish(temp: &Path, dst: &Path) -> Result<()> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // On Windows rename fails if the target exists; remove first (small window,
    // mirrors the parallel core's behaviour).
    if dst.exists() {
        let _ = std::fs::remove_file(dst);
    }
    std::fs::rename(temp, dst).map_err(UniFlowError::Io)
}

/// Convenience for callers/tests: open `path` as a [`LocalFileSource`].
pub fn open_source(path: &Path) -> Result<crate::infrastructure::transfer::adapters::LocalFileSource> {
    crate::infrastructure::transfer::adapters::LocalFileSource::open(path)
}

/// Helper retained for symmetry / future daemon accept-loop wiring.
pub fn temp_and_ckpt(dst: &Path) -> (PathBuf, PathBuf) {
    (dst.with_extension("uniflow-tmp"), dst.with_extension("uniflow-ckpt"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::transfer::adapters::LocalFileSource;

    fn sandbox_case(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("uniflow_sandbox")
            .join(format!("p2p_{}_{}", name, uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Drive run_sender ↔ run_receiver over two in-memory duplex pipes (no sockets).
    /// This exercises the entire real protocol — handshake, framing, per-chunk +
    /// end-to-end integrity, compression, encryption, and atomic publish — in CI.
    async fn duplex_transfer(
        content: &[u8],
        chunk_size: u64,
        comp: Option<i32>,
        key: Option<[u8; 32]>,
        use_chacha: bool,
        resume_checkpoint: Option<u64>,
    ) -> Vec<u8> {
        let dir = sandbox_case("duplex");
        let src = dir.join("src.bin");
        let dst = dir.join("dst.bin");
        std::fs::write(&src, content).unwrap();

        // sender.send → receiver.recv  (pair s)
        let (mut s_tx, mut s_rx) = tokio::io::duplex(1 << 20);
        // receiver.send → sender.recv  (pair r)
        let (mut r_tx, mut r_rx) = tokio::io::duplex(1 << 20);

        let src2 = src.clone();
        let sender = tokio::spawn(async move {
            let source = LocalFileSource::open(&src2).unwrap();
            let crypto = key.map(|k| WireCrypto {
                enc: ClientSideEncryption::new(k),
                use_chacha,
            });
            run_sender(&mut s_tx, &mut r_rx, &source, chunk_size, comp, crypto.as_ref())
                .await
                .unwrap()
        });

        let dst2 = dst.clone();
        let receiver = tokio::spawn(async move {
            run_receiver(&mut r_tx, &mut s_rx, &dst2, key, resume_checkpoint)
                .await
                .unwrap()
        });

        let (s_out, r_out) = tokio::join!(sender, receiver);
        let (sroot, sbytes) = s_out.unwrap();
        let (rroot, rbytes) = r_out.unwrap();
        assert_eq!(sroot, rroot, "sender and receiver roots must agree");
        assert_eq!(sbytes, rbytes);
        std::fs::read(&dst).unwrap()
    }

    #[tokio::test]
    async fn plain_transfer_is_byte_exact() {
        let content: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let got = duplex_transfer(&content, 64 * 1024, None, None, false, None).await;
        assert_eq!(got, content);
    }

    #[tokio::test]
    async fn compressed_and_encrypted_transfer_is_lossless() {
        let content: Vec<u8> = (0..(512 * 1024 + 123)).map(|i| (i % 251) as u8).collect();
        let key = [42u8; 32];
        let got = duplex_transfer(&content, 64 * 1024, Some(6), Some(key), true, None).await;
        assert_eq!(got, content);
    }

    #[tokio::test]
    async fn empty_file_transfers() {
        let got = duplex_transfer(b"", 64 * 1024, None, None, false, None).await;
        assert!(got.is_empty());
    }

    #[tokio::test]
    async fn aes_path_roundtrips() {
        let content: Vec<u8> = (0..70_000u32).map(|i| (i % 251) as u8).collect();
        let key = [7u8; 32];
        let got = duplex_transfer(&content, 16 * 1024, None, Some(key), false, None).await;
        assert_eq!(got, content);
    }

    #[tokio::test]
    async fn receiver_requires_key_when_sender_encrypts() {
        // Sender encrypts, receiver has no key → receiver must error (no silent plaintext).
        let dir = sandbox_case("nokey");
        let src = dir.join("src.bin");
        let dst = dir.join("dst.bin");
        std::fs::write(&src, vec![1u8; 4096]).unwrap();

        let (mut s_tx, mut s_rx) = tokio::io::duplex(1 << 20);
        let (mut r_tx, mut r_rx) = tokio::io::duplex(1 << 20);

        let src2 = src.clone();
        let sender = tokio::spawn(async move {
            let source = LocalFileSource::open(&src2).unwrap();
            let crypto = WireCrypto {
                enc: ClientSideEncryption::new([9u8; 32]),
                use_chacha: true,
            };
            run_sender(&mut s_tx, &mut r_rx, &source, 4096, None, Some(&crypto)).await
        });
        let dst2 = dst.clone();
        let receiver =
            tokio::spawn(async move { run_receiver(&mut r_tx, &mut s_rx, &dst2, None, None).await });

        let (_s, r) = tokio::join!(sender, receiver);
        assert!(r.unwrap().is_err(), "receiver without key must reject encrypted transfer");
        assert!(!dst.exists(), "no destination should be published on failure");
    }
}
