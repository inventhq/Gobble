/**
 * Webhook management routes.
 *
 * Tenants register webhook endpoints to receive real-time HTTP
 * notifications when tracking events occur. Supports filtering
 * by event type and per-webhook signing secrets.
 */

import { Hono } from "hono";
import { type AppType } from "../types.js";
import { generateId, generateWebhookSecret } from "../lib/crypto.js";

const webhooks = new Hono<AppType>();

/** POST /api/webhooks — Register a new webhook endpoint. */
webhooks.post("/", async (c) => {
  const tenantId = c.get("tenantId");
  const db = c.get("db");
  const body = await c.req.json<{
    url: string;
    event_types?: string[];
    filter_param_key?: string;
    filter_param_value?: string;
  }>();

  if (!body.url || typeof body.url !== "string") {
    return c.json({ error: "url is required" }, 400);
  }

  // Validate URL format
  try {
    new URL(body.url);
  } catch {
    return c.json({ error: "Invalid URL format" }, 400);
  }

  const id = generateId();
  const eventTypes = JSON.stringify(body.event_types || ["*"]);
  const secret = generateWebhookSecret();
  const filterParamKey = body.filter_param_key || null;
  const filterParamValue = body.filter_param_value || null;

  await db.execute({
    sql: `INSERT INTO webhooks (id, tenant_id, url, event_types, secret, filter_param_key, filter_param_value)
          VALUES (?, ?, ?, ?, ?, ?, ?)`,
    args: [id, tenantId, body.url, eventTypes, secret, filterParamKey, filterParamValue],
  });

  return c.json({
    id,
    url: body.url,
    event_types: body.event_types || ["*"],
    filter_param_key: filterParamKey,
    filter_param_value: filterParamValue,
    secret,
    active: true,
    message: "Save the webhook secret — it will not be shown again.",
  }, 201);
});

/** GET /api/webhooks — List webhooks for the authenticated tenant. */
webhooks.get("/", async (c) => {
  const tenantId = c.get("tenantId");
  const db = c.get("db");

  const result = await db.execute({
    sql: "SELECT id, url, event_types, active, filter_param_key, filter_param_value, created_at FROM webhooks WHERE tenant_id = ? ORDER BY created_at DESC",
    args: [tenantId],
  });

  return c.json({
    webhooks: result.rows.map((row) => ({
      ...row,
      event_types: JSON.parse(row.event_types as string),
      filter_param_key: row.filter_param_key || null,
      filter_param_value: row.filter_param_value || null,
    })),
  });
});

/** PATCH /api/webhooks/:id — Update a webhook. */
webhooks.patch("/:id", async (c) => {
  const tenantId = c.get("tenantId");
  const db = c.get("db");
  const id = c.req.param("id");
  const body = await c.req.json<{
    url?: string;
    event_types?: string[];
    active?: boolean;
    filter_param_key?: string | null;
    filter_param_value?: string | null;
  }>();

  const updates: string[] = [];
  const args: (string | number | null)[] = [];

  if (body.url) {
    try {
      new URL(body.url);
    } catch {
      return c.json({ error: "Invalid URL format" }, 400);
    }
    updates.push("url = ?");
    args.push(body.url);
  }
  if (body.event_types) {
    updates.push("event_types = ?");
    args.push(JSON.stringify(body.event_types));
  }
  if (body.active !== undefined) {
    updates.push("active = ?");
    args.push(body.active ? 1 : 0);
  }
  if (body.filter_param_key !== undefined) {
    updates.push("filter_param_key = ?");
    args.push(body.filter_param_key);
  }
  if (body.filter_param_value !== undefined) {
    updates.push("filter_param_value = ?");
    args.push(body.filter_param_value);
  }

  if (updates.length === 0) {
    return c.json({ error: "Nothing to update" }, 400);
  }

  args.push(id, tenantId);
  const result = await db.execute({
    sql: `UPDATE webhooks SET ${updates.join(", ")} WHERE id = ? AND tenant_id = ?`,
    args,
  });

  if (result.rowsAffected === 0) {
    return c.json({ error: "Webhook not found" }, 404);
  }

  return c.json({ updated: true });
});

/** DELETE /api/webhooks/:id — Remove a webhook. */
webhooks.delete("/:id", async (c) => {
  const tenantId = c.get("tenantId");
  const db = c.get("db");
  const id = c.req.param("id");

  const result = await db.execute({
    sql: "DELETE FROM webhooks WHERE id = ? AND tenant_id = ?",
    args: [id, tenantId],
  });

  if (result.rowsAffected === 0) {
    return c.json({ error: "Webhook not found" }, 404);
  }

  return c.json({ deleted: true });
});

/** POST /api/webhooks/:id/test — Send a test event to the webhook. */
webhooks.post("/:id/test", async (c) => {
  const tenantId = c.get("tenantId");
  const db = c.get("db");
  const id = c.req.param("id");

  const result = await db.execute({
    sql: "SELECT url, secret FROM webhooks WHERE id = ? AND tenant_id = ?",
    args: [id, tenantId],
  });

  if (result.rows.length === 0) {
    return c.json({ error: "Webhook not found" }, 404);
  }

  const webhook = result.rows[0];
  const testPayload = {
    event_id: "test_" + Date.now(),
    event_type: "test",
    timestamp: Date.now(),
    ip: "127.0.0.1",
    user_agent: "tracker-platform-api/test",
    referer: null,
    accept_language: null,
    request_path: "/test",
    request_host: "platform-api",
    params: { test: "true" },
  };

  try {
    const response = await fetch(webhook.url as string, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "X-Webhook-Secret": String(webhook.secret),
      },
      body: JSON.stringify(testPayload),
    });

    return c.json({
      delivered: true,
      status_code: response.status,
      url: webhook.url,
    });
  } catch (error) {
    return c.json({
      delivered: false,
      error: error instanceof Error ? error.message : "Unknown error",
      url: webhook.url,
    });
  }
});

export default webhooks;
