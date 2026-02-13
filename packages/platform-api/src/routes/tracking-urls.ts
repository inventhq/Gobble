/**
 * Tracking URL management routes.
 *
 * Naked link registry: maps a stable ID to a destination URL.
 * tracker-core resolves the ID at redirect time, enabling short URLs
 * and destination rotation without regenerating distributed links.
 *
 * No business logic — no names, tags, or status. Just ID → destination.
 */

import { Hono } from "hono";
import { Client } from "pg";
import { type AppType } from "../types.js";
import { generateTrackingUrlId } from "../lib/crypto.js";

const trackingUrls = new Hono<AppType>();

/** POST /api/tracking-urls — Register a new tracking URL. */
trackingUrls.post("/", async (c) => {
  const isAdmin = c.get("isAdmin");
  const db = c.get("db");
  const body = await c.req.json<{ destination: string; tenant_id?: string }>();

  // Admin must specify tenant_id; tenant-scoped keys use their own
  const tenantId = isAdmin ? body.tenant_id : c.get("tenantId");
  if (!tenantId || tenantId === "__admin__") {
    return c.json({ error: "tenant_id is required (admin must specify in body)" }, 400);
  }

  if (!body.destination || typeof body.destination !== "string") {
    return c.json({ error: "destination is required" }, 400);
  }

  try {
    new URL(body.destination);
  } catch {
    return c.json({ error: "Invalid destination URL format" }, 400);
  }

  const id = generateTrackingUrlId();

  await db.execute({
    sql: `INSERT INTO tracking_urls (id, tenant_id, destination) VALUES (?, ?, ?)`,
    args: [id, tenantId, body.destination],
  });

  return c.json({ id, destination: body.destination }, 201);
});

/**
 * GET /api/tracking-urls — List tracking URLs for the authenticated tenant.
 *
 * Returns each URL with aggregated event counts from recent_events
 * (joined via the tu_id param stored in the event's JSON params).
 */
trackingUrls.get("/", async (c) => {
  const isAdmin = c.get("isAdmin");
  const tenantId = c.get("tenantId");
  const db = c.get("db");

  const limit = Math.min(Number(c.req.query("limit") || "50"), 200);
  const offset = Number(c.req.query("offset") || "0");

  // Admin sees all; tenant sees own
  const result = isAdmin
    ? await db.execute({
        sql: `SELECT tu.id, tu.destination, tu.created_at, t.key_prefix
              FROM tracking_urls tu JOIN tenants t ON tu.tenant_id = t.id
              ORDER BY tu.created_at DESC LIMIT ? OFFSET ?`,
        args: [limit, offset],
      })
    : await db.execute({
        sql: `SELECT tu.id, tu.destination, tu.created_at, t.key_prefix
              FROM tracking_urls tu JOIN tenants t ON tu.tenant_id = t.id
              WHERE tu.tenant_id = ?
              ORDER BY tu.created_at DESC LIMIT ? OFFSET ?`,
        args: [tenantId, limit, offset],
      });

  const keyPrefix = !isAdmin && result.rows.length > 0
    ? (result.rows[0].key_prefix as string)
    : null;

  // Batch-fetch event counts per tracking URL
  // Use RisingWave stats_hourly_by_link MV if available, fall back to Turso
  const rwUrl = c.env.RISINGWAVE_URL;
  let rwCounts: Record<string, { clicks: number; postbacks: number; impressions: number }> = {};

  if (rwUrl) {
    let rw: Client | null = null;
    try {
      rw = new Client({ connectionString: rwUrl });
      await rw.connect();
      const tuIds = result.rows.map((r) => r.id as string);
      if (tuIds.length > 0) {
        const placeholders = tuIds.map((_, i) => `$${i + 1}`).join(", ");
        const countResult = await rw.query(
          `SELECT tu_id, event_type, SUM(count)::bigint AS total
           FROM stats_hourly_by_link
           WHERE tu_id IN (${placeholders})
           GROUP BY tu_id, event_type`,
          tuIds
        );
        for (const row of countResult.rows) {
          const tid = row.tu_id as string;
          if (!rwCounts[tid]) rwCounts[tid] = { clicks: 0, postbacks: 0, impressions: 0 };
          const total = Number(row.total);
          if (row.event_type === "click") rwCounts[tid].clicks = total;
          else if (row.event_type === "postback") rwCounts[tid].postbacks = total;
          else if (row.event_type === "impression") rwCounts[tid].impressions = total;
        }
      }
    } catch (e) {
      console.error("RisingWave tracking-url counts error:", e);
    } finally {
      if (rw) await rw.end().catch(() => {});
    }
  }

  const urls = await Promise.all(
    result.rows.map(async (row) => {
      const tuId = row.id as string;
      const rowPrefix = row.key_prefix as string;
      let clicks = 0, postbacks = 0, impressions = 0;

      if (rwCounts[tuId]) {
        clicks = rwCounts[tuId].clicks;
        postbacks = rwCounts[tuId].postbacks;
        impressions = rwCounts[tuId].impressions;
      } else {
        // Fallback to Turso recent_events
        const counts = await db.execute({
          sql: `SELECT event_type, COUNT(*) as cnt FROM recent_events
                WHERE tenant_id = ? AND params LIKE ?
                GROUP BY event_type`,
          args: [rowPrefix, `%"tu_id":"${tuId}"%`],
        });
        for (const cr of counts.rows) {
          const et = cr.event_type as string;
          const cnt = Number(cr.cnt);
          if (et === "click") clicks = cnt;
          else if (et === "postback") postbacks = cnt;
          else if (et === "impression") impressions = cnt;
        }
      }

      return {
        id: tuId,
        destination: row.destination,
        clicks,
        postbacks,
        impressions,
        created_at: row.created_at,
      };
    })
  );

  return c.json({ tracking_urls: urls, count: urls.length, limit, offset });
});

