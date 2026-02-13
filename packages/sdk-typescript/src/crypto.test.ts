import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { signHmac, verifyHmac, encryptUrl, decryptUrl } from "./crypto.js";

describe("HMAC-SHA256", () => {
  const secret = "test-secret-key";
  const url = "https://example.com/landing?offer=123";

  it("should generate a consistent signature", () => {
    const sig1 = signHmac(secret, url);
    const sig2 = signHmac(secret, url);
    assert.equal(sig1, sig2);
    assert.equal(sig1.length, 64); // 32 bytes hex = 64 chars
  });

  it("should verify a valid signature", () => {
    const sig = signHmac(secret, url);
    assert.equal(verifyHmac(secret, url, sig), true);
  });

  it("should reject a tampered signature", () => {
    const sig = signHmac(secret, url);
    const tampered = "0".repeat(64);
    assert.equal(verifyHmac(secret, url, tampered), false);
  });

  it("should reject a wrong secret", () => {
    const sig = signHmac(secret, url);
    assert.equal(verifyHmac("wrong-secret", url, sig), false);
  });

  it("should reject a tampered URL", () => {
    const sig = signHmac(secret, url);
    assert.equal(verifyHmac(secret, url + "&evil=1", sig), false);
  });

  it("should produce a signature compatible with the Rust core", () => {
    // This is the same secret and URL used in the stress tests.
    // The Rust core uses identical HMAC-SHA256, so signatures must match.
    const testSecret = "super-secret-key-change-me";
    const testUrl = "https://example.com/landing";
    const sig = signHmac(testSecret, testUrl);
    assert.equal(sig, "5e52f1d64a5c437a084126d3040f19a7e4f997d44015796ad5e00b7fed404b08");
  });
});

describe("AES-256-GCM", () => {
  // 32 bytes = 64 hex chars
  const keyHex = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
  const url = "https://example.com/landing?offer=123";

  it("should encrypt and decrypt a URL", () => {
    const encrypted = encryptUrl(keyHex, url);
    const decrypted = decryptUrl(keyHex, encrypted);
    assert.equal(decrypted, url);
  });

  it("should produce different ciphertexts each time (random nonce)", () => {
    const enc1 = encryptUrl(keyHex, url);
    const enc2 = encryptUrl(keyHex, url);
    assert.notEqual(enc1, enc2);
  });

  it("should reject a wrong key", () => {
    const encrypted = encryptUrl(keyHex, url);
    const wrongKey = "f".repeat(64);
    assert.throws(() => decryptUrl(wrongKey, encrypted));
  });

  it("should reject tampered ciphertext", () => {
    const encrypted = encryptUrl(keyHex, url);
    // Decode, flip a byte in the ciphertext region, re-encode
    const buf = Buffer.from(encrypted, "base64url");
    buf[buf.length - 1] ^= 0xff; // flip last byte (inside auth tag)
    const tampered = buf.toString("base64url");
    assert.throws(() => decryptUrl(keyHex, tampered));
  });

  it("should reject a key that is not 32 bytes", () => {
    assert.throws(() => encryptUrl("aabb", url));
  });
});
