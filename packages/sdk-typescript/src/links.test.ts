import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { buildSignedClickUrl, buildEncryptedClickUrl, buildPostbackUrl, buildImpressionUrl } from "./links.js";
import { verifyHmac, decryptUrl } from "./crypto.js";

describe("buildSignedClickUrl", () => {
  const base = "https://track.example.com";
  const secret = "test-secret";
  const dest = "https://offer.example.com/landing";

  it("should build a URL with sig and url params", () => {
    const url = buildSignedClickUrl(base, secret, dest);
    const parsed = new URL(url);
    assert.equal(parsed.pathname, "/t");
    assert.equal(parsed.searchParams.get("url"), dest);
    assert.ok(parsed.searchParams.get("sig"));
  });

  it("should produce a valid HMAC signature", () => {
    const url = buildSignedClickUrl(base, secret, dest);
    const parsed = new URL(url);
    const sig = parsed.searchParams.get("sig")!;
    assert.equal(verifyHmac(secret, dest, sig), true);
  });

  it("should include extra params", () => {
    const url = buildSignedClickUrl(base, secret, dest, {
      offer_id: "123",
      aff_id: "456",
    });
    const parsed = new URL(url);
    assert.equal(parsed.searchParams.get("offer_id"), "123");
    assert.equal(parsed.searchParams.get("aff_id"), "456");
  });

  it("should strip trailing slashes from baseUrl", () => {
    const url = buildSignedClickUrl("https://track.example.com///", secret, dest);
    assert.ok(url.includes("/t?"));
    assert.ok(!url.includes("///"));
  });
});

describe("buildEncryptedClickUrl", () => {
  const base = "https://track.example.com";
  const keyHex = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
  const dest = "https://offer.example.com/landing";

  it("should build a URL with encrypted d param", () => {
    const url = buildEncryptedClickUrl(base, keyHex, dest);
    const parsed = new URL(url);
    assert.equal(parsed.pathname, "/t");
    assert.ok(parsed.searchParams.get("d"));
  });

  it("should produce a decryptable payload", () => {
    const url = buildEncryptedClickUrl(base, keyHex, dest);
    const parsed = new URL(url);
    const d = parsed.searchParams.get("d")!;
    const decrypted = decryptUrl(keyHex, d);
    assert.equal(decrypted, dest);
  });
});

describe("buildPostbackUrl", () => {
  it("should build a /p URL with params", () => {
    const url = buildPostbackUrl("https://track.example.com", {
      click_id: "abc123",
      payout: "2.50",
    });
    const parsed = new URL(url);
    assert.equal(parsed.pathname, "/p");
    assert.equal(parsed.searchParams.get("click_id"), "abc123");
    assert.equal(parsed.searchParams.get("payout"), "2.50");
  });
});

describe("buildImpressionUrl", () => {
  it("should build an /i URL with params", () => {
    const url = buildImpressionUrl("https://track.example.com", {
      campaign_id: "789",
      placement: "header",
    });
    const parsed = new URL(url);
    assert.equal(parsed.pathname, "/i");
    assert.equal(parsed.searchParams.get("campaign_id"), "789");
    assert.equal(parsed.searchParams.get("placement"), "header");
  });
});
