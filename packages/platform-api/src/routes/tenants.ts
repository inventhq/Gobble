/**
 * Tenant management routes.
 *
 * Admin-only endpoints for creating and managing tenant accounts.
 * Each tenant gets a unique key_prefix, hmac_secret, and encryption_key.
 */

import { Hono } from "hono";
import { type AppType } from "../types.js";
import {
  generateId,
  generateKeyPrefix,
  generateHmacSecret,
  generateEncryptionKey,
} from "../lib/crypto.js";
import { requireAdmin } from "../middleware/auth.js";
import { syncUserToPermit } from "../lib/permit.js";

const tenants = new Hono<AppType>();

// All tenant routes require admin access
tenants.use("/*", requireAdmin());

/** POST /api/tenants — Create a new tenant. */
tenants.post("/", async (c) => {
  const body = await c.req.json<{ name: string; plan?: string; email?: string }>();

  if (!body.name || typeof body.name !== "string") {
    return c.json({ error: "name is required" }, 400);
  }

  const db = c.get("db");
  const id = generateId();
  const plan = body.plan || "free";
  const email = body.email || null;
  const hmacSecret = generateHmacSecret();
  const encryptionKey = generateEncryptionKey();

  // Generate a unique key_prefix (retry on collision)
  let keyPrefix: string;
  for (let attempt = 0; attempt < 10; attempt++) {
    keyPrefix = generateKeyPrefix();
    const existing = await db.execute({
      sql: "SELECT 1 FROM tenants WHERE key_prefix = ?",
      args: [keyPrefix],
    });
    if (existing.rows.length === 0) break;
  }

  await db.execute({
    sql: `INSERT INTO tenants (id, name, plan, key_prefix, hmac_secret, encryption_key, email)
          VALUES (?, ?, ?, ?, ?, ?, ?)`,
    args: [id, body.name, plan, keyPrefix!, hmacSecret, encryptionKey, email],
  });

  // Auto-provision user in Permit.io with "tenant" role
  let permitSync: { success: boolean; error?: string } | null = null;
  const permitKey = c.env.PERMIT_API_KEY;
  if (permitKey && email) {
    permitSync = await syncUserToPermit(permitKey, email, "tenant");
    if (!permitSync.success) {
      console.error(`Permit.io sync failed for ${email}:`, permitSync.error);
    }
  }

  // Return secrets — shown once, like Stripe
  return c.json({
    id,
    name: body.name,
    plan,
    email,
    key_prefix: keyPrefix!,
    hmac_secret: hmacSecret,
    encryption_key: encryptionKey,
    permit_synced: permitSync?.success ?? null,
    created_at: Math.floor(Date.now() / 1000),
  }, 201);
});

/** GET /api/tenants — List all tenants. */
tenants.get("/", async (c) => {
  const db = c.get("db");
  const result = await db.execute(
    "SELECT id, name, plan, email, key_prefix, rate_limit_rps, created_at FROM tenants ORDER BY created_at DESC"
  );

  return c.json({
    tenants: result.rows.map((row) => ({
      id: row.id,
      name: row.name,
      plan: row.plan,
      email: row.email,
      key_prefix: row.key_prefix,
      rate_limit_rps: row.rate_limit_rps,
      created_at: row.created_at,
    })),
  });
});

/** GET /api/tenants/:id — Get tenant details (no secrets). */
tenants.get("/:id", async (c) => {
  const db = c.get("db");
  const id = c.req.param("id");

  const result = await db.execute({
    sql: "SELECT id, name, plan, email, key_prefix, rate_limit_rps, created_at FROM tenants WHERE id = ?",
    args: [id],
  });

  if (result.rows.length === 0) {
    return c.json({ error: "Tenant not found" }, 404);
  }

  return c.json(result.rows[0]);
});

/** PATCH /api/tenants/:id — Update tenant name, plan, or email. */
tenants.patch("/:id", async (c) => {
  const db = c.get("db");
  const id = c.req.param("id");
  const body = await c.req.json<{ name?: string; plan?: string; email?: string; rate_limit_rps?: number }>();

  const updates: string[] = [];
  const args: (string | number)[] = [];

  if (body.name) {
    updates.push("name = ?");
    args.push(body.name);
  }
  if (body.plan) {
    updates.push("plan = ?");
    args.push(body.plan);
  }
  if (body.email) {
    updates.push("email = ?");
    args.push(body.email);
  }
  if (typeof body.rate_limit_rps === "number" && body.rate_limit_rps > 0) {
    updates.push("rate_limit_rps = ?");
    args.push(body.rate_limit_rps);
  }

  if (updates.length === 0) {
    return c.json({ error: "Nothing to update" }, 400);
  }

  args.push(id);
  await db.execute({
    sql: `UPDATE tenants SET ${updates.join(", ")} WHERE id = ?`,
    args,
  });

  // If email was updated and Permit.io is configured, sync the user
  if (body.email) {
    const permitKey = c.env.PERMIT_API_KEY;
    if (permitKey) {
      const result = await syncUserToPermit(permitKey, body.email, "tenant");
      if (!result.success) {
        console.error(`Permit.io sync failed for ${body.email}:`, result.error);
      }
    }
  }

  return c.json({ updated: true });
});

/** POST /api/tenants/:id/rotate-secrets — Rotate HMAC and encryption keys. */
tenants.post("/:id/rotate-secrets", async (c) => {
  const db = c.get("db");
  const id = c.req.param("id");

  const hmacSecret = generateHmacSecret();
  const encryptionKey = generateEncryptionKey();

  const result = await db.execute({
    sql: "UPDATE tenants SET hmac_secret = ?, encryption_key = ? WHERE id = ?",
    args: [hmacSecret, encryptionKey, id],
  });

  if (result.rowsAffected === 0) {
    return c.json({ error: "Tenant not found" }, 404);
  }

  return c.json({
    hmac_secret: hmacSecret,
    encryption_key: encryptionKey,
    message: "Secrets rotated. Update your SDK configuration.",
  });
});

export default tenants;
