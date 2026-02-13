/**
 * Tracking link generation utilities.
 *
 * Pure functions that build signed or encrypted tracking URLs.
 * No network calls — links are generated entirely on the developer's server.
 */

import { signHmac, encryptUrl } from "./crypto.js";

/**
 * Build a signed click-tracking URL.
 *
 * The destination URL is visible in the query string, protected by an
 * HMAC-SHA256 signature. Additional parameters are appended as query params.
 *
 * @param baseUrl - The tracker-core server URL (e.g. "https://track.example.com").
 * @param secret - The shared HMAC secret.
 * @param destinationUrl - The URL the user will be redirected to.
 * @param params - Optional additional tracking parameters (offer_id, aff_id, etc.).
 * @returns The full signed tracking URL.
 *
 * @example
 * ```ts
 * const url = buildSignedClickUrl(
 *   "https://track.example.com",
 *   "my-secret",
 *   "https://offer.example.com/landing",
 *   { offer_id: "123", aff_id: "456" }
 * );
 * // => "https://track.example.com/t?url=https%3A%2F%2Foffer.example.com%2Flanding&sig=abc123...&offer_id=123&aff_id=456"
 * ```
 */
export function buildSignedClickUrl(
  baseUrl: string,
  secret: string,
  destinationUrl: string,
  params?: Record<string, string>,
): string {
  const rawSig = signHmac(secret, destinationUrl);
  // Multi-tenant: prefix the signature with key_prefix so tracker-core
  // knows which tenant's HMAC secret to verify against.
  const sig = params?.key_prefix ? `${params.key_prefix}_${rawSig}` : rawSig;
  const qs = new URLSearchParams();
  qs.set("url", destinationUrl);
  qs.set("sig", sig);

  if (params) {
    for (const [key, value] of Object.entries(params)) {
      qs.set(key, value);
    }
  }

  return `${baseUrl.replace(/\/+$/, "")}/t?${qs.toString()}`;
}

/**
 * Build an encrypted click-tracking URL.
 *
 * The destination URL is encrypted with AES-256-GCM and passed as an opaque
 * base64url blob. The URL is invisible to the end user.
 *
 * @param baseUrl - The tracker-core server URL.
 * @param keyHex - The 64-character hex encryption key.
 * @param destinationUrl - The URL the user will be redirected to.
 * @param params - Optional additional tracking parameters.
 * @returns The full encrypted tracking URL.
 *
 * @example
 * ```ts
 * const url = buildEncryptedClickUrl(
 *   "https://track.example.com",
 *   "aabbccdd...64chars",
 *   "https://offer.example.com/landing",
 *   { offer_id: "123" }
 * );
 * // => "https://track.example.com/t?d=base64urlblob&offer_id=123"
 * ```
 */
export function buildEncryptedClickUrl(
  baseUrl: string,
  keyHex: string,
  destinationUrl: string,
  params?: Record<string, string>,
): string {
  const encrypted = encryptUrl(keyHex, destinationUrl);
  const qs = new URLSearchParams();
  qs.set("d", encrypted);

  if (params) {
    for (const [key, value] of Object.entries(params)) {
      qs.set(key, value);
    }
  }

  return `${baseUrl.replace(/\/+$/, "")}/t?${qs.toString()}`;
}

/**
 * Build a postback URL for server-to-server conversion tracking.
 *
 * @param baseUrl - The tracker-core server URL.
 * @param params - Conversion parameters (click_id, payout, status, etc.).
 * @returns The full postback URL.
 *
 * @example
 * ```ts
 * const url = buildPostbackUrl("https://track.example.com", {
 *   click_id: "abc123",
 *   payout: "2.50",
 *   status: "approved",
 * });
 * // => "https://track.example.com/p?click_id=abc123&payout=2.50&status=approved"
 * ```
 */
export function buildPostbackUrl(
  baseUrl: string,
  params: Record<string, string>,
): string {
  const qs = new URLSearchParams(params);
  return `${baseUrl.replace(/\/+$/, "")}/p?${qs.toString()}`;
}

/**
 * Build an impression pixel URL for embedding in HTML/emails.
 *
 * @param baseUrl - The tracker-core server URL.
 * @param params - Impression parameters (campaign_id, placement, etc.).
 * @returns The full impression pixel URL.
 *
 * @example
 * ```ts
 * const url = buildImpressionUrl("https://track.example.com", {
 *   campaign_id: "789",
 *   placement: "header_banner",
 * });
 * // => "https://track.example.com/i?campaign_id=789&placement=header_banner"
 *
 * // Embed in HTML:
 * const pixel = `<img src="${url}" width="1" height="1" alt="" />`;
 * ```
 */
/**
 * Build a tracked click URL using a registered tracking URL ID.
 *
 * The destination is resolved server-side from the tracking URL cache,
 * so it doesn't appear in the URL. The HMAC signature signs the `tu_id`
 * (not the destination), enabling destination rotation without
 * regenerating distributed links.
 *
 * @param baseUrl - The tracker-core server URL (e.g. "https://track.example.com").
 * @param secret - The shared HMAC secret.
 * @param tuId - The tracking URL ID (e.g. "tu_019502a1-7b3c-7def-8abc-1234567890ab").
 * @param params - Optional additional tracking parameters (aff_id, click_id, etc.).
 * @returns The full signed short tracking URL.
 *
 * @example
 * ```ts
 * const url = buildTrackedClickUrl(
 *   "https://track.example.com",
 *   "my-secret",
 *   "tu_019502a1-7b3c-7def-8abc-1234567890ab",
 *   { key_prefix: "6vct", aff_id: "456" }
 * );
 * // => "https://track.example.com/t/tu_019502a1-...?sig=6vct_abc123...&key_prefix=6vct&aff_id=456"
 * ```
 */
export function buildTrackedClickUrl(
  baseUrl: string,
  secret: string,
  tuId: string,
  params?: Record<string, string>,
): string {
  // Sign the tu_id (not a destination URL)
  const rawSig = signHmac(secret, tuId);
  const sig = params?.key_prefix ? `${params.key_prefix}_${rawSig}` : rawSig;
  const qs = new URLSearchParams();
  qs.set("sig", sig);

  if (params) {
    for (const [key, value] of Object.entries(params)) {
      qs.set(key, value);
    }
  }

  return `${baseUrl.replace(/\/+$/, "")}/t/${tuId}?${qs.toString()}`;
}

export function buildImpressionUrl(
  baseUrl: string,
  params: Record<string, string>,
): string {
  const qs = new URLSearchParams(params);
  return `${baseUrl.replace(/\/+$/, "")}/i?${qs.toString()}`;
}
