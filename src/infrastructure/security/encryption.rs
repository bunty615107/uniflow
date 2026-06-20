//! Client-side (end-to-end) encryption for zero-knowledge and at-rest protection.
//! Uses AES-256-GCM and ChaCha20-Poly1305 (Section 13 / RustCrypto or ring style).
//! Streaming support for large files, deltas, and multipart chunks.

use chacha20poly1305::{aead::{Aead, KeyInit}, ChaCha20Poly1305, Nonce};
use aes_gcm::{Aes256Gcm, Key};
use zeroize::{Zeroize, Zeroizing};
// Use rand's OsRng + RngCore for cryptographically secure nonce (fix for previous rand::random which was not OsRng + no dep).
// See Cargo.toml: rand = "0.8" added with note. (aead::OsRng alias also works but we standardize on rand here for explicitness)
use rand::{rngs::OsRng, RngCore};
use crate::error::{Result, UniFlowError};
use std::io::{Read, Write};

pub struct ClientSideEncryption {
    // In real use, key comes from CredentialVault (derived or hardware-backed).
    // Never stored long-term in daemon for zero-knowledge.
    // Full zeroize: using Zeroizing<[u8;32]> ensures key material is zeroed on drop (even without explicit Drop).
    key: Zeroizing<[u8; 32]>, // 256-bit
}

impl ClientSideEncryption {
    pub fn new(key: [u8; 32]) -> Self {
        Self { key: Zeroizing::new(key) }
    }

    /// Encrypt data with ChaCha20 (preferred for mobile/P2P per some profiles) or AES.
    /// Returns (ciphertext, nonce) or error.
    pub fn encrypt(&self, plaintext: &[u8], use_chacha: bool) -> Result<(Vec<u8>, [u8; 12])> {
        // Fixed: proper OsRng for nonce (was rand::random which is not explicitly secure Os + caused dep issue).
        // Nonce is public but must be unique/unguessable per key use (never reuse).
        let mut nonce_bytes = [0u8; 12];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        if use_chacha {
            let cipher = ChaCha20Poly1305::new_from_slice(&*self.key)
                .map_err(|_| UniFlowError::Config("bad key for chacha".into()))?;
            let ct = cipher.encrypt(nonce, plaintext)
                .map_err(|_| UniFlowError::Internal("chacha encrypt failed".into()))?;
            Ok((ct, nonce_bytes))
        } else {
            let key = Key::<Aes256Gcm>::from_slice(&*self.key);
            let cipher = Aes256Gcm::new(key);
            let ct = cipher.encrypt(nonce, plaintext)
                .map_err(|_| UniFlowError::Internal("aes encrypt failed".into()))?;
            Ok((ct, nonce_bytes))
        }
    }

    /// Decrypt. Caller must provide the nonce stored with the ciphertext.
    pub fn decrypt(&self, ciphertext: &[u8], nonce_bytes: &[u8; 12], use_chacha: bool) -> Result<Vec<u8>> {
        let nonce = Nonce::from_slice(nonce_bytes);

        if use_chacha {
            let cipher = ChaCha20Poly1305::new_from_slice(&*self.key)
                .map_err(|_| UniFlowError::Config("bad key".into()))?;
            cipher.decrypt(nonce, ciphertext)
                .map_err(|_| UniFlowError::Internal("chacha decrypt failed".into()))
        } else {
            let key = Key::<Aes256Gcm>::from_slice(&*self.key);
            let cipher = Aes256Gcm::new(key);
            cipher.decrypt(nonce, ciphertext)
                .map_err(|_| UniFlowError::Internal("aes decrypt failed".into()))
        }
    }

    /// Streaming encrypt for large payloads (integrates with delta chunks, multipart).
    /// Simple chunked version; real impl would use aead::stream.
    pub fn encrypt_stream<R: Read, W: Write>(&self, mut reader: R, mut writer: W, use_chacha: bool) -> Result<()> {
        let mut buf = vec![0u8; 64 * 1024]; // 64KiB chunks
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 { break; }
            let (ct, nonce) = self.encrypt(&buf[..n], use_chacha)?;
            // In real: prefix with nonce or store separately per chunk + MAC
            writer.write_all(&nonce)?;
            writer.write_all(&(ct.len() as u32).to_le_bytes())?;
            writer.write_all(&ct)?;
        }
        Ok(())
    }
}

// Drop impl removed: full zeroize now provided by Zeroizing<[u8;32]> wrapper (zeros on drop automatically + securely).
// This is stronger than manual Zeroize in Drop (no reliance on explicit drop or panic safety).
// Additional: callers should prefer short-lived ClientSideEncryption instances; sensitive plaintexts passed in should be zeroized by caller where possible (e.g. via Zeroizing<Vec<u8>>).