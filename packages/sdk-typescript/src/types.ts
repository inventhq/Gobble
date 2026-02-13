/**
 * A single tracking event matching the tracker-core schema.
 *
 * The SDK constructs these for batch submission. The `event_id` and
 * `timestamp` are auto-generated if not provided.
 */
export interface TrackingEvent {
  /** Unique event identifier (UUIDv7 recommended). Auto-generated if omitted. */
  event_id: string;
  /** Event type: "click", "postback", or "impression". */
  event_type: string;
  /** Unix timestamp in milliseconds. Auto-generated if omitted. */
  timestamp: number;
  /** Client IP address. */
  ip: string;
  /** Client User-Agent string. */
  user_agent: string;
  /** HTTP Referer header, if available. */
  referer: string | null;
  /** Accept-Language header, if available. */
  accept_language: string | null;
  /** The endpoint path (e.g. "/t", "/p", "/i"). */
  request_path: string;
  /** The Host header value. */
  request_host: string;
  /** Arbitrary key-value parameters — passed through opaquely. */
  params: Record<string, string>;
}

/**
 * Configuration for the Tracker SDK.
 */
export interface TrackerConfig {
  /** Base URL of the tracker-core server (e.g. "https://track.example.com"). */
  apiUrl: string;

  /**
   * URL security mode. Must match the server's `URL_MODE`.
   * - `"signed"` — HMAC-SHA256 signature appended to the URL.
   * - `"encrypted"` — AES-256-GCM encrypted URL blob.
   */
  mode: "signed" | "encrypted";

  /**
   * HMAC secret (required when `mode` is `"signed"`).
   * Must match the server's `HMAC_SECRET`.
   */
  hmacSecret?: string;

  /**
   * AES-256-GCM encryption key as a hex string (64 hex chars = 32 bytes).
   * Required when `mode` is `"encrypted"`.
   * Must match the server's `ENCRYPTION_KEY`.
   */
  encryptionKey?: string;

  /**
   * Maximum number of events to buffer before auto-flushing.
   * @default 100
   */
  batchSize?: number;

  /**
   * Maximum time in milliseconds to wait before flushing a partial batch.
   * @default 1000
   */
  flushInterval?: number;

  /**
   * Called when a batch flush fails after all retries.
   * Receives the error and the events that failed to send.
   */
  onError?: (error: Error, events: TrackingEvent[]) => void;
}

/** Response from the `POST /batch` endpoint. */
export interface BatchResponse {
  accepted: number;
}
