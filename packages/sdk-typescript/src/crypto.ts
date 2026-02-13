/**
 * Cryptographic utilities for URL signing and encryption.
 *
 * Uses Node.js built-in `crypto` module — zero external dependencies.
 * These are pure functions with no network calls, so link generation
 * is instantaneous and works offline.
 */

import { createHmac, createCipheriv, createDecipheriv, randomBytes } from "node:crypto";

/**
 * Generate an HMAC-SHA256 signature for a URL string.
 *
 * @param secret - The shared HMAC secret (must match server's `HMAC_SECRET`).
 * @param url - The destination URL to sign.
 * @returns Lowercase hex-encoded signature string.
 */
export function signHmac(secret: string, url: string): string {
  return createHmac("sha256", secret).update(url).digest("hex");
}

/**
 * Verify an HMAC-SHA256 signature against a URL string.
 *
 * @param secret - The shared HMAC secret.
 * @param url - The destination URL that was signed.
 * @param signature - The hex-encoded signature to verify.
 * @returns `true` if the signature is valid.
 */
export function verifyHmac(secret: string, url: string, signature: string): boolean {
  const expected = signHmac(secret, url);
  // Constant-time comparison to prevent timing attacks
  if (expected.length !== signature.length) return false;
  let result = 0;
  for (let i = 0; i < expected.length; i++) {
    result |= expected.charCodeAt(i) ^ signature.charCodeAt(i);
  }
  return result === 0;
}

/**
 * Encrypt a URL using AES-256-GCM.
 *
 * Returns a base64url-encoded string containing `nonce || ciphertext || tag`.
 * Compatible with the Rust core's `decrypt_url()`.
 *
 * @param keyHex - 64-character hex string (32 bytes) for the AES key.
 * @param url - The destination URL to encrypt.
 * @returns Base64url-encoded encrypted blob (no padding).
 */
export function encryptUrl(keyHex: string, url: string): string {
  const key = Buffer.from(keyHex, "hex");
  if (key.length !== 32) {
    throw new Error(`Encryption key must be 32 bytes (got ${key.length})`);
  }

  const nonce = randomBytes(12); // 96-bit nonce for GCM
  const cipher = createCipheriv("aes-256-gcm", key, nonce);

  const encrypted = Buffer.concat([cipher.update(url, "utf8"), cipher.final()]);
  const tag = cipher.getAuthTag(); // 16 bytes

  // Format: nonce (12) || ciphertext || tag (16)
  // This matches the Rust core's format: nonce || ciphertext+tag
  // (aes-gcm crate appends tag to ciphertext automatically)
  const combined = Buffer.concat([nonce, encrypted, tag]);

  // Base64url encoding without padding (matches Rust's URL_SAFE_NO_PAD)
  return combined.toString("base64url");
}

/**
 * Decrypt a base64url-encoded AES-256-GCM payload back to the original URL.
 *
 * @param keyHex - 64-character hex string (32 bytes) for the AES key.
 * @param encoded - Base64url-encoded encrypted blob from `encryptUrl()`.
 * @returns The original plaintext URL.
 */
export function decryptUrl(keyHex: string, encoded: string): string {
  const key = Buffer.from(keyHex, "hex");
  const combined = Buffer.from(encoded, "base64url");

  if (combined.length < 28) {
    // 12 (nonce) + 16 (tag) minimum
    throw new Error("Ciphertext too short");
  }

  const nonce = combined.subarray(0, 12);
  const ciphertext = combined.subarray(12, combined.length - 16);
  const tag = combined.subarray(combined.length - 16);

  const decipher = createDecipheriv("aes-256-gcm", key, nonce);
  decipher.setAuthTag(tag);

  const decrypted = Buffer.concat([decipher.update(ciphertext), decipher.final()]);
  return decrypted.toString("utf8");
}
