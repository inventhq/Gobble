/**
 * API key authentication middleware.
 *
 * Extracts the Bearer token from the Authorization header, hashes it,
 * and looks up the matching API key in Turso. Attaches the tenant_id
 * to the Hono context for downstream handlers.
 *
 * Also supports a bootstrap admin key for initial setup (creating the
 * first tenant before any API keys exist).
 */

import { createMiddleware } from "hono/factory";
import { type AppType } from "../types.js";
import { sha256 } from "../lib/crypto.js";

/**
 * Create the auth middleware. Requires the libSQL client to be set
 * in the Hono context variable "db" before this middleware runs.
 */
export function authMiddleware() {
  return createMiddleware<AppType>(async (c, next) => {
    const authHeader = c.req.header("Authorization");
    if (!authHeader?.startsWith("Bearer ")) {
      return c.json({ error: "Missing Authorization: Bearer <api_key>" }, 401);
    }

    const token = authHeader.slice(7);

    // Check bootstrap admin key (for initial setup only)
    const adminKey = c.env.ADMIN_BOOTSTRAP_KEY;
    if (adminKey && token === adminKey) {
      c.set("tenantId", "__admin__");
      c.set("isAdmin", true);
      return next();
    }

    // Hash the token and look up in DB
    const keyHash = await sha256(token);
    const db = c.get("db");

    const result = await db.execute({
      sql: "SELECT tenant_id, scopes FROM api_keys WHERE key_hash = ?",
      args: [keyHash],
    });

    if (result.rows.length === 0) {
      return c.json({ error: "Invalid API key" }, 401);
    }

    const row = result.rows[0];
    c.set("tenantId", String(row.tenant_id));
    c.set("isAdmin", false);

    // Update last_used_at (fire-and-forget, don't block the response)
    db.execute({
      sql: "UPDATE api_keys SET last_used_at = unixepoch() WHERE key_hash = ?",
      args: [keyHash],
    }).catch(() => {});

    return next();
  });
}

/**
 * Require admin access. Use after authMiddleware().
 * Only the bootstrap admin key grants admin access.
 */
export function requireAdmin() {
  return createMiddleware<AppType>(async (c, next) => {
    if (!c.get("isAdmin")) {
      return c.json({ error: "Admin access required" }, 403);
    }
    return next();
  });
}
