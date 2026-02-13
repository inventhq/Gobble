/**
 * Ingest token management routes.
 *
 * Ingest tokens authenticate POST /ingest calls from plugin-runtime and
 * direct programmatic ingestion. Token format: pt_{key_prefix}_{random}.
 *
 * tracker-core validates the SHA-256 hash and injects key_prefix from the
 * token record — callers cannot choose their own key_prefix (prevents
 * tenant spoofing). Tracking endpoints (/t /p /i) remain unauthenticated.
 */

import { Hono } from "hono";
import { type AppType } from "../types.js";
import { generateId, randomAlphanumeric, sha256 } from "../lib/crypto.js";

const ingestTokens = new Hono<AppType>();

/**
 * Generate a new ingest token with the tenant's key_prefix baked in.
 * Format: pt_{key_prefix}_{32 random chars}
 */
function generateIngestToken(keyPrefix: string): string {
  return `pt_${keyPrefix}_${randomAlphanumeric(32)}`;
}

/** POST /api/ingest-tokens — Issue a new ingest token. */
ingestTokens.post("/", async (c) => {
  const tenantId = c.get("tenantId");
  const isAdmin = c.get("isAdmin");
  const db = c.get("db");

  const body = await c.req.json<{
    tenant_id?: string;
    name?: string;
    plugin_id?: string;
    expires_in_days?: number;
  }>().catch(() => ({}));

  // Admin must specify tenant_id; regular tenants use their own
  let targetTenantId = tenantId;
  if (isAdmin) {
    if (!(body as any).tenant_id) {
      return c.json({ error: "tenant_id is required for admin token creation" }, 400);
    }
    targetTenantId = (body as any).tenant_id;
  }

  // Look up the tenant's key_prefix
  const tenant = await db.execute({
    sql: "SELECT key_prefix FROM tenants WHERE id = ?",
    args: [targetTenantId],
  });

  if (tenant.rows.length === 0) {
    return c.json({ error: "Tenant not found" }, 404);
  }

  const keyPrefix = tenant.rows[0].key_prefix as string;
  const id = generateId();
  const plainToken = generateIngestToken(keyPrefix);
  const tokenHash = await sha256(plainToken);
  const name = (body as any).name || "Default";
  const pluginId = (body as any).plugin_id || null;

  // Optional expiry
  let expiresAt: number | null = null;
  if ((body as any).expires_in_days) {
    expiresAt = Math.floor(Date.now() / 1000) + (body as any).expires_in_days * 86400;
  }

  await db.execute({
    sql: `INSERT INTO ingest_tokens (id, tenant_id, key_prefix, token_hash, name, plugin_id, expires_at)
          VALUES (?, ?, ?, ?, ?, ?, ?)`,
    args: [id, targetTenantId, keyPrefix, tokenHash, name, pluginId, expiresAt],
  });

  return c.json({
    id,
    token: plainToken,
    key_prefix: keyPrefix,
    name,
    plugin_id: pluginId,
    expires_at: expiresAt,
    tenant_id: targetTenantId,
    message: "Save this token — it will not be shown again.",
  }, 201);
});

/** GET /api/ingest-tokens — List ingest tokens. */
ingestTokens.get("/", async (c) => {
  const tenantId = c.get("tenantId");
  const isAdmin = c.get("isAdmin");
  const db = c.get("db");

  // Optional filter by key_prefix
  const keyPrefix = c.req.query("key_prefix");

  let result;
  if (isAdmin) {
    if (keyPrefix) {
      result = await db.execute({
        sql: `SELECT id, tenant_id, key_prefix, name, plugin_id, expires_at, revoked, last_used_at, created_at
              FROM ingest_tokens WHERE key_prefix = ? ORDER BY created_at DESC`,
        args: [keyPrefix],
      });
    } else {
      result = await db.execute(
        `SELECT id, tenant_id, key_prefix, name, plugin_id, expires_at, revoked, last_used_at, created_at
         FROM ingest_tokens ORDER BY created_at DESC`
      );
    }
  } else {
    result = await db.execute({
      sql: `SELECT id, key_prefix, name, plugin_id, expires_at, revoked, last_used_at, created_at
            FROM ingest_tokens WHERE tenant_id = ? ORDER BY created_at DESC`,
      args: [tenantId],
    });
  }

  return c.json({ tokens: result.rows });
});

/** DELETE /api/ingest-tokens/:id — Revoke an ingest token. */
ingestTokens.delete("/:id", async (c) => {
  const tenantId = c.get("tenantId");
  const isAdmin = c.get("isAdmin");
  const db = c.get("db");
  const id = c.req.param("id");

  let result;
  if (isAdmin) {
    result = await db.execute({
      sql: "UPDATE ingest_tokens SET revoked = 1 WHERE id = ?",
      args: [id],
    });
  } else {
    result = await db.execute({
      sql: "UPDATE ingest_tokens SET revoked = 1 WHERE id = ? AND tenant_id = ?",
      args: [id, tenantId],
    });
  }

  if (result.rowsAffected === 0) {
    return c.json({ error: "Ingest token not found" }, 404);
  }

  return c.json({ revoked: true });
});

export default ingestTokens;
