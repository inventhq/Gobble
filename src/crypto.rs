//! Cryptographic utilities for URL signing and encryption.
//!
//! Provides two security modes for the `/t` click-tracking endpoint:
//!
//! - **Signed (HMAC-SHA256):** The destination URL is visible in the query string
//!   and protected by a hex-encoded HMAC signature. Fast to verify, transparent.
//! - **Encrypted (AES-256-GCM):** The destination URL is encrypted into an opaque
//!   base64url blob. Provides confidentiality — the URL is invisible to the end user.
//!
//! The SDK uses [`sign_hmac()`] or [`encrypt_url()`] to generate links;
//! the core uses [`verify_hmac()`] or [`decrypt_url()`] to validate them.

use hmac::{Hmac, Mac};
use sha2::Sha256;

/// HMAC-SHA256 type alias for convenience.
type HmacSha256 = Hmac<Sha256>;

/// Verify an HMAC-SHA256 signature against a URL string.
///
/// Returns `true` if the hex-encoded `signature` matches the HMAC of `url`
/// computed with `secret`. Returns `false` on any error (bad hex, wrong key, etc.).
pub fn verify_hmac(secret: &str, url: &str, signature: &str) -> bool {
    let Ok(mut mac) = <HmacSha256 as Mac>::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(url.as_bytes());
    let Ok(sig_bytes) = hex::decode(signature) else {
        return false;
    };
    mac.verify_slice(&sig_bytes).is_ok()
}

/// Generate an HMAC-SHA256 signature for a URL string.
///
/// Returns the signature as a lowercase hex string. Used by the SDK
/// to sign tracking URLs before distribution.
pub fn sign_hmac(secret: &str, url: &str) -> String {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(secret.as_bytes())
        .expect("HMAC can take key of any size");
    mac.update(url.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

// --- AES-256-GCM encryption/decryption for opaque URL mode ---

use aes_gcm::{
    aead::{Aead, KeyInit, OsRng},
    Aes256Gcm, AeadCore, Nonce,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

/// Encrypt a URL using AES-256-GCM.
///
/// Generates a random 12-byte nonce, encrypts the URL, and returns a
/// base64url-encoded string containing `nonce || ciphertext || tag`.
/// Used by the SDK to create opaque tracking links.
pub fn encrypt_url(key: &[u8], url: &str) -> Result<String, String> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|e| format!("Invalid key: {e}"))?;
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, url.as_bytes())
        .map_err(|e| format!("Encryption failed: {e}"))?;

    // Prepend nonce (12 bytes) to ciphertext
    let mut combined = nonce.to_vec();
    combined.extend_from_slice(&ciphertext);
    Ok(URL_SAFE_NO_PAD.encode(&combined))
}

/// Decrypt a base64url-encoded AES-256-GCM payload back to the original URL.
///
/// Splits the first 12 bytes as the nonce, decrypts the remainder, and
/// returns the plaintext URL. Returns an error if the key is wrong,
/// the data is tampered, or the payload is malformed.
pub fn decrypt_url(key: &[u8], encoded: &str) -> Result<String, String> {
    let combined = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|e| format!("Invalid base64: {e}"))?;

    if combined.len() < 12 {
        return Err("Ciphertext too short".into());
    }

    let (nonce_bytes, ciphertext) = combined.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|e| format!("Invalid key: {e}"))?;
    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| "Decryption failed: invalid key or tampered data".to_string())?;

    String::from_utf8(plaintext).map_err(|e| format!("Invalid UTF-8: {e}"))
}
