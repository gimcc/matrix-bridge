//! Matrix attachment encryption (AES-256-CTR) per the Matrix spec.
//!
//! In encrypted rooms, media files must be encrypted client-side before
//! uploading. The decryption key and IV are shared inside the Megolm-encrypted
//! event content using the `file` object instead of a plain `url` field.
//!
//! Spec reference: Matrix Client-Server API § Sending encrypted attachments.

use aes::Aes256;
use base64::{Engine, engine::general_purpose::{STANDARD_NO_PAD, URL_SAFE_NO_PAD}};
use ctr::cipher::{KeyIvInit, StreamCipher};
use sha2::{Digest, Sha256};

type Aes256Ctr = ctr::Ctr64BE<Aes256>;

/// Metadata for an encrypted attachment, used to build the `file` JSON object
/// in Matrix message content.
///
/// Does NOT derive `Debug` to prevent accidental logging of key material.
#[derive(Clone)]
pub(super) struct EncryptedAttachment {
    /// mxc:// URI of the uploaded ciphertext.
    pub mxc_uri: String,
    /// JWK `k` field: base64url-encoded 256-bit AES key.
    pub key_b64url: String,
    /// Base64-encoded 16-byte IV (8 random bytes + 8 zero counter bytes).
    pub iv_b64: String,
    /// Unpadded base64-encoded SHA-256 hash of the ciphertext.
    pub sha256_b64: String,
}

impl EncryptedAttachment {
    /// Build the Matrix `file` JSON object for this encrypted attachment.
    pub fn to_file_json(&self) -> serde_json::Value {
        serde_json::json!({
            "url": self.mxc_uri,
            "key": {
                "kty": "oct",
                "key_ops": ["encrypt", "decrypt"],
                "alg": "A256CTR",
                "k": self.key_b64url,
                "ext": true,
            },
            "iv": self.iv_b64,
            "hashes": {
                "sha256": self.sha256_b64,
            },
            "v": "v2",
        })
    }
}

/// Encrypt file data for a Matrix encrypted attachment.
///
/// Returns `(ciphertext, key, iv)` where:
/// - `ciphertext` is the AES-256-CTR encrypted data
/// - `key` is the 32-byte AES key
/// - `iv` is the 16-byte IV (8 random + 8 zero counter)
pub(super) fn encrypt_attachment(plaintext: &[u8]) -> anyhow::Result<(Vec<u8>, [u8; 32], [u8; 16])> {
    // Generate random key and IV.
    let mut key = [0u8; 32];
    let mut iv = [0u8; 16];

    getrandom::getrandom(&mut key)
        .map_err(|e| anyhow::anyhow!("failed to generate random key: {e}"))?;
    // Only first 8 bytes are random; last 8 are the counter (start at 0).
    getrandom::getrandom(&mut iv[..8])
        .map_err(|e| anyhow::anyhow!("failed to generate random IV: {e}"))?;

    let mut ciphertext = plaintext.to_vec();
    let mut cipher = Aes256Ctr::new(&key.into(), &iv.into());
    cipher.apply_keystream(&mut ciphertext);

    Ok((ciphertext, key, iv))
}

/// Encrypt data and return the full metadata needed for a Matrix `file` object.
///
/// This is the high-level function: encrypt, compute hash, encode to base64.
pub(super) fn encrypt_attachment_full(plaintext: &[u8]) -> anyhow::Result<EncryptedAttachmentData> {
    let (ciphertext, key, iv) = encrypt_attachment(plaintext)?;

    // SHA-256 hash of the ciphertext.
    let hash = Sha256::digest(&ciphertext);

    Ok(EncryptedAttachmentData {
        ciphertext,
        key_b64url: URL_SAFE_NO_PAD.encode(key),
        iv_b64: STANDARD_NO_PAD.encode(iv),
        sha256_b64: STANDARD_NO_PAD.encode(hash),
    })
}

/// Result of encrypting an attachment, before uploading.
pub(super) struct EncryptedAttachmentData {
    /// The encrypted file bytes to upload.
    pub ciphertext: Vec<u8>,
    /// Base64url-encoded AES-256 key (JWK `k` field).
    pub key_b64url: String,
    /// Base64-encoded IV.
    pub iv_b64: String,
    /// Unpadded base64-encoded SHA-256 of ciphertext.
    pub sha256_b64: String,
}

