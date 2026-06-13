//! Credential encryption at rest using ChaCha20-Poly1305.
//!
//! Stored blob layout: `nonce(12 bytes) || ciphertext`. The 32-byte master key
//! comes from the `MASTER_KEY` environment variable.

use anyhow::{anyhow, Context, Result};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use rand::RngCore;

const NONCE_LEN: usize = 12;

/// Stateless cipher wrapper around the master key.
#[derive(Clone)]
pub struct Cipher {
    key: [u8; 32],
}

impl Cipher {
    pub fn new(key: [u8; 32]) -> Self {
        Self { key }
    }

    /// Encrypt a plaintext secret, returning `nonce || ciphertext`.
    pub fn encrypt(&self, plaintext: &str) -> Result<Vec<u8>> {
        let cipher = ChaCha20Poly1305::new_from_slice(&self.key)
            .map_err(|e| anyhow!("invalid master key length: {e}"))?;

        let mut nonce_bytes = [0u8; NONCE_LEN];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = cipher
            .encrypt(nonce, plaintext.as_bytes())
            .map_err(|e| anyhow!("encryption failed: {e}"))?;

        let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&ciphertext);
        Ok(out)
    }

    /// Decrypt a `nonce || ciphertext` blob back into the plaintext secret.
    pub fn decrypt(&self, blob: &[u8]) -> Result<String> {
        if blob.len() < NONCE_LEN {
            return Err(anyhow!("ciphertext too short"));
        }
        let cipher = ChaCha20Poly1305::new_from_slice(&self.key)
            .map_err(|e| anyhow!("invalid master key length: {e}"))?;

        let (nonce_bytes, ciphertext) = blob.split_at(NONCE_LEN);
        let nonce = Nonce::from_slice(nonce_bytes);

        let plaintext = cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| anyhow!("decryption failed: {e}"))?;
        String::from_utf8(plaintext).context("decrypted secret is not valid UTF-8")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let cipher = Cipher::new([7u8; 32]);
        let blob = cipher.encrypt("sk-secret-value").unwrap();
        // Nonce is prepended, so the blob differs from plaintext.
        assert!(blob.len() > NONCE_LEN);
        assert_eq!(cipher.decrypt(&blob).unwrap(), "sk-secret-value");
    }

    #[test]
    fn distinct_nonces_produce_distinct_ciphertexts() {
        let cipher = Cipher::new([3u8; 32]);
        let a = cipher.encrypt("same").unwrap();
        let b = cipher.encrypt("same").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn wrong_key_fails() {
        let blob = Cipher::new([1u8; 32]).encrypt("secret").unwrap();
        assert!(Cipher::new([2u8; 32]).decrypt(&blob).is_err());
    }
}
