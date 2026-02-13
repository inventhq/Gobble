/**
 * Batch event client for tracker-core.
 *
 * Buffers tracking events locally and flushes them to the `POST /batch`
 * endpoint in configurable batches. Supports both size-based and time-based
 * flush triggers for optimal throughput at any event volume.
 *
 * @example
 * ```ts
 * import { TrackerClient } from "@tracker/sdk";
 *
 * const client = new TrackerClient({
 *   apiUrl: "https://track.example.com",
 *   mode: "signed",
 *   hmacSecret: "my-secret",
 *   batchSize: 100,
 *   flushInterval: 1000,
 * });
 *
 * // Queue events — they flush automatically
 * client.track({
 *   event_type: "postback",
 *   ip: "203.0.113.1",
 *   user_agent: "Mozilla/5.0",
 *   request_path: "/p",
 *   request_host: "track.example.com",
 *   params: { click_id: "abc123", payout: "2.50" },
 * });
 *
 * // Flush remaining events on shutdown
 * await client.flush();
 * client.destroy();
 * ```
 */

import { randomUUID } from "node:crypto";
import type { TrackingEvent, TrackerConfig, BatchResponse } from "./types.js";

/** Fields the developer must provide when tracking an event. */
export type TrackInput = Omit<TrackingEvent, "event_id" | "timestamp"> & {
  /** Override the auto-generated event ID. */
  event_id?: string;
  /** Override the auto-generated timestamp (ms). */
  timestamp?: number;
};

/**
 * Buffered batch client for sending tracking events to tracker-core.
 *
 * Events are queued in memory and flushed to `POST /batch` when either
 * the batch size limit or the flush interval timer fires — whichever
 * comes first. This minimizes HTTP overhead while keeping delivery latency
 * bounded.
 */
export class TrackerClient {
  private readonly apiUrl: string;
  private readonly batchSize: number;
  private readonly flushInterval: number;
  private readonly onError?: (error: Error, events: TrackingEvent[]) => void;

  private buffer: TrackingEvent[] = [];
  private timer: ReturnType<typeof setInterval> | null = null;
  private flushing = false;

  constructor(config: TrackerConfig) {
    this.apiUrl = config.apiUrl.replace(/\/+$/, "");
    this.batchSize = config.batchSize ?? 100;
    this.flushInterval = config.flushInterval ?? 1000;
    this.onError = config.onError;

    // Start the periodic flush timer
    if (this.flushInterval > 0) {
      this.timer = setInterval(() => {
        if (this.buffer.length > 0) {
          this.flush().catch(() => {});
        }
      }, this.flushInterval);

      // Allow the process to exit even if the timer is running
      if (this.timer && typeof this.timer === "object" && "unref" in this.timer) {
        this.timer.unref();
      }
    }
  }

  /**
   * Queue a tracking event for batch delivery.
   *
   * Auto-generates `event_id` (UUIDv4) and `timestamp` if not provided.
   * Triggers an immediate flush if the buffer reaches `batchSize`.
   */
  track(input: TrackInput): void {
    const event: TrackingEvent = {
      event_id: input.event_id ?? randomUUID(),
      timestamp: input.timestamp ?? Date.now(),
      event_type: input.event_type,
      ip: input.ip,
      user_agent: input.user_agent,
      referer: input.referer,
      accept_language: input.accept_language,
      request_path: input.request_path,
      request_host: input.request_host,
      params: input.params,
    };

    this.buffer.push(event);

    if (this.buffer.length >= this.batchSize) {
      this.flush().catch(() => {});
    }
  }

  /**
   * Immediately flush all buffered events to the server.
   *
   * Safe to call multiple times — concurrent flushes are serialized.
   * Returns the number of events accepted by the server, or 0 on failure.
   */
  async flush(): Promise<number> {
    if (this.buffer.length === 0 || this.flushing) return 0;

    this.flushing = true;
    const batch = this.buffer.splice(0);

    try {
      const response = await fetch(`${this.apiUrl}/batch`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(batch),
      });

      if (!response.ok) {
        const text = await response.text();
        throw new Error(`Batch rejected (${response.status}): ${text}`);
      }

      const result = (await response.json()) as BatchResponse;
      return result.accepted;
    } catch (error) {
      // Put events back at the front of the buffer for retry
      this.buffer.unshift(...batch);

      if (this.onError) {
        this.onError(error instanceof Error ? error : new Error(String(error)), batch);
      }
      return 0;
    } finally {
      this.flushing = false;
    }
  }

  /** Returns the number of events currently buffered. */
  get pending(): number {
    return this.buffer.length;
  }

  /**
   * Stop the flush timer and flush remaining events.
   *
   * Call this during graceful shutdown to ensure no events are lost.
   */
  async destroy(): Promise<void> {
    if (this.timer) {
      clearInterval(this.timer);
      this.timer = null;
    }
    await this.flush();
  }
}
