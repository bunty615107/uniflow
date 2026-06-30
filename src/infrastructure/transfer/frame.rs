//! Self-describing wire frame for networked transports (P2P now; future cloud-stream).
//!
//! This is the EXACT framing documented in `docs/client-contract.md`. Every chunk is
//! placed by the receiver using **only** the frame header — no shared cursor — so
//! frames can be sent over multiple streams, resumed, and verified independently.
//!
//! Layout (little-endian):
//! ```text
//! | offset u64 | plain_len u32 | wire_len u32 | flags u8 | [nonce 12 iff encrypted] | blake3 32 | payload (wire_len) |
//! ```
//! * `flags` bit0 = compressed (zstd), bit1 = encrypted (AEAD).
//! * `nonce` is present **iff** the encrypted bit is set (matches the contract doc).
//! * `blake3` is the hash of the **plaintext** chunk — the integrity anchor.
//! * `offset == OFFSET_TRAILER` marks the end-of-stream trailer whose `blake3` field
//!   carries the end-to-end BLAKE3 root of the whole file (`payload` empty).
//!
//! The codec is generic over Tokio's async read/write traits, so it works over an
//! iroh/QUIC stream, an in-memory duplex (for deterministic CI tests), or any future
//! byte transport — keeping the engine transport-blind exactly as the blueprint asks.

use crate::error::{Result, UniFlowError};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// bit0: payload was zstd-compressed before encryption.
pub const FLAG_COMPRESSED: u8 = 0b0000_0001;
/// bit1: payload was AEAD-encrypted (nonce field present).
pub const FLAG_ENCRYPTED: u8 = 0b0000_0010;

/// Sentinel `offset` marking the end-of-stream trailer frame.
pub const OFFSET_TRAILER: u64 = u64::MAX;

/// Defensive upper bound on a single frame payload, so a malicious or buggy peer
/// cannot force an unbounded allocation. 256 MiB comfortably exceeds any chunk size
/// the planner produces (it clamps chunk_size to ≤ 64 MiB).
pub const MAX_PAYLOAD: u32 = 256 * 1024 * 1024;

/// The fixed prefix size (offset + plain_len + wire_len + flags) read before the
/// variable-size nonce/hash/payload tail.
const FIXED_PREFIX: usize = 8 + 4 + 4 + 1;

#[derive(Clone, Debug)]
pub struct FrameHeader {
    pub offset: u64,
    pub plain_len: u32,
    pub wire_len: u32,
    pub flags: u8,
    pub nonce: Option<[u8; 12]>,
    pub blake3: [u8; 32],
}

impl FrameHeader {
    /// A data frame placing `payload` at `offset`.
    pub fn data(offset: u64, plain_len: u32, wire_len: u32, flags: u8, nonce: Option<[u8; 12]>, blake3: [u8; 32]) -> Self {
        Self { offset, plain_len, wire_len, flags, nonce, blake3 }
    }

    /// The end-of-stream trailer carrying the whole-file BLAKE3 root.
    pub fn trailer(root: [u8; 32]) -> Self {
        Self { offset: OFFSET_TRAILER, plain_len: 0, wire_len: 0, flags: 0, nonce: None, blake3: root }
    }

    pub fn is_trailer(&self) -> bool {
        self.offset == OFFSET_TRAILER
    }
    pub fn is_compressed(&self) -> bool {
        self.flags & FLAG_COMPRESSED != 0
    }
    pub fn is_encrypted(&self) -> bool {
        self.flags & FLAG_ENCRYPTED != 0
    }
}

/// Serialize a complete frame (header + payload) to an async stream.
pub async fn write_frame<W: AsyncWrite + Unpin>(
    w: &mut W,
    header: &FrameHeader,
    payload: &[u8],
) -> Result<()> {
    debug_assert_eq!(payload.len() as u32, header.wire_len, "payload len must equal wire_len");
    let mut head = Vec::with_capacity(FIXED_PREFIX + 12 + 32);
    head.extend_from_slice(&header.offset.to_le_bytes());
    head.extend_from_slice(&header.plain_len.to_le_bytes());
    head.extend_from_slice(&header.wire_len.to_le_bytes());
    head.push(header.flags);
    if header.flags & FLAG_ENCRYPTED != 0 {
        let nonce = header
            .nonce
            .ok_or_else(|| UniFlowError::Internal("encrypted frame missing nonce".into()))?;
        head.extend_from_slice(&nonce);
    }
    head.extend_from_slice(&header.blake3);
    w.write_all(&head).await.map_err(UniFlowError::Io)?;
    if !payload.is_empty() {
        w.write_all(payload).await.map_err(UniFlowError::Io)?;
    }
    Ok(())
}