/** GET /api/tracking-urls/:id — Get a single tracking URL with event counts. */
trackingUrls.get("/:id", async (c) => {
  const isAdmin = c.get("isAdmin");
  const tenantId = c.get("tenantId");
  const db = c.get("db");
  const id = c.req.param("id");

  const result = isAdmin
    ? await db.execute({
        sql: `SELECT tu.id, tu.destination, tu.created_at, t.key_prefix
              FROM tracking_urls tu JOIN tenants t ON tu.tenant_id = t.id
              WHERE tu.id = ?`,
        args: [id],
      })
    : await db.execute({
        sql: `SELECT tu.id, tu.destination, tu.created_at, t.key_prefix
              FROM tracking_urls tu JOIN tenants t ON tu.tenant_id = t.id
              WHERE tu.id = ? AND tu.tenant_id = ?`,
        args: [id, tenantId],
      });

  if (result.rows.length === 0) {
    return c.json({ error: "Tracking URL not found" }, 404);
  }

  const row = result.rows[0];
  const rowPrefix = row.key_prefix as string;

  let clicks = 0, postbacks = 0, impressions = 0;
  const rwUrl = c.env.RISINGWAVE_URL;
  let rwFound = false;

  if (rwUrl) {
    let rw: Client | null = null;
    try {
      rw = new Client({ connectionString: rwUrl });
      await rw.connect();
      const countResult = await rw.query(
        `SELECT event_type, SUM(count)::bigint AS total
         FROM stats_hourly_by_link
         WHERE tu_id = $1
         GROUP BY event_type`,
        [id]
      );
      for (const cr of countResult.rows) {
        const total = Number(cr.total);
        if (cr.event_type === "click") clicks = total;
        else if (cr.event_type === "postback") postbacks = total;
        else if (cr.event_type === "impression") impressions = total;
      }
      rwFound = true;
    } catch (e) {
      console.error("RisingWave single TU counts error:", e);
    } finally {
      if (rw) await rw.end().catch(() => {});
    }
  }

  if (!rwFound) {
    const counts = await db.execute({
      sql: `SELECT event_type, COUNT(*) as cnt FROM recent_events
            WHERE tenant_id = ? AND params LIKE ?
            GROUP BY event_type`,
      args: [rowPrefix, `%"tu_id":"${id}"%`],
    });
    for (const cr of counts.rows) {
      const et = cr.event_type as string;
      const cnt = Number(cr.cnt);
      if (et === "click") clicks = cnt;
      else if (et === "postback") postbacks = cnt;
      else if (et === "impression") impressions = cnt;
    }
  }

  return c.json({
    id: row.id,
    destination: row.destination,
    clicks,
    postbacks,
    impressions,
    created_at: row.created_at,
  });
});

/** PATCH /api/tracking-urls/:id — Update destination (link rotation). */
trackingUrls.patch("/:id", async (c) => {
  const isAdmin = c.get("isAdmin");
  const tenantId = c.get("tenantId");
  const db = c.get("db");
  const id = c.req.param("id");
  const body = await c.req.json<{ destination: string }>();

  if (!body.destination || typeof body.destination !== "string") {
    return c.json({ error: "destination is required" }, 400);
  }

  try {
    new URL(body.destination);
  } catch {
    return c.json({ error: "Invalid destination URL format" }, 400);
  }

  const result = isAdmin
    ? await db.execute({
        sql: "UPDATE tracking_urls SET destination = ? WHERE id = ?",
        args: [body.destination, id],
      })
    : await db.execute({
        sql: "UPDATE tracking_urls SET destination = ? WHERE id = ? AND tenant_id = ?",
        args: [body.destination, id, tenantId],
      });

  if (result.rowsAffected === 0) {
    return c.json({ error: "Tracking URL not found" }, 404);
  }

  return c.json({ id, destination: body.destination, updated: true });
});

/** DELETE /api/tracking-urls/:id — Remove a tracking URL. */
trackingUrls.delete("/:id", async (c) => {
  const isAdmin = c.get("isAdmin");
  const tenantId = c.get("tenantId");
  const db = c.get("db");
  const id = c.req.param("id");

  const result = isAdmin
    ? await db.execute({
        sql: "DELETE FROM tracking_urls WHERE id = ?",
        args: [id],
      })
    : await db.execute({
        sql: "DELETE FROM tracking_urls WHERE id = ? AND tenant_id = ?",
        args: [id, tenantId],
      });

  if (result.rowsAffected === 0) {
    return c.json({ error: "Tracking URL not found" }, 404);
  }

  return c.json({ deleted: true });
});

export default trackingUrls;
