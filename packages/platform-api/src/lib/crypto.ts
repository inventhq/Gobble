/**
 * Cryptographic utilities for the Platform API.
 *
 * Uses the Web Crypto API (available in CF Workers) for all operations.
 * No Node.js dependencies.
 */

/** Generate a random hex string of the given byte length. */
export function randomHex(bytes: number): string {
  const buf = new Uint8Array(bytes);
  crypto.getRandomValues(buf);
  return Array.from(buf)
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}

/** Generate a random alphanumeric string of the given length. */
export function randomAlphanumeric(length: number): string {
  const chars = "abcdefghijklmnopqrstuvwxyz0123456789";
  const buf = new Uint8Array(length);
  crypto.getRandomValues(buf);
  return Array.from(buf)
    .map((b) => chars[b % chars.length])
    .join("");
}

/** Generate a ULID-like ID (timestamp prefix + random suffix). */
export function generateId(): string {
  const ts = Date.now().toString(36).padStart(9, "0");
  const rand = randomAlphanumeric(16);
  return `${ts}${rand}`;
}

/** SHA-256 hash a string, return hex. */
export async function sha256(input: string): Promise<string> {
  const encoded = new TextEncoder().encode(input);
  const hash = await crypto.subtle.digest("SHA-256", encoded);
  return Array.from(new Uint8Array(hash))
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}

/** Generate a new API key with a recognizable prefix. */
export function generateApiKey(): string {
  // Format: tk_live_<32 random chars>
  return `tk_live_${randomAlphanumeric(32)}`;
}

/** Generate a unique 4-character tenant key prefix for signature routing. */
export function generateKeyPrefix(): string {
  return randomAlphanumeric(4);
}

/** Generate an HMAC secret for a tenant. */
export function generateHmacSecret(): string {
  return randomHex(32); // 32 bytes = 64 hex chars
}

/** Generate an AES-256-GCM encryption key for a tenant. */
export function generateEncryptionKey(): string {
  return randomHex(32); // 32 bytes = 64 hex chars
}

/** Generate a webhook signing secret. */
export function generateWebhookSecret(): string {
  return `whsec_${randomAlphanumeric(32)}`;
}

/** Generate a UUIDv7 (time-ordered, ms precision + random). */
export function generateUUIDv7(): string {
  const now = Date.now();
  const buf = new Uint8Array(16);
  crypto.getRandomValues(buf);
  // Timestamp (48 bits, big-endian)
  buf[0] = (now / 2 ** 40) & 0xff;
  buf[1] = (now / 2 ** 32) & 0xff;
  buf[2] = (now / 2 ** 24) & 0xff;
  buf[3] = (now / 2 ** 16) & 0xff;
  buf[4] = (now / 2 ** 8) & 0xff;
  buf[5] = now & 0xff;
  // Version 7
  buf[6] = (buf[6] & 0x0f) | 0x70;
  // Variant 10xx
  buf[8] = (buf[8] & 0x3f) | 0x80;
  const hex = Array.from(buf)
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

/** Generate a tracking URL ID (tu_ prefix + UUIDv7). */
export function generateTrackingUrlId(): string {
  return `tu_${generateUUIDv7()}`;
}
