/**
 * @tracker/sdk — TypeScript SDK for tracker-core.
 *
 * Provides two main capabilities:
 * 1. **Link generation** — pure functions to build signed/encrypted tracking URLs.
 * 2. **Batch client** — buffered event delivery to the `POST /batch` endpoint.
 *
 * @example
 * ```ts
 * import {
 *   buildSignedClickUrl,
 *   buildPostbackUrl,
 *   buildImpressionUrl,
 *   TrackerClient,
 * } from "@tracker/sdk";
 *
 * // Generate a signed click URL (pure function, no network)
 * const clickUrl = buildSignedClickUrl(
 *   "https://track.example.com",
 *   "my-hmac-secret",
 *   "https://offer.example.com/landing",
 *   { offer_id: "123", aff_id: "456" },
 * );
 *
 * // Batch client for server-side event ingestion
 * const client = new TrackerClient({
 *   apiUrl: "https://track.example.com",
 *   mode: "signed",
 *   hmacSecret: "my-hmac-secret",
 *   batchSize: 100,
 *   flushInterval: 1000,
 * });
 *
 * client.track({ event_type: "postback", ip: "1.2.3.4", ... });
 * await client.destroy(); // flush on shutdown
 * ```
 *
 * @packageDocumentation
 */

// Types
export type { TrackingEvent, TrackerConfig, BatchResponse } from "./types.js";

// Crypto utilities
export { signHmac, verifyHmac, encryptUrl, decryptUrl } from "./crypto.js";

// Link builders
export {
  buildSignedClickUrl,
  buildTrackedClickUrl,
  buildEncryptedClickUrl,
  buildPostbackUrl,
  buildImpressionUrl,
} from "./links.js";

// Batch client
export { TrackerClient } from "./client.js";
export type { TrackInput } from "./client.js";
