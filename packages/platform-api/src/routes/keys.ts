/**
 * API key management routes.
 *
 * Tenants create API keys to authenticate programmatic access.
 * Keys are hashed before storage — the plaintext is shown once on creation.
 */

import { Hono } from "hono";
import { type AppType } from "../types.js";
import { generateId, generateApiKey, sha256 } from "../lib/crypto.js";

const keys = new Hono<AppType>();

/** POST /api/keys — Create a new API key for the authenticated tenant. */
keys.post("/", async (c) => {
  const tenantId = c.get("tenantId");
  const isAdmin = c.get("isAdmin");

  // Admin must specify which tenant the key is for
  let targetTenantId = tenantId;
  if (isAdmin) {
    const body = await c.req.json<{ tenant_id: string; name?: string }>();
    if (!body.tenant_id) {
      return c.json({ error: "tenant_id is required for admin key creation" }, 400);
    }
    targetTenantId = body.tenant_id;
  }

  const body = await c.req.json<{ name?: string }>().catch(() => ({}));
  const db = c.get("db");
  const id = generateId();
  const plainKey = generateApiKey();
  const keyHash = await sha256(plainKey);
  const keyPrefix = plainKey.slice(0, 12); // "tk_live_xxxx" visible prefix
  const name = (body as { name?: string }).name || "Default";

  await db.execute({
    sql: `INSERT INTO api_keys (id, tenant_id, key_hash, key_prefix, name)
          VALUES (?, ?, ?, ?, ?)`,
    args: [id, targetTenantId, keyHash, keyPrefix, name],
  });

  // Return plaintext key — shown ONCE, never stored
  return c.json({
    id,
    key: plainKey,
    key_prefix: keyPrefix,
    name,
    tenant_id: targetTenantId,
    message: "Save this key — it will not be shown again.",
  }, 201);
});

/** GET /api/keys — List API keys for the authenticated tenant. */
keys.get("/", async (c) => {
  const tenantId = c.get("tenantId");
  const isAdmin = c.get("isAdmin");
  const db = c.get("db");

  let result;
  if (isAdmin) {
    result = await db.execute(
      "SELECT id, tenant_id, key_prefix, name, last_used_at, created_at FROM api_keys ORDER BY created_at DESC"
    );
  } else {
    result = await db.execute({
      sql: "SELECT id, key_prefix, name, last_used_at, created_at FROM api_keys WHERE tenant_id = ? ORDER BY created_at DESC",
      args: [tenantId],
    });
  }

  return c.json({ keys: result.rows });
});

/** DELETE /api/keys/:id — Revoke an API key. */
keys.delete("/:id", async (c) => {
  const tenantId = c.get("tenantId");
  const isAdmin = c.get("isAdmin");
  const db = c.get("db");
  const id = c.req.param("id");

  let result;
  if (isAdmin) {
    result = await db.execute({
      sql: "DELETE FROM api_keys WHERE id = ?",
      args: [id],
    });
  } else {
    result = await db.execute({
      sql: "DELETE FROM api_keys WHERE id = ? AND tenant_id = ?",
      args: [id, tenantId],
    });
  }

  if (result.rowsAffected === 0) {
    return c.json({ error: "API key not found" }, 404);
  }

  return c.json({ deleted: true });
});

export default keys;
