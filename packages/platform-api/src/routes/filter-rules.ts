/**
 * Filter rules CRUD routes.
 *
 * Manages per-tenant event filter rules stored in Turso. The event-filter
 * binary hot-reloads these rules every 30s to decide which events pass
 * through to the clean Iggy topic.
 *
 * Routes:
 *   GET    /api/filter-rules          — list rules (tenant-scoped or all for admin)
 *   POST   /api/filter-rules          — create a rule
 *   PATCH  /api/filter-rules/:id      — update a rule
 *   DELETE /api/filter-rules/:id      — delete a rule
 */

import { Hono } from "hono";
import { type AppType } from "../types.js";

const filterRules = new Hono<AppType>();

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function generateId(): string {
  return crypto.randomUUID().replace(/-/g, "").slice(0, 25);
}

const VALID_FIELDS = [
  "user_agent", "referer", "ip", "event_type",
  "request_path", "request_host",
];
const VALID_OPERATORS = ["contains", "equals", "is_empty", "not_empty", "starts_with"];
const VALID_ACTIONS = ["drop", "flag"];

function isValidField(field: string): boolean {
  return VALID_FIELDS.includes(field) || field.startsWith("param:");
}

// ---------------------------------------------------------------------------
// GET / — List filter rules
// ---------------------------------------------------------------------------

filterRules.get("/", async (c) => {
  const tenantId = c.get("tenantId");
  const db = c.get("db");
  const isAdmin = c.get("isAdmin");

  let result;
  if (isAdmin) {
    result = await db.execute(
      "SELECT id, tenant_id, field, operator, value, action, description, active, created_at FROM filter_rules ORDER BY created_at DESC"
    );
  } else {
    result = await db.execute({
      sql: "SELECT id, tenant_id, field, operator, value, action, description, active, created_at FROM filter_rules WHERE tenant_id = ? OR tenant_id = '*' ORDER BY created_at DESC",
      args: [tenantId],
    });
  }

  return c.json({
    rules: result.rows.map((r: any) => ({
      id: r.id,
      tenant_id: r.tenant_id,
      field: r.field,
      operator: r.operator,
      value: r.value,
      action: r.action,
      description: r.description,
      active: !!r.active,
      created_at: r.created_at,
    })),
    count: result.rows.length,
  });
});

// ---------------------------------------------------------------------------
// POST / — Create a filter rule
// ---------------------------------------------------------------------------

filterRules.post("/", async (c) => {
  const tenantId = c.get("tenantId");
  const db = c.get("db");
  const isAdmin = c.get("isAdmin");

  const body = await c.req.json<{
    tenant_id?: string;
    field: string;
    operator: string;
    value?: string;
    action?: string;
    description?: string;
    active?: boolean;
  }>();

  if (!body.field || !body.operator) {
    return c.json({ error: "field and operator are required" }, 400);
  }

  if (!isValidField(body.field)) {
    return c.json({ error: `Invalid field: ${body.field}. Valid: ${VALID_FIELDS.join(", ")}, param:<key>` }, 400);
  }

  if (!VALID_OPERATORS.includes(body.operator)) {
    return c.json({ error: `Invalid operator: ${body.operator}. Valid: ${VALID_OPERATORS.join(", ")}` }, 400);
  }

  const action = body.action || "drop";
  if (!VALID_ACTIONS.includes(action)) {
    return c.json({ error: `Invalid action: ${action}. Valid: ${VALID_ACTIONS.join(", ")}` }, 400);
  }

  // Admin can create global rules ("*") or for any tenant. Non-admin only for their own tenant.
  let ruleTenantId: string;
  if (isAdmin) {
    ruleTenantId = body.tenant_id || "*";
  } else {
    ruleTenantId = tenantId;
  }

  const id = generateId();
  const active = body.active !== false ? 1 : 0;

  await db.execute({
    sql: "INSERT INTO filter_rules (id, tenant_id, field, operator, value, action, description, active) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    args: [id, ruleTenantId, body.field, body.operator, body.value || "", action, body.description || null, active],
  });

  return c.json({
    id,
    tenant_id: ruleTenantId,
    field: body.field,
    operator: body.operator,
    value: body.value || "",
    action,
    description: body.description || null,
    active: !!active,
    message: "Filter rule created. It will be active within 30 seconds.",
  }, 201);
});

// ---------------------------------------------------------------------------
// PATCH /:id — Update a filter rule
// ---------------------------------------------------------------------------

filterRules.patch("/:id", async (c) => {
  const tenantId = c.get("tenantId");
  const db = c.get("db");
  const isAdmin = c.get("isAdmin");
  const ruleId = c.req.param("id");

  // Verify ownership
  const existing = await db.execute({
    sql: "SELECT id, tenant_id FROM filter_rules WHERE id = ?",
    args: [ruleId],
  });
  if (existing.rows.length === 0) {
    return c.json({ error: "Filter rule not found" }, 404);
  }
  if (!isAdmin && existing.rows[0].tenant_id !== tenantId) {
    return c.json({ error: "Not authorized" }, 403);
  }

  const body = await c.req.json<{
    field?: string;
    operator?: string;
    value?: string;
    action?: string;
    description?: string;
    active?: boolean;
  }>();

  const updates: string[] = [];
  const args: any[] = [];

  if (body.field !== undefined) {
    if (!isValidField(body.field)) {
      return c.json({ error: `Invalid field: ${body.field}` }, 400);
    }
    updates.push("field = ?");
    args.push(body.field);
  }
  if (body.operator !== undefined) {
    if (!VALID_OPERATORS.includes(body.operator)) {
      return c.json({ error: `Invalid operator: ${body.operator}` }, 400);
    }
    updates.push("operator = ?");
    args.push(body.operator);
  }
  if (body.value !== undefined) {
    updates.push("value = ?");
    args.push(body.value);
  }
  if (body.action !== undefined) {
    if (!VALID_ACTIONS.includes(body.action)) {
      return c.json({ error: `Invalid action: ${body.action}` }, 400);
    }
    updates.push("action = ?");
    args.push(body.action);
  }
  if (body.description !== undefined) {
    updates.push("description = ?");
    args.push(body.description);
  }
  if (body.active !== undefined) {
    updates.push("active = ?");
    args.push(body.active ? 1 : 0);
  }

  if (updates.length === 0) {
    return c.json({ error: "No fields to update" }, 400);
  }

  args.push(ruleId);
  await db.execute({
    sql: `UPDATE filter_rules SET ${updates.join(", ")} WHERE id = ?`,
    args,
  });

  return c.json({ id: ruleId, updated: true, message: "Rule updated. Changes active within 30 seconds." });
});

// ---------------------------------------------------------------------------
// DELETE /:id — Delete a filter rule
// ---------------------------------------------------------------------------

filterRules.delete("/:id", async (c) => {
  const tenantId = c.get("tenantId");
  const db = c.get("db");
  const isAdmin = c.get("isAdmin");
  const ruleId = c.req.param("id");

  // Verify ownership
  const existing = await db.execute({
    sql: "SELECT id, tenant_id FROM filter_rules WHERE id = ?",
    args: [ruleId],
  });
  if (existing.rows.length === 0) {
    return c.json({ error: "Filter rule not found" }, 404);
  }
  if (!isAdmin && existing.rows[0].tenant_id !== tenantId) {
    return c.json({ error: "Not authorized" }, 403);
  }

  await db.execute({
    sql: "DELETE FROM filter_rules WHERE id = ?",
    args: [ruleId],
  });

  return c.json({ id: ruleId, deleted: true });
});

export { filterRules };