/// Decrypt an encrypted Matrix attachment.
///
/// This is the reverse of `encrypt_attachment`: given ciphertext, the 256-bit
/// AES key, and the 16-byte IV, returns the plaintext. Optionally verifies
/// the SHA-256 hash of the ciphertext if `expected_sha256` is provided.
pub(super) fn decrypt_attachment(
    ciphertext: &[u8],
    key_b64url: &str,
    iv_b64: &str,
    expected_sha256_b64: Option<&str>,
) -> anyhow::Result<Vec<u8>> {
    // Verify ciphertext hash if provided (constant-time comparison).
    if let Some(expected) = expected_sha256_b64 {
        let hash = Sha256::digest(ciphertext);
        let actual = STANDARD_NO_PAD.encode(hash);
        if actual.len() != expected.len()
            || !bool::from(subtle::ConstantTimeEq::ct_eq(
                actual.as_bytes(),
                expected.as_bytes(),
            ))
        {
            anyhow::bail!("ciphertext SHA-256 mismatch");
        }
    }

    let key_bytes = URL_SAFE_NO_PAD
        .decode(key_b64url)
        .map_err(|e| anyhow::anyhow!("invalid base64url key: {e}"))?;
    if key_bytes.len() != 32 {
        anyhow::bail!("AES-256 key must be 32 bytes, got {}", key_bytes.len());
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&key_bytes);

    let iv_bytes = STANDARD_NO_PAD
        .decode(iv_b64)
        .map_err(|e| anyhow::anyhow!("invalid base64 IV: {e}"))?;
    if iv_bytes.len() != 16 {
        anyhow::bail!("IV must be 16 bytes, got {}", iv_bytes.len());
    }
    let mut iv = [0u8; 16];
    iv.copy_from_slice(&iv_bytes);

    let mut plaintext = ciphertext.to_vec();
    let mut cipher = Aes256Ctr::new(&key.into(), &iv.into());
    cipher.apply_keystream(&mut plaintext);

    Ok(plaintext)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypt_and_verify_hash() {
        let plaintext = b"Hello, Matrix encrypted attachment!";
        let data = encrypt_attachment_full(plaintext).unwrap();

        // Ciphertext should be same length as plaintext (CTR mode).
        assert_eq!(data.ciphertext.len(), plaintext.len());
        // Ciphertext should differ from plaintext.
        assert_ne!(&data.ciphertext, plaintext);

        // Verify SHA-256 hash.
        let hash = Sha256::digest(&data.ciphertext);
        let expected_hash = STANDARD_NO_PAD.encode(hash);
        assert_eq!(data.sha256_b64, expected_hash);
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let plaintext = b"Roundtrip test data for AES-256-CTR";
        let (ciphertext, key, iv) = encrypt_attachment(plaintext).unwrap();

        // Decrypt by applying the same keystream again.
        let mut decrypted = ciphertext.clone();
        let mut cipher = Aes256Ctr::new(&key.into(), &iv.into());
        cipher.apply_keystream(&mut decrypted);

        assert_eq!(&decrypted, plaintext);
    }

    #[test]
    fn iv_counter_bytes_are_zero() {
        let plaintext = b"test";
        let (_, _, iv) = encrypt_attachment(plaintext).unwrap();
        // Last 8 bytes (counter) should be zero.
        assert_eq!(&iv[8..], &[0u8; 8]);
    }

    #[test]
    fn encrypted_attachment_to_file_json() {
        let att = EncryptedAttachment {
            mxc_uri: "mxc://example.com/abc".to_string(),
            key_b64url: "dGVzdGtleQ".to_string(),
            iv_b64: "dGVzdGl2".to_string(),
            sha256_b64: "dGVzdGhhc2g".to_string(),
        };
        let json = att.to_file_json();
        assert_eq!(json["url"], "mxc://example.com/abc");
        assert_eq!(json["key"]["alg"], "A256CTR");
        assert_eq!(json["key"]["k"], "dGVzdGtleQ");
        assert_eq!(json["v"], "v2");
        assert_eq!(json["hashes"]["sha256"], "dGVzdGhhc2g");
    }

    #[test]
    fn different_encryptions_produce_different_keys() {
        let plaintext = b"same data";
        let (_, key1, iv1) = encrypt_attachment(plaintext).unwrap();
        let (_, key2, iv2) = encrypt_attachment(plaintext).unwrap();
        // Keys should differ (random).
        assert_ne!(key1, key2);
        // IVs should differ (random first 8 bytes).
        assert_ne!(&iv1[..8], &iv2[..8]);
    }

    #[test]
    fn decrypt_attachment_roundtrip() {
        let plaintext = b"Decrypt test: roundtrip via encrypt_attachment_full";
        let data = encrypt_attachment_full(plaintext).unwrap();

        let decrypted = decrypt_attachment(
            &data.ciphertext,
            &data.key_b64url,
            &data.iv_b64,
            Some(&data.sha256_b64),
        )
        .unwrap();

        assert_eq!(&decrypted, plaintext);
    }

    #[test]
    fn decrypt_attachment_rejects_wrong_hash() {
        let plaintext = b"hash check test";
        let data = encrypt_attachment_full(plaintext).unwrap();

        let result = decrypt_attachment(
            &data.ciphertext,
            &data.key_b64url,
            &data.iv_b64,
            Some("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
        );
        assert!(result.is_err());
    }
}