/// Read one frame. Returns `Ok(None)` on a clean end-of-stream *before any header
/// byte*; any EOF mid-frame is a hard error (truncated/corrupt stream).
pub async fn read_frame<R: AsyncRead + Unpin>(
    r: &mut R,
) -> Result<Option<(FrameHeader, Vec<u8>)>> {
    let mut prefix = [0u8; FIXED_PREFIX];
    if !read_exact_or_eof(r, &mut prefix).await? {
        return Ok(None);
    }
    let offset = u64::from_le_bytes(prefix[0..8].try_into().unwrap());
    let plain_len = u32::from_le_bytes(prefix[8..12].try_into().unwrap());
    let wire_len = u32::from_le_bytes(prefix[12..16].try_into().unwrap());
    let flags = prefix[16];

    if wire_len > MAX_PAYLOAD {
        return Err(UniFlowError::Transport(format!(
            "frame wire_len {wire_len} exceeds MAX_PAYLOAD {MAX_PAYLOAD}"
        )));
    }

    let nonce = if flags & FLAG_ENCRYPTED != 0 {
        let mut n = [0u8; 12];
        r.read_exact(&mut n).await.map_err(UniFlowError::Io)?;
        Some(n)
    } else {
        None
    };

    let mut blake3 = [0u8; 32];
    r.read_exact(&mut blake3).await.map_err(UniFlowError::Io)?;

    let mut payload = vec![0u8; wire_len as usize];
    if wire_len > 0 {
        r.read_exact(&mut payload).await.map_err(UniFlowError::Io)?;
    }

    Ok(Some((
        FrameHeader { offset, plain_len, wire_len, flags, nonce, blake3 },
        payload,
    )))
}

/// Fill `buf` fully. Returns `Ok(false)` if EOF arrives with **no** bytes read (clean
/// end of stream), `Ok(true)` on a full read, and an error if EOF arrives partway.
async fn read_exact_or_eof<R: AsyncRead + Unpin>(r: &mut R, buf: &mut [u8]) -> Result<bool> {
    let mut filled = 0;
    while filled < buf.len() {
        let n = r.read(&mut buf[filled..]).await.map_err(UniFlowError::Io)?;
        if n == 0 {
            if filled == 0 {
                return Ok(false);
            }
            return Err(UniFlowError::Transport(
                "unexpected EOF in the middle of a frame header".into(),
            ));
        }
        filled += n;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn plaintext_frame_roundtrips_byte_exact() {
        let (mut a, mut b) = tokio::io::duplex(64 * 1024);
        let payload = b"hello uniflow frame".to_vec();
        let hash = *blake3::hash(&payload).as_bytes();
        let header = FrameHeader::data(4096, payload.len() as u32, payload.len() as u32, 0, None, hash);

        let writer = {
            let header = header.clone();
            let payload = payload.clone();
            tokio::spawn(async move {
                write_frame(&mut a, &header, &payload).await.unwrap();
                // Drop `a` to signal EOF so a subsequent read returns None.
            })
        };

        let (got, body) = read_frame(&mut b).await.unwrap().expect("a frame");
        writer.await.unwrap();

        assert_eq!(got.offset, 4096);
        assert_eq!(got.plain_len as usize, payload.len());
        assert!(!got.is_encrypted() && !got.is_compressed());
        assert_eq!(got.blake3, hash);
        assert_eq!(body, payload);

        // After the single frame and the writer dropping, EOF reads as None.
        assert!(read_frame(&mut b).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn encrypted_frame_carries_nonce_and_trailer_marks_eos() {
        let (mut a, mut b) = tokio::io::duplex(64 * 1024);
        let nonce = [7u8; 12];
        let payload = vec![0xABu8; 100];
        let hash = [9u8; 32];
        let data = FrameHeader::data(
            0,
            80,
            payload.len() as u32,
            FLAG_COMPRESSED | FLAG_ENCRYPTED,
            Some(nonce),
            hash,
        );
        let trailer = FrameHeader::trailer([0x5Au8; 32]);

        let writer = {
            let data = data.clone();
            let payload = payload.clone();
            let trailer = trailer.clone();
            tokio::spawn(async move {
                write_frame(&mut a, &data, &payload).await.unwrap();
                write_frame(&mut a, &trailer, &[]).await.unwrap();
            })
        };

        let (h1, p1) = read_frame(&mut b).await.unwrap().unwrap();
        assert!(h1.is_compressed() && h1.is_encrypted());
        assert_eq!(h1.nonce, Some(nonce));
        assert_eq!(p1, payload);

        let (h2, p2) = read_frame(&mut b).await.unwrap().unwrap();
        assert!(h2.is_trailer());
        assert_eq!(h2.blake3, [0x5Au8; 32]);
        assert!(p2.is_empty());

        writer.await.unwrap();
    }
}
