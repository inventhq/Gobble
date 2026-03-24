/**
 * Onboarding helper routes.
 *
 * Convenience endpoints that simplify the agent-driven onboarding flow:
 * - Generate signed tracking links (server-side, no secret exposure)
 * - Send test events through the full pipeline
 * - Get ready-to-paste beacon snippets
 */

import { Hono } from "hono";
import { type AppType } from "../types.js";

const onboarding = new Hono<AppType>();

// ---------------------------------------------------------------------------
// HMAC-SHA256 signing using Web Crypto API (CF Workers compatible)
// ---------------------------------------------------------------------------

async function signHmac(secret: string, message: string): Promise<string> {
  const encoder = new TextEncoder();
  const key = await crypto.subtle.importKey(
    "raw",
    encoder.encode(secret),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"]
  );
  const signature = await crypto.subtle.sign("HMAC", key, encoder.encode(message));
  return Array.from(new Uint8Array(signature))
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}

// ---------------------------------------------------------------------------
// POST /api/tracking-urls/generate-link
// ---------------------------------------------------------------------------

/**
 * Generate a ready-to-use signed click tracking URL.
 *
 * Loads the tenant's HMAC secret from Turso, signs the destination URL,
 * and returns the complete tracking URL. No secret leaves the server.
 *
 * Body: { destination: string, params?: string (JSON), tenant_id?: string }
 */
onboarding.post("/tracking-urls/generate-link", async (c) => {
  const db = c.get("db");

  const body = await c.req.json<{
    destination: string;
    params?: string;
    tenant_id?: string;
  }>();

  if (!body.destination || typeof body.destination !== "string") {
    return c.json({ error: "destination is required" }, 400);
  }

  try {
    new URL(body.destination);
  } catch {
    return c.json({ error: "Invalid destination URL format" }, 400);
  }

  // Resolve tenant secret
  const effectiveTenantId = c.get("isAdmin") ? body.tenant_id : c.get("tenantId");
  if (!effectiveTenantId) {
    return c.json({ error: "tenant_id is required (admin must specify in body)" }, 400);
  }

  const tenant = await db.execute({
    sql: "SELECT key_prefix, hmac_secret FROM tenants WHERE id = ?",
    args: [effectiveTenantId],
  });

  if (tenant.rows.length === 0) {
    return c.json({ error: "Tenant not found" }, 404);
  }

  const keyPrefix = tenant.rows[0].key_prefix as string;
  const hmacSecret = tenant.rows[0].hmac_secret as string;

  // Sign the destination URL with the tenant's HMAC secret
  const rawSig = await signHmac(hmacSecret, body.destination);
  const sig = `${keyPrefix}_${rawSig}`;

  // Build the tracking URL
  const trackerHost = c.env.TRACKER_CORE_URL || "https://track.juicyapi.com";
  const qs = new URLSearchParams();
  qs.set("url", body.destination);
  qs.set("sig", sig);
  qs.set("key_prefix", keyPrefix);

  // Add custom params if provided
  if (body.params) {
    try {
      const extraParams = JSON.parse(body.params) as Record<string, string>;
      for (const [key, value] of Object.entries(extraParams)) {
        qs.set(key, value);
      }
    } catch {
      return c.json({ error: "Invalid params JSON" }, 400);
    }
  }

  const trackingUrl = `${trackerHost.replace(/\/+$/, "")}/t?${qs.toString()}`;

  return c.json({
    tracking_url: trackingUrl,
    destination: body.destination,
    key_prefix: keyPrefix,
  });
});

// ---------------------------------------------------------------------------
// POST /api/ingest-event
// ---------------------------------------------------------------------------

/**
 * Send an event through the full platform pipeline via tracker-core /ingest.
 *
 * Proxies to tracker-core's /ingest endpoint using the provided ingest token.
 * Useful for agents to verify the pipeline works end-to-end during onboarding.
 *
 * Body: { token: string, event_type: string, params?: string (JSON), raw_payload?: string (JSON) }
 */
onboarding.post("/ingest-event", async (c) => {
  const body = await c.req.json<{
    token: string;
    event_type: string;
    params?: string;
    raw_payload?: string;
  }>();

  if (!body.token || typeof body.token !== "string") {
    return c.json({ error: "token is required" }, 400);
  }
  if (!body.event_type || typeof body.event_type !== "string") {
    return c.json({ error: "event_type is required" }, 400);
  }

  // Parse optional params and raw_payload from JSON strings
  let params: Record<string, string> = {};
  if (body.params) {
    try {
      params = JSON.parse(body.params);
    } catch {
      return c.json({ error: "Invalid params JSON" }, 400);
    }
  }

  let rawPayload: unknown = undefined;
  if (body.raw_payload) {
    try {
      rawPayload = JSON.parse(body.raw_payload);
    } catch {
      return c.json({ error: "Invalid raw_payload JSON" }, 400);
    }
  }

  // Forward to tracker-core /ingest
  const trackerHost = c.env.TRACKER_CORE_URL || "http://localhost:3030";
  const ingestBody: Record<string, unknown> = {
    event_type: body.event_type,
    params,
  };
  if (rawPayload !== undefined) {
    ingestBody.raw_payload = rawPayload;
  }

  try {
    const resp = await fetch(`${trackerHost.replace(/\/+$/, "")}/ingest`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${body.token}`,
      },
      body: JSON.stringify(ingestBody),
    });

    const result = await resp.json();

    if (!resp.ok) {
      return c.json(
        { error: "Ingest failed", status: resp.status, details: result },
        resp.status as any
      );
    }

    return c.json(result);
  } catch (err) {
    return c.json(
      { error: "Failed to reach tracker-core", details: String(err) },
      502
    );
  }
});

// ---------------------------------------------------------------------------
// GET /api/beacon-snippet
// ---------------------------------------------------------------------------

/**
 * Get the ready-to-paste browser beacon script tag.
 *
 * Query: ?key_prefix=6vct&host=https://track.juicyapi.com (host is optional)
 */
onboarding.get("/beacon-snippet", async (c) => {
  const keyPrefix = c.req.query("key_prefix");
  if (!keyPrefix) {
    return c.json({ error: "key_prefix is required" }, 400);
  }

  const host = c.req.query("host") || "https://track.juicyapi.com";

  const snippet = `<script src="https://js.juicyapi.com/t.js" data-key="${keyPrefix}" data-host="${host}" async defer></script>`;

  return c.json({
    snippet,
    key_prefix: keyPrefix,
    host,
    instructions:
      "Add this script tag to any HTML page. It automatically tracks pageviews and outbound clicks. Zero cookies, zero fingerprinting.",
  });
});

export default onboarding;
