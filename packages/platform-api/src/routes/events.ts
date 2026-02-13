/**
 * Event and stats query routes.
 *
 * Queries RisingWave (via Postgres wire protocol) for event data and
 * materialized view stats. Falls back to Turso if RISINGWAVE_URL is
 * not configured. Turso is still used for tenant key_prefix resolution.
 */

import { Hono } from "hono";
import { Client } from "pg";
import { type AppType } from "../types.js";

const events = new Hono<AppType>();

/** Escape single quotes for SQL string literals. */
function escapeSql(s: string): string {
  return s.replace(/'/g, "''");
}

/** Get or create a RisingWave pg Client for this request. */
async function getRwClient(connectionString: string): Promise<Client> {
  const client = new Client({ connectionString });
  await client.connect();
  return client;
}

/**
 * Resolve tenant key_prefix from Turso (used as tenant_id in RisingWave).
 * Returns null for admin (sees all tenants).
 */
async function resolveKeyPrefix(
  db: any,
  tenantId: string,
  isAdmin: boolean
): Promise<string | null> {
  if (isAdmin) return null;
  const tenant = await db.execute({
    sql: "SELECT key_prefix FROM tenants WHERE id = ?",
    args: [tenantId],
  });
  if (tenant.rows.length === 0) return undefined as any;
  return tenant.rows[0].key_prefix as string;
}

/**
 * Resolve tenant key_prefix and plan from Turso.
 * Returns { keyPrefix, plan } or undefined if tenant not found.
 * Admin gets { keyPrefix: null, plan: "admin" }.
 */
async function resolveTenantInfo(
  db: any,
  tenantId: string,
  isAdmin: boolean
): Promise<{ keyPrefix: string | null; plan: string } | undefined> {
  if (isAdmin) return { keyPrefix: null, plan: "admin" };
  const tenant = await db.execute({
    sql: "SELECT key_prefix, plan FROM tenants WHERE id = ?",
    args: [tenantId],
  });
  if (tenant.rows.length === 0) return undefined;
  return {
    keyPrefix: tenant.rows[0].key_prefix as string,
    plan: (tenant.rows[0].plan as string) || "free",
  };
}

/**
 * GET /api/events — List recent events for the authenticated tenant.
 *
 * Query params:
 *   - limit (default 50, max 200)
 *   - offset (default 0)
 *   - event_type (optional filter: "click", "postback", "impression")
 *   - tu_id (optional filter: tracking URL ID)
 *   - since (optional, Unix ms timestamp) — only return events newer than this
 *   - param_key (optional) — filter by a specific param key
 *   - param_value (optional) — required with param_key, match exact value
 *
 * Response includes `server_time` (Unix ms) for use as the next `since` value.
 */
events.get("/", async (c) => {
  const tenantId = c.get("tenantId");
  const db = c.get("db");
  const isAdmin = c.get("isAdmin");
  const rwUrl = c.env.RISINGWAVE_URL;

  const limit = Math.min(Number(c.req.query("limit") || "50"), 200);
  const offset = Number(c.req.query("offset") || "0");
  const eventType = c.req.query("event_type");
  const tuId = c.req.query("tu_id");
  const since = c.req.query("since") ? Number(c.req.query("since")) : null;
  const paramKey = c.req.query("param_key");
  const paramValue = c.req.query("param_value");

  // Resolve tenant key_prefix from Turso
  const keyPrefix = await resolveKeyPrefix(db, tenantId, isAdmin);
  if (keyPrefix === undefined) {
    return c.json({ error: "Tenant not found" }, 404);
  }

  // If no RisingWave URL, fall back to Turso (backward compatible)
  if (!rwUrl) {
    return tursoEventsQuery(c, db, keyPrefix, eventType, tuId, since, limit, offset, paramKey, paramValue);
  }

  // Build SQL with inline values (RisingWave has limited $N param support)
  const conditions: string[] = [];

  if (keyPrefix) {
    conditions.push(`tenant_id = '${escapeSql(keyPrefix)}'`);
  }
  if (eventType) {
    conditions.push(`event_type = '${escapeSql(eventType)}'`);
  }
  if (tuId) {
    conditions.push(`params->>'tu_id' = '${escapeSql(tuId)}'`);
  }
  if (since) {
    conditions.push(`timestamp_ms > ${Number(since)}`);
  }
  if (paramKey && paramValue) {
    conditions.push(`params->>'${escapeSql(paramKey)}' = '${escapeSql(paramValue)}'`);
  } else if (paramKey) {
    conditions.push(`params->>'${escapeSql(paramKey)}' IS NOT NULL`);
  }

  const where = conditions.length > 0 ? `WHERE ${conditions.join(" AND ")}` : "";

  let rw: Client | null = null;
  try {
    rw = await getRwClient(rwUrl);
    const result = await rw.query(
      `SELECT event_id, event_type, timestamp_ms as timestamp, ip, user_agent, referer, request_path, request_host, params
       FROM events ${where}
       ORDER BY timestamp_ms DESC, event_id DESC
       LIMIT ${limit} OFFSET ${offset}`
    );

    return c.json({
      events: result.rows.map((row: any) => ({
        ...row,
        timestamp: Number(row.timestamp),
        params: row.params || {},
      })),
      count: result.rows.length,
      limit,
      offset,
      server_time: Date.now(),
    });
  } catch (e) {
    console.error("RisingWave query error:", e);
    return c.json({ error: "Failed to query events" }, 500);
  } finally {
    if (rw) await rw.end().catch(() => {});
  }
});

/**
 * GET /api/stats — Get aggregated stats for the authenticated tenant.
 *
 * Query params:
 *   - hours (default 24, max 168 = 7 days) — how many hours back to query
 *   - event_type (optional filter)
 *   - tu_id (optional filter: tracking URL ID)
 *   - param_key (optional) — filter by a specific param key
 *   - param_value (optional) — required with param_key, match exact value
 *   - group_by (optional) — "param:<key>" to break down stats by param values
 *
 * Returns hourly buckets with counts, plus a summary total.
 * When group_by is used, returns a `breakdown` array instead of hourly/summary.
 */
events.get("/stats", async (c) => {
  const tenantId = c.get("tenantId");
  const db = c.get("db");
  const isAdmin = c.get("isAdmin");
  const rwUrl = c.env.RISINGWAVE_URL;

  const hours = Math.min(Number(c.req.query("hours") || "24"), 168);
  const eventType = c.req.query("event_type");
  const tuId = c.req.query("tu_id");
  const paramKey = c.req.query("param_key");
  const paramValue = c.req.query("param_value");
  const groupBy = c.req.query("group_by");

  // Resolve tenant key_prefix from Turso
  const keyPrefix = await resolveKeyPrefix(db, tenantId, isAdmin);
  if (keyPrefix === undefined) {
    return c.json({ error: "Tenant not found" }, 404);
  }

  const nowSecs = Math.floor(Date.now() / 1000);
  const currentHour = nowSecs - (nowSecs % 3600);
  const cutoffHour = currentHour - hours * 3600;

  // If no RisingWave URL, fall back to Turso
  if (!rwUrl) {
    return tursoStatsQuery(c, db, keyPrefix, eventType, cutoffHour, currentHour, hours);
  }

  // Parse group_by=param:<key>
  const groupByParam = groupBy?.startsWith("param:") ? groupBy.slice(6) : null;

  // If grouping by param, query raw events table (no MV for arbitrary params)
  if (groupByParam) {
    return rwParamGroupByQuery(
      c, rwUrl, keyPrefix, eventType, tuId, paramKey, paramValue,
      cutoffHour, currentHour, hours, groupByParam
    );
  }

  // Choose MV: per-link or global
  const statsTable = tuId ? "stats_hourly_by_link" : "stats_hourly";

  // Build SQL with inline values (RisingWave has limited $N param support)
  const conditions: string[] = [];

  if (keyPrefix) {
    conditions.push(`tenant_id = '${escapeSql(keyPrefix)}'`);
  }
  if (tuId) {
    conditions.push(`tu_id = '${escapeSql(tuId)}'`);
  }
  if (eventType) {
    conditions.push(`event_type = '${escapeSql(eventType)}'`);
  }
  if (paramKey && paramValue) {
    // For MV-based stats, we need to fall back to raw events table
    return rwParamFilteredStatsQuery(
      c, rwUrl, keyPrefix, eventType, tuId, paramKey, paramValue,
      cutoffHour, currentHour, hours
    );
  } else if (paramKey) {
    return rwParamFilteredStatsQuery(
      c, rwUrl, keyPrefix, eventType, tuId, paramKey, undefined,
      cutoffHour, currentHour, hours
    );
  }
  conditions.push(`hour >= ${cutoffHour}`);

  const where = `WHERE ${conditions.join(" AND ")}`;

  let rw: Client | null = null;
  try {
    rw = await getRwClient(rwUrl);

    // Hourly breakdown from materialized view
    const result = await rw.query(
      `SELECT event_type, hour, count
       FROM ${statsTable} ${where}
       ORDER BY hour DESC`
    );

    // Summary totals
    const summary = await rw.query(
      `SELECT event_type, SUM(count) as total
       FROM ${statsTable} ${where}
       GROUP BY event_type`
    );

    return c.json({
      hourly: result.rows.map((r: any) => ({ ...r, hour: Number(r.hour), count: Number(r.count) })),
      summary: summary.rows.map((r: any) => ({ ...r, total: Number(r.total) })),
      hours,
      from_hour: cutoffHour,
      to_hour: currentHour,
      server_time: Date.now(),
    });
  } catch (e) {
    console.error("RisingWave stats query error:", e);
    return c.json({ error: "Failed to query stats" }, 500);
  } finally {
    if (rw) await rw.end().catch(() => {});
  }
});

// --- RisingWave param-filtered stats (queries raw events table) ---

/**
 * Stats query with param_key/param_value filter.
 * Cannot use materialized views since params are arbitrary — queries raw events table.
 */
async function rwParamFilteredStatsQuery(
  c: any, rwUrl: string, keyPrefix: string | null,
  eventType: string | undefined, tuId: string | undefined,
  paramKey: string, paramValue: string | undefined,
  cutoffHour: number, currentHour: number, hours: number
) {
  const conditions: string[] = [];

  if (keyPrefix) conditions.push(`tenant_id = '${escapeSql(keyPrefix)}'`);
  if (eventType) conditions.push(`event_type = '${escapeSql(eventType)}'`);
  if (tuId) conditions.push(`params->>'tu_id' = '${escapeSql(tuId)}'`);
  if (paramValue) {
    conditions.push(`params->>'${escapeSql(paramKey)}' = '${escapeSql(paramValue)}'`);
  } else {
    conditions.push(`params->>'${escapeSql(paramKey)}' IS NOT NULL`);
  }
  conditions.push(`timestamp_ms >= ${cutoffHour * 1000}`);

  const where = `WHERE ${conditions.join(" AND ")}`;

  let rw: Client | null = null;
  try {
    rw = await getRwClient(rwUrl);

    const result = await rw.query(
      `SELECT event_type,
              (EXTRACT(EPOCH FROM date_trunc('hour', to_timestamp(timestamp_ms / 1000.0))))::bigint AS hour,
              COUNT(*)::bigint AS count
       FROM events ${where}
       GROUP BY event_type, hour
       ORDER BY hour DESC`
    );

    const summary = await rw.query(
      `SELECT event_type, COUNT(*)::bigint AS total
       FROM events ${where}
       GROUP BY event_type`
    );

    return c.json({
      hourly: result.rows.map((r: any) => ({ ...r, hour: Number(r.hour), count: Number(r.count) })),
      summary: summary.rows.map((r: any) => ({ ...r, total: Number(r.total) })),
      hours,
      from_hour: cutoffHour,
      to_hour: currentHour,
      server_time: Date.now(),
      filters: { param_key: paramKey, param_value: paramValue || null },
    });
  } catch (e) {
    console.error("RisingWave param stats query error:", e);
    return c.json({ error: "Failed to query param stats" }, 500);
  } finally {
    if (rw) await rw.end().catch(() => {});
  }
}

/**
 * Stats grouped by a param key's values.
 * Returns a breakdown array: [{ param_value, event_type, total }]
 */
async function rwParamGroupByQuery(
  c: any, rwUrl: string, keyPrefix: string | null,
  eventType: string | undefined, tuId: string | undefined,
  paramKey: string | undefined, paramValue: string | undefined,
  cutoffHour: number, currentHour: number, hours: number,
  groupByParam: string
) {
  const conditions: string[] = [];

  if (keyPrefix) conditions.push(`tenant_id = '${escapeSql(keyPrefix)}'`);
  if (eventType) conditions.push(`event_type = '${escapeSql(eventType)}'`);
  if (tuId) conditions.push(`params->>'tu_id' = '${escapeSql(tuId)}'`);
  if (paramKey && paramValue) {
    conditions.push(`params->>'${escapeSql(paramKey)}' = '${escapeSql(paramValue)}'`);
  }
  conditions.push(`params->>'${escapeSql(groupByParam)}' IS NOT NULL`);
  conditions.push(`timestamp_ms >= ${cutoffHour * 1000}`);

  const where = `WHERE ${conditions.join(" AND ")}`;

  let rw: Client | null = null;
  try {
    rw = await getRwClient(rwUrl);

    const result = await rw.query(
      `SELECT params->>'${escapeSql(groupByParam)}' AS param_value,
              event_type,
              COUNT(*)::bigint AS total
       FROM events ${where}
       GROUP BY param_value, event_type
       ORDER BY total DESC
       LIMIT 100`
    );

    return c.json({
      breakdown: result.rows.map((r: any) => ({ ...r, total: Number(r.total) })),
      group_by: `param:${groupByParam}`,
      hours,
      from_hour: cutoffHour,
      to_hour: currentHour,
      server_time: Date.now(),
    });
  } catch (e) {
    console.error("RisingWave param group-by query error:", e);
    return c.json({ error: "Failed to query param breakdown" }, 500);
  } finally {
    if (rw) await rw.end().catch(() => {});
  }
}

/**
 * GET /api/events/match — Generic event-pair matching.
 *
 * Joins two event types by a shared param key. Business-agnostic — the API
 * doesn't know what "click_id" or "conversion" means. It just finds pairs
 * of events that share a value for a given key and computes the time delta.
 *
 * Query params:
 *   - trigger (default "click") — the event_type of the first event
 *   - result  (default "postback") — the event_type of the second event
 *   - on      (default "click_id") — the param key that links trigger→result
 *   - tu_id (optional) — filter by tracking URL ID
 *   - hours (default 24, max 168) — how far back to look
 *   - limit (default 50, max 200)
 *   - offset (default 0)
 *   - param_key / param_value (optional) — additional param filter
 *
 * Returns pairs: { trigger_event, result_event, on, on_value, time_delta_ms, matched }
 * and a summary: { total_triggers, matched, unmatched, match_rate }.
 */
events.get("/match", async (c) => {
  const tenantId = c.get("tenantId");
  const db = c.get("db");
  const isAdmin = c.get("isAdmin");
  const rwUrl = c.env.RISINGWAVE_URL;

  const triggerType = c.req.query("trigger") || "click";
  const resultType = c.req.query("result") || "postback";
  const onKey = c.req.query("on") || "click_id";
  const tuId = c.req.query("tu_id");
  const hours = Math.min(Number(c.req.query("hours") || "24"), 168);
  const limit = Math.min(Number(c.req.query("limit") || "50"), 200);
  const offset = Number(c.req.query("offset") || "0");
  const paramKey = c.req.query("param_key");
  const paramValue = c.req.query("param_value");

  const keyPrefix = await resolveKeyPrefix(db, tenantId, isAdmin);
  if (keyPrefix === undefined) {
    return c.json({ error: "Tenant not found" }, 404);
  }

  if (!rwUrl) {
    return c.json({ error: "Match endpoint requires RisingWave" }, 501);
  }

  const cutoffMs = Date.now() - hours * 3600 * 1000;

  // Build conditions for the trigger (t.) side
  const triggerConditions: string[] = [];
  if (keyPrefix) triggerConditions.push(`t.tenant_id = '${escapeSql(keyPrefix)}'`);
  if (tuId) triggerConditions.push(`t.params->>'tu_id' = '${escapeSql(tuId)}'`);
  if (paramKey && paramValue) {
    triggerConditions.push(`t.params->>'${escapeSql(paramKey)}' = '${escapeSql(paramValue)}'`);
  } else if (paramKey) {
    triggerConditions.push(`t.params->>'${escapeSql(paramKey)}' IS NOT NULL`);
  }
  triggerConditions.push(`t.timestamp_ms >= ${cutoffMs}`);
  triggerConditions.push(`t.params->>'${escapeSql(onKey)}' IS NOT NULL`);

  const triggerWhere = triggerConditions.join(" AND ");

  let rw: Client | null = null;
  try {
    rw = await getRwClient(rwUrl);

    // LEFT JOIN trigger events to result events on the shared param key
    const sql = `
      SELECT
        t.event_id     AS trigger_event_id,
        t.timestamp_ms AS trigger_timestamp,
        t.ip           AS trigger_ip,
        t.user_agent   AS trigger_user_agent,
        t.params       AS trigger_params,
        r.event_id     AS result_event_id,
        r.timestamp_ms AS result_timestamp,
        r.params       AS result_params,
        t.params->>'${escapeSql(onKey)}' AS on_value
      FROM events t
      LEFT JOIN events r
        ON r.event_type = '${escapeSql(resultType)}'
        AND r.params->>'${escapeSql(onKey)}' = t.params->>'${escapeSql(onKey)}'
        ${keyPrefix ? `AND r.tenant_id = '${escapeSql(keyPrefix)}'` : ""}
        AND r.timestamp_ms >= ${cutoffMs}
      WHERE t.event_type = '${escapeSql(triggerType)}'
        AND ${triggerWhere}
      ORDER BY t.timestamp_ms DESC
      LIMIT ${limit} OFFSET ${offset}
    `;

    const result = await rw.query(sql);

    // Summary stats
    const summarySql = `
      SELECT
        COUNT(DISTINCT t.params->>'${escapeSql(onKey)}') AS total_triggers,
        COUNT(DISTINCT CASE WHEN r.event_id IS NOT NULL
          THEN t.params->>'${escapeSql(onKey)}' END) AS matched,
        COUNT(DISTINCT CASE WHEN r.event_id IS NULL
          THEN t.params->>'${escapeSql(onKey)}' END) AS unmatched
      FROM events t
      LEFT JOIN events r
        ON r.event_type = '${escapeSql(resultType)}'
        AND r.params->>'${escapeSql(onKey)}' = t.params->>'${escapeSql(onKey)}'
        ${keyPrefix ? `AND r.tenant_id = '${escapeSql(keyPrefix)}'` : ""}
        AND r.timestamp_ms >= ${cutoffMs}
      WHERE t.event_type = '${escapeSql(triggerType)}'
        AND ${triggerWhere}
    `;

    const summaryResult = await rw.query(summarySql);
    const summary = summaryResult.rows[0] || { total_triggers: 0, matched: 0, unmatched: 0 };

    const pairs = result.rows.map((row: any) => {
      const triggerTs = Number(row.trigger_timestamp);
      const resultTs = row.result_timestamp ? Number(row.result_timestamp) : null;
      return {
        on: onKey,
        on_value: row.on_value,
        trigger_event: {
          event_id: row.trigger_event_id,
          timestamp: triggerTs,
          ip: row.trigger_ip,
          user_agent: row.trigger_user_agent,
          params: row.trigger_params || {},
        },
        result_event: row.result_event_id
          ? {
              event_id: row.result_event_id,
              timestamp: resultTs,
              params: row.result_params || {},
            }
          : null,
        time_delta_ms: resultTs ? resultTs - triggerTs : null,
        matched: !!row.result_event_id,
      };
    });

    return c.json({
      pairs,
      summary: {
        total_triggers: Number(summary.total_triggers),
        matched: Number(summary.matched),
        unmatched: Number(summary.unmatched),
        match_rate: Number(summary.total_triggers) > 0
          ? Number(summary.matched) / Number(summary.total_triggers)
          : 0,
      },
      trigger: triggerType,
      result: resultType,
      on: onKey,
      hours,
      limit,
      offset,
      server_time: Date.now(),
    });
  } catch (e) {
    console.error("RisingWave match query error:", e);
    return c.json({ error: "Failed to query event matches" }, 500);
  } finally {
    if (rw) await rw.end().catch(() => {});
  }
});

// --- Polars cold/warm storage query router ---

/** RisingWave hot window — queries beyond this are routed to Polars. */
const HOT_WINDOW_HOURS = 168; // 7 days

/** Warm tier max window — polars-lite serves up to 30 days of aggregates. */
const WARM_WINDOW_DAYS = 30;

/**
 * Proxy a query to the Polars query service.
 * Returns null if POLARS_QUERY_URL is not configured.
 */
async function queryPolars(
  polarsUrl: string,
  body: {
    tenant_id: string;
    tu_id?: string;
    event_type?: string;
    date_from?: string;
    date_to?: string;
    param_key?: string;
    param_value?: string;
    group_by?: string;
    limit?: number;
    mode?: string;
  }
): Promise<any | null> {
  try {
    const resp = await fetch(`${polarsUrl}/query`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
    if (!resp.ok) {
      console.error(`Polars query failed: ${resp.status} ${resp.statusText}`);
      return null;
    }
    return await resp.json();
  } catch (e) {
    console.error("Polars query error:", e);
    return null;
  }
}

/**
 * Convert a Unix epoch seconds value to YYYY-MM-DD date string.
 */
function epochToDate(epochSecs: number): string {
  return new Date(epochSecs * 1000).toISOString().slice(0, 10);
}

/**
 * Proxy a query to the polars-lite warm tier service.
 * Same contract as queryPolars but hits the warm tier (pre-aggregated hourly data).
 * Returns null if POLARS_LITE_URL is not configured.
 */
async function queryPolarsLite(
  polarsLiteUrl: string,
  body: {
    tenant_id: string;
    event_type?: string;
    date_from?: string;
    date_to?: string;
    group_by?: string;
  }
): Promise<any | null> {
  try {
    const resp = await fetch(`${polarsLiteUrl}/query`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
    if (!resp.ok) {
      console.error(`Polars-lite query failed: ${resp.status} ${resp.statusText}`);
      return null;
    }
    return await resp.json();
  } catch (e) {
    console.error("Polars-lite query error:", e);
    return null;
  }
}

/**
 * GET /api/events/history — Query historical events from cold storage (R2 Parquet via Polars).
 *
 * Query params:
 *   - date_from (required, YYYY-MM-DD)
 *   - date_to (required, YYYY-MM-DD)
 *   - tu_id (optional)
 *   - event_type (optional)
 *   - param_key / param_value (optional)
 *   - group_by (optional): "event_type", "tu_id", "date", "param:<key>"
 *   - limit (default 1000)
 *   - mode (optional): "events" or "stats" (default "stats")
 *
 * Returns the Polars query response directly.
 */
events.get("/history", async (c) => {
  const tenantId = c.get("tenantId");
  const db = c.get("db");
  const isAdmin = c.get("isAdmin");
  const polarsUrl = c.env.POLARS_QUERY_URL;

  if (!polarsUrl) {
    return c.json({ error: "Historical queries not configured (POLARS_QUERY_URL not set)" }, 501);
  }

  const dateFrom = c.req.query("date_from");
  const dateTo = c.req.query("date_to");
  if (!dateFrom || !dateTo) {
    return c.json({ error: "date_from and date_to are required (YYYY-MM-DD)" }, 400);
  }

  const keyPrefix = await resolveKeyPrefix(db, tenantId, isAdmin);
  if (keyPrefix === undefined) {
    return c.json({ error: "Tenant not found" }, 404);
  }

  // Admin sees all — "*" tells Polars to skip tenant filter
  const tenantFilter = keyPrefix || "*";

  const result = await queryPolars(polarsUrl, {
    tenant_id: tenantFilter,
    tu_id: c.req.query("tu_id") || undefined,
    event_type: c.req.query("event_type") || undefined,
    date_from: dateFrom,
    date_to: dateTo,
    param_key: c.req.query("param_key") || undefined,
    param_value: c.req.query("param_value") || undefined,
    group_by: c.req.query("group_by") || undefined,
    limit: Number(c.req.query("limit") || "1000"),
    mode: c.req.query("mode") || "stats",
  });

  if (!result) {
    return c.json({ error: "Failed to query historical data" }, 502);
  }

  // Normalize cold tier response to match hot tier field names:
  //   - timestamp_ms → timestamp (consistent with /api/events)
  //   - params: JSON string → parsed object (consistent with RisingWave JSONB)
  if (result.rows && Array.isArray(result.rows)) {
    result.rows = result.rows.map((row: any) => {
      const normalized: any = { ...row };
      if ("timestamp_ms" in normalized && !("timestamp" in normalized)) {
        normalized.timestamp = normalized.timestamp_ms;
        delete normalized.timestamp_ms;
      }
      if (typeof normalized.params === "string") {
        try { normalized.params = JSON.parse(normalized.params); } catch { /* keep as string */ }
      }
      return normalized;
    });
  }

  return c.json({
    ...result,
    source: "cold",
    server_time: Date.now(),
  });
});

/**
 * GET /api/events/stats/merged — Transparent hot+archive stats merge.
 *
 * When the requested time range extends beyond the hot window (7 days),
 * queries both RisingWave (recent) and an archive tier (historical) and merges.
 *
 * Archive tier routing (automatic based on tenant plan):
 *   - free plan → warm tier (polars-lite, 30-day aggregates, stats only)
 *   - pro/enterprise/admin → cold tier (polars-query, full Delta Lake history)
 *
 * Query params: same as /api/events/stats plus:
 *   - hours (can exceed 168 to trigger archive query)
 *   - date_from / date_to (alternative to hours, YYYY-MM-DD)
 *   - tier (optional: "warm" or "cold" — override automatic plan-based routing)
 */
events.get("/stats/merged", async (c) => {
  const tenantId = c.get("tenantId");
  const db = c.get("db");
  const isAdmin = c.get("isAdmin");
  const rwUrl = c.env.RISINGWAVE_URL;
  const polarsUrl = c.env.POLARS_QUERY_URL;
  const polarsLiteUrl = c.env.POLARS_LITE_URL;

  const hours = Number(c.req.query("hours") || "24");
  const dateFrom = c.req.query("date_from");
  const dateTo = c.req.query("date_to");
  const eventType = c.req.query("event_type");
  const tuId = c.req.query("tu_id");
  const groupBy = c.req.query("group_by");
  const tierOverride = c.req.query("tier"); // "warm" or "cold" — explicit tier selection

  const info = await resolveTenantInfo(db, tenantId, isAdmin);
  if (!info) {
    return c.json({ error: "Tenant not found" }, 404);
  }
  const { keyPrefix, plan } = info;

  // Determine which storage tier to use for historical data:
  //   - "warm" override or free plan → polars-lite (30-day aggregates, stats only)
  //   - "cold" override, admin, or pro/enterprise → polars-query (full Delta Lake)
  const useWarmTier =
    tierOverride === "warm" ||
    (tierOverride !== "cold" && plan === "free" && polarsLiteUrl);

  const nowMs = Date.now();
  const nowSecs = Math.floor(nowMs / 1000);

  // Determine time boundaries
  let archiveFrom: string | undefined;
  let archiveTo: string | undefined;
  let hotHours = Math.min(hours, HOT_WINDOW_HOURS);

  if (dateFrom && dateTo) {
    // Explicit date range — split into hot and archive portions
    const fromMs = new Date(dateFrom).getTime();
    const hotBoundaryMs = nowMs - HOT_WINDOW_HOURS * 3600 * 1000;

    if (fromMs < hotBoundaryMs) {
      archiveFrom = dateFrom;
      archiveTo = dateTo;
    }
    // Hot portion: clamp to HOT_WINDOW_HOURS
    hotHours = Math.min(
      Math.ceil((nowMs - Math.max(fromMs, hotBoundaryMs)) / (3600 * 1000)),
      HOT_WINDOW_HOURS
    );

    // Warm tier: clamp to WARM_WINDOW_DAYS
    if (useWarmTier && archiveFrom) {
      const warmBoundary = new Date(nowMs - WARM_WINDOW_DAYS * 86400 * 1000);
      const warmBoundaryStr = warmBoundary.toISOString().slice(0, 10);
      if (archiveFrom < warmBoundaryStr) {
        archiveFrom = warmBoundaryStr;
      }
    }
  } else if (hours > HOT_WINDOW_HOURS) {
    // hours-based range exceeds hot window
    const coldCutoffSecs = nowSecs - hours * 3600;
    const hotBoundarySecs = nowSecs - HOT_WINDOW_HOURS * 3600;
    archiveFrom = epochToDate(coldCutoffSecs);
    archiveTo = epochToDate(hotBoundarySecs);
    hotHours = HOT_WINDOW_HOURS;

    // Warm tier: clamp to WARM_WINDOW_DAYS
    if (useWarmTier) {
      const warmBoundarySecs = nowSecs - WARM_WINDOW_DAYS * 86400;
      if (coldCutoffSecs < warmBoundarySecs) {
        archiveFrom = epochToDate(warmBoundarySecs);
      }
    }
  }

  // Query hot (RisingWave) — direct query, not self-fetch
  let hotResult: any = null;
  if (rwUrl && hotHours > 0) {
    const hotCutoffHour = Math.floor(nowSecs / 3600) * 3600 - hotHours * 3600;
    const hotCurrentHour = Math.floor(nowSecs / 3600) * 3600;
    const statsTable = tuId ? "stats_hourly_by_link" : "stats_hourly";

    const hotConditions: string[] = [];
    if (keyPrefix) hotConditions.push(`tenant_id = '${escapeSql(keyPrefix)}'`);
    if (tuId) hotConditions.push(`tu_id = '${escapeSql(tuId)}'`);
    if (eventType) hotConditions.push(`event_type = '${escapeSql(eventType)}'`);
    hotConditions.push(`hour >= ${hotCutoffHour}`);
    const hotWhere = `WHERE ${hotConditions.join(" AND ")}`;

    let rw: Client | null = null;
    try {
      rw = await getRwClient(rwUrl);
      const hourlyRes = await rw.query(
        `SELECT event_type, hour, count FROM ${statsTable} ${hotWhere} ORDER BY hour DESC`
      );
      const summaryRes = await rw.query(
        `SELECT event_type, SUM(count) as total FROM ${statsTable} ${hotWhere} GROUP BY event_type`
      );
      hotResult = {
        hourly: hourlyRes.rows.map((r: any) => ({ ...r, hour: Number(r.hour), count: Number(r.count) })),
        summary: summaryRes.rows.map((r: any) => ({ ...r, total: Number(r.total) })),
      };
    } catch (e) {
      console.error("Hot stats query failed:", e);
    } finally {
      if (rw) await rw.end().catch(() => {});
    }
  }

  // Query archive tier (warm or cold) based on plan
  let archiveResult: any = null;
  let archiveTier: string | null = null;

  if (archiveFrom && archiveTo) {
    const tenantFilter = keyPrefix || "*";

    if (useWarmTier && polarsLiteUrl) {
      // Warm tier: polars-lite (pre-aggregated hourly data, stats only)
      archiveTier = "warm";
      archiveResult = await queryPolarsLite(polarsLiteUrl, {
        tenant_id: tenantFilter,
        event_type: eventType || undefined,
        date_from: archiveFrom,
        date_to: archiveTo,
        group_by: groupBy || undefined,
      });
    } else if (polarsUrl) {
      // Cold tier: polars-query (full Delta Lake, raw events + stats)
      archiveTier = "cold";
      archiveResult = await queryPolars(polarsUrl, {
        tenant_id: tenantFilter,
        tu_id: tuId || undefined,
        event_type: eventType || undefined,
        date_from: archiveFrom,
        date_to: archiveTo,
        group_by: groupBy || "event_type",
        mode: "stats",
      });
    }
  }

  // Merge results
  const mergedSummary: Record<string, number> = {};

  // Add hot summary
  if (hotResult?.summary) {
    for (const row of hotResult.summary) {
      const key = row.event_type;
      mergedSummary[key] = (mergedSummary[key] || 0) + Number(row.total || row.count || 0);
    }
  }

  // Add archive summary (works for both warm and cold tier responses)
  if (archiveResult?.rows) {
    for (const row of archiveResult.rows) {
      const key = row.event_type;
      mergedSummary[key] = (mergedSummary[key] || 0) + Number(row.count || 0);
    }
  }

  const summary = Object.entries(mergedSummary).map(([event_type, total]) => ({
    event_type,
    total,
  }));

  return c.json({
    summary,
    hourly: hotResult?.hourly || [],
    hours,
    hot_hours: hotHours,
    archive_range: archiveFrom && archiveTo ? { from: archiveFrom, to: archiveTo } : null,
    sources: {
      hot: !!hotResult,
      warm: archiveTier === "warm",
      cold: archiveTier === "cold",
      tier: archiveTier,
      plan,
      hot_partitions: hotResult ? "risingwave" : null,
      archive_partitions: archiveResult?.partitions_scanned || 0,
      archive_query_ms: archiveResult?.query_ms || 0,
    },
    server_time: Date.now(),
  });
});

/**
 * GET /api/events/stats/warm — Query warm tier directly (polars-lite, 30-day aggregates).
 *
 * Returns pre-aggregated hourly stats from the warm tier (R2 aggregate Parquet files).
 * Warm tier has stats only (no raw events). Max 30-day window.
 *
 * Query params:
 *   - date_from (required, YYYY-MM-DD)
 *   - date_to (required, YYYY-MM-DD)
 *   - event_type (optional)
 *   - group_by (optional): "event_type", "date", "hour"
 */
events.get("/stats/warm", async (c) => {
  const tenantId = c.get("tenantId");
  const db = c.get("db");
  const isAdmin = c.get("isAdmin");
  const polarsLiteUrl = c.env.POLARS_LITE_URL;

  if (!polarsLiteUrl) {
    return c.json({ error: "Warm tier not configured (POLARS_LITE_URL not set)" }, 501);
  }

  const dateFrom = c.req.query("date_from");
  const dateTo = c.req.query("date_to");
  if (!dateFrom || !dateTo) {
    return c.json({ error: "date_from and date_to are required (YYYY-MM-DD)" }, 400);
  }

  const keyPrefix = await resolveKeyPrefix(db, tenantId, isAdmin);
  if (keyPrefix === undefined) {
    return c.json({ error: "Tenant not found" }, 404);
  }

  // Clamp to warm window (30 days)
  const nowMs = Date.now();
  const warmBoundary = new Date(nowMs - WARM_WINDOW_DAYS * 86400 * 1000).toISOString().slice(0, 10);
  const clampedFrom = dateFrom < warmBoundary ? warmBoundary : dateFrom;

  const tenantFilter = keyPrefix || "*";

  const result = await queryPolarsLite(polarsLiteUrl, {
    tenant_id: tenantFilter,
    event_type: c.req.query("event_type") || undefined,
    date_from: clampedFrom,
    date_to: dateTo,
    group_by: c.req.query("group_by") || undefined,
  });

  if (!result) {
    return c.json({ error: "Failed to query warm tier" }, 502);
  }

  // Normalize warm tier column names to match cold tier / hot tier conventions:
  //   - date_path → date (consistent with cold tier group_by=date output)
  if (result.rows && Array.isArray(result.rows)) {
    result.rows = result.rows.map((row: any) => {
      const normalized: any = { ...row };
      if ("date_path" in normalized && !("date" in normalized)) {
        normalized.date = normalized.date_path;
        delete normalized.date_path;
      }
      return normalized;
    });
  }

  return c.json({
    ...result,
    source: "warm",
    date_from: clampedFrom,
    date_to: dateTo,
    warm_window_days: WARM_WINDOW_DAYS,
    server_time: Date.now(),
  });
});

/**
 * POST /api/events/query — Ad-hoc SQL query against RisingWave (hot tier).
 *
 * Accepts a tenant-scoped SQL SELECT against the `events` table.
 * The tenant_id filter and time window are injected automatically.
 * JSONB operators work on `params` and `raw_payload` columns.
 *
 * Body:
 *   - sql (required) — SELECT statement (FROM events is implicit, query against `scoped`)
 *   - hours (default 24, max 168) — time window
 *   - limit (default 100, max 1000)
 *
 * Example:
 *   { "sql": "SELECT event_type, raw_payload->>'currency' AS currency, SUM((raw_payload->>'amount')::int) AS total FROM scoped GROUP BY 1, 2 ORDER BY total DESC" }
 */
events.post("/query", async (c) => {
  const tenantId = c.get("tenantId");
  const db = c.get("db");
  const isAdmin = c.get("isAdmin");
  const rwUrl = c.env.RISINGWAVE_URL;

  if (!rwUrl) {
    return c.json({ error: "Ad-hoc queries require RisingWave (RISINGWAVE_URL not set)" }, 501);
  }

  let body: any;
  try {
    body = await c.req.json();
  } catch {
    return c.json({ error: "Invalid JSON body" }, 400);
  }

  const userSql = body.sql;
  if (!userSql || typeof userSql !== "string" || userSql.trim().length === 0) {
    return c.json({ error: "sql is required" }, 400);
  }

  const hours = Math.min(Number(body.hours || 24), 168);
  const limit = Math.min(Number(body.limit || 100), 1000);

  // Safety: reject forbidden keywords
  const lower = userSql.toLowerCase();
  const forbidden = ["drop ", "delete ", "insert ", "update ", "alter ", "create ", "truncate "];
  for (const kw of forbidden) {
    if (lower.includes(kw)) {
      return c.json({ error: `Forbidden keyword in sql: ${kw.trim()}` }, 400);
    }
  }

  // Resolve tenant key_prefix
  const keyPrefix = await resolveKeyPrefix(db, tenantId, isAdmin);
  if (keyPrefix === undefined) {
    return c.json({ error: "Tenant not found" }, 404);
  }

  // Build scoped subquery with mandatory tenant_id + time window
  const cutoffMs = Date.now() - hours * 3600 * 1000;
  const scopeConditions: string[] = [];
  if (keyPrefix) {
    scopeConditions.push(`tenant_id = '${escapeSql(keyPrefix)}'`);
  }
  scopeConditions.push(`timestamp_ms >= ${cutoffMs}`);
  const scopeWhere = scopeConditions.join(" AND ");

  // Wrap: user SQL queries from `scoped` which is a tenant-filtered subquery
  // Only append LIMIT if user SQL doesn't already have one
  const hasLimit = /\bLIMIT\s+\d+/i.test(userSql);
  const fullSql = hasLimit
    ? `WITH scoped AS (SELECT * FROM events WHERE ${scopeWhere}) ${userSql}`
    : `WITH scoped AS (SELECT * FROM events WHERE ${scopeWhere}) ${userSql} LIMIT ${limit}`;

  let rw: Client | null = null;
  const startMs = Date.now();
  try {
    rw = await getRwClient(rwUrl);
    // Set statement timeout to prevent runaway queries
    await rw.query("SET statement_timeout = 10000");
    const result = await rw.query(fullSql);
    const queryMs = Date.now() - startMs;

    return c.json({
      rows: result.rows,
      count: result.rows.length,
      query_ms: queryMs,
      hours,
      limit,
      server_time: Date.now(),
    });
  } catch (e: any) {
    console.error("RisingWave ad-hoc query error:", e);
    return c.json({ error: `Query failed: ${e.message || e}` }, 400);
  } finally {
    if (rw) await rw.end().catch(() => {});
  }
});

// --- Turso fallback functions (used when RISINGWAVE_URL is not set) ---

async function tursoEventsQuery(
  c: any, db: any, keyPrefix: string | null,
  eventType: string | undefined, tuId: string | undefined,
  since: number | null, limit: number, offset: number,
  paramKey?: string, paramValue?: string
) {
  const tenantFilter = keyPrefix ? "tenant_id = ?" : "";
  const typeFilter = eventType ? "event_type = ?" : "";
  const tuIdFilter = tuId ? "params LIKE ?" : "";
  const sinceFilter = since ? "timestamp > ?" : "";
  const paramFilter = paramKey && paramValue
    ? "params LIKE ?"
    : paramKey
      ? "params LIKE ?"
      : "";

  const conditions = [tenantFilter, typeFilter, tuIdFilter, sinceFilter, paramFilter].filter(Boolean);
  const where = conditions.length > 0 ? `WHERE ${conditions.join(" AND ")}` : "";

  const args: (string | number)[] = [];
  if (keyPrefix) args.push(keyPrefix);
  if (eventType) args.push(eventType);
  if (tuId) args.push(`%"tu_id":"${tuId}"%`);
  if (since) args.push(since);
  if (paramKey && paramValue) {
    args.push(`%"${paramKey}":"${paramValue}"%`);
  } else if (paramKey) {
    args.push(`%"${paramKey}":%`);
  }
  args.push(limit, offset);

  const result = await db.execute({
    sql: `SELECT event_id, event_type, timestamp, ip, user_agent, referer, request_path, request_host, params
          FROM recent_events ${where}
          ORDER BY timestamp DESC, event_id DESC
          LIMIT ? OFFSET ?`,
    args,
  });

  return c.json({
    events: result.rows.map((row: any) => ({
      ...row,
      params: row.params ? JSON.parse(row.params as string) : {},
    })),
    count: result.rows.length,
    limit,
    offset,
    server_time: Date.now(),
  });
}

async function tursoStatsQuery(
  c: any, db: any, keyPrefix: string | null,
  eventType: string | undefined, cutoffHour: number,
  currentHour: number, hours: number
) {
  const tenantFilter = keyPrefix ? "tenant_id = ?" : "";
  const typeFilter = eventType ? "event_type = ?" : "";
  const hourFilter = "hour >= ?";

  const conditions = [tenantFilter, typeFilter, hourFilter].filter(Boolean);
  const where = `WHERE ${conditions.join(" AND ")}`;

  const args: (string | number)[] = [];
  if (keyPrefix) args.push(keyPrefix);
  if (eventType) args.push(eventType);
  args.push(cutoffHour);

  const result = await db.execute({
    sql: `SELECT event_type, hour, count FROM stats ${where} ORDER BY hour DESC`,
    args,
  });

  const summary = await db.execute({
    sql: `SELECT event_type, SUM(count) as total FROM stats ${where} GROUP BY event_type`,
    args: [...args],
  });

  return c.json({
    hourly: result.rows,
    summary: summary.rows,
    hours,
    from_hour: cutoffHour,
    to_hour: currentHour,
    server_time: Date.now(),
  });
}

export default events;
