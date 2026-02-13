/**
 * Internal endpoints for tracker-core integration.
 *
 * These endpoints are called by tracker-core to load tenant secrets
 * for signature verification. Protected by the admin bootstrap key.
 */

import { Hono } from "hono";
import { type AppType } from "../types.js";
import { requireAdmin } from "../middleware/auth.js";

const internal = new Hono<AppType>();

// Internal endpoints require admin access
internal.use("/*", requireAdmin());

/**
 * GET /internal/secrets — Return all tenant key_prefix → hmac_secret mappings.
 *
 * tracker-core calls this on startup and periodically to refresh its
 * in-memory cache. Returns a flat map for O(1) prefix lookups.
 *
 * Response:
 * ```json
 * {
 *   "secrets": {
 *     "tk8a": { "hmac_secret": "...", "encryption_key": "..." },
 *     "b2x9": { "hmac_secret": "...", "encryption_key": "..." }
 *   },
 *   "count": 2
 * }
 * ```
 */
internal.get("/secrets", async (c) => {
  const db = c.get("db");

  const result = await db.execute(
    "SELECT key_prefix, hmac_secret, encryption_key, rate_limit_rps FROM tenants"
  );

  const secrets: Record<string, { hmac_secret: string; encryption_key: string; rate_limit_rps: number }> = {};
  for (const row of result.rows) {
    secrets[row.key_prefix as string] = {
      hmac_secret: row.hmac_secret as string,
      encryption_key: row.encryption_key as string,
      rate_limit_rps: (row.rate_limit_rps as number) ?? 100,
    };
  }

  return c.json({ secrets, count: result.rows.length });
});

/**
 * GET /internal/tracking-urls — Return all tracking URL mappings.
 *
 * tracker-core calls this on startup and periodically to refresh its
 * in-memory cache. Returns a flat map for O(1) tu_id → destination lookups.
 *
 * Response:
 * ```json
 * {
 *   "urls": {
 *     "tu_019502a1-...": { "destination": "https://...", "key_prefix": "6vct" }
 *   },
 *   "count": 42
 * }
 * ```
 */
internal.get("/tracking-urls", async (c) => {
  const db = c.get("db");

  const result = await db.execute(
    `SELECT tu.id, tu.destination, t.key_prefix
     FROM tracking_urls tu
     JOIN tenants t ON tu.tenant_id = t.id`
  );

  const urls: Record<string, { destination: string; key_prefix: string }> = {};
  for (const row of result.rows) {
    urls[row.id as string] = {
      destination: row.destination as string,
      key_prefix: row.key_prefix as string,
    };
  }

  return c.json({ urls, count: result.rows.length });
});

/**
 * POST /internal/validate-ingest-token — Validate an ingest token hash.
 *
 * tracker-core calls this when it receives a POST /ingest with a Bearer token
 * that isn't in its local cache. Returns the key_prefix if valid, or 401.
 *
 * Request: { "token_hash": "<sha256 hex>" }
 * Response (valid): { "valid": true, "key_prefix": "6vct", "token_id": "..." }
 * Response (invalid): { "valid": false }
 */
internal.post("/validate-ingest-token", async (c) => {
  const db = c.get("db");
  const body = await c.req.json<{ token_hash: string }>();

  if (!body.token_hash) {
    return c.json({ error: "token_hash is required" }, 400);
  }

  const result = await db.execute({
    sql: `SELECT id, key_prefix, expires_at, revoked
          FROM ingest_tokens
          WHERE token_hash = ?`,
    args: [body.token_hash],
  });

  if (result.rows.length === 0) {
    return c.json({ valid: false });
  }

  const row = result.rows[0];

  // Check if revoked
  if (row.revoked) {
    return c.json({ valid: false, reason: "revoked" });
  }

  // Check if expired
  if (row.expires_at) {
    const now = Math.floor(Date.now() / 1000);
    if (now > (row.expires_at as number)) {
      return c.json({ valid: false, reason: "expired" });
    }
  }

  // Update last_used_at (fire-and-forget)
  db.execute({
    sql: "UPDATE ingest_tokens SET last_used_at = unixepoch() WHERE id = ?",
    args: [row.id],
  }).catch(() => {});

  return c.json({
    valid: true,
    key_prefix: row.key_prefix as string,
    token_id: row.id as string,
  });
});

export default internal;
