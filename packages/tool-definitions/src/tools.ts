/**
 * Canonical tool definitions — single source of truth for all 26 platform tools.
 *
 * Both @tracker/vivgrid-tools and @tracker/mcp-server derive their tool
 * registrations from this list. Add a tool here and it appears everywhere.
 */

import type { ToolDefinition } from "./types.js";

// ========================== TENANT TOOLS ==================================

const list_tenants: ToolDefinition = {
  name: "list_tenants",
  description:
    "List all tenants on the platform. Returns an array of tenants with id, name, plan, key_prefix, and created_at. Requires admin API key.",
  parameters: {},
  method: "GET",
  path: "/api/tenants",
};

const get_tenant: ToolDefinition = {
  name: "get_tenant",
  description:
    "Get details for a specific tenant by ID. Returns tenant info including name, plan, key_prefix, and created_at. Does NOT return secrets.",
  parameters: {
    tenant_id: {
      type: "string",
      description: "The tenant ID (e.g. '0mld3hqb1utz3skc44tcmam32')",
      required: true,
    },
  },
  method: "GET",
  path: "/api/tenants/:tenant_id",
  pathArgs: ["tenant_id"],
};

const create_tenant: ToolDefinition = {
  name: "create_tenant",
  description:
    "Create a new tenant on the platform. Returns the tenant with id, name, plan, key_prefix, hmac_secret, and encryption_key. IMPORTANT: The hmac_secret and encryption_key are shown ONLY ONCE at creation time. Save them — they cannot be retrieved later.",
  parameters: {
    name: {
      type: "string",
      description: "Tenant display name (e.g. 'Acme Corp')",
      required: true,
    },
    plan: {
      type: "string",
      description:
        "Plan tier: 'free', 'pro', or 'enterprise' (default: 'free')",
    },
    email: {
      type: "string",
      description:
        "Tenant owner email — used for Permit.io RBAC auto-provisioning",
    },
  },
  method: "POST",
  path: "/api/tenants",
  bodyArgs: ["name", "plan", "email"],
};

const update_tenant: ToolDefinition = {
  name: "update_tenant",
  description: "Update a tenant's name, plan, or email.",
  parameters: {
    tenant_id: {
      type: "string",
      description: "The tenant ID to update",
      required: true,
    },
    name: { type: "string", description: "New tenant name" },
    plan: {
      type: "string",
      description: "New plan: 'free', 'pro', or 'enterprise'",
    },
    email: {
      type: "string",
      description: "Tenant owner email — synced to Permit.io for RBAC",
    },
  },
  method: "PATCH",
  path: "/api/tenants/:tenant_id",
  pathArgs: ["tenant_id"],
  bodyArgs: ["name", "plan", "email"],
};

const rotate_tenant_secrets: ToolDefinition = {
  name: "rotate_tenant_secrets",
  description:
    "Rotate a tenant's HMAC secret and encryption key. This invalidates all existing signed/encrypted URLs for this tenant. The new secrets are shown ONLY ONCE — save them immediately. Requires admin API key.",
  parameters: {
    tenant_id: {
      type: "string",
      description: "The tenant ID whose secrets to rotate",
      required: true,
    },
  },
  method: "POST",
  path: "/api/tenants/:tenant_id/rotate-secrets",
  pathArgs: ["tenant_id"],
};

// ========================== API KEY TOOLS =================================

const list_api_keys: ToolDefinition = {
  name: "list_api_keys",
  description:
    "List API keys for the authenticated tenant. Returns key prefix and name only — full keys are never shown after creation.",
  parameters: {},
  method: "GET",
  path: "/api/keys",
};

const create_api_key: ToolDefinition = {
  name: "create_api_key",
  description:
    "Create a new API key for a tenant. Returns the full key ONLY ONCE — save it immediately. The key is used as a Bearer token for API authentication.",
  parameters: {
    tenant_id: {
      type: "string",
      description: "The tenant ID to create the key for",
      required: true,
    },
    name: {
      type: "string",
      description:
        "Descriptive name for the key (e.g. 'Production', 'Staging')",
    },
  },
  method: "POST",
  path: "/api/keys",
  bodyArgs: ["tenant_id", "name"],
};

const revoke_api_key: ToolDefinition = {
  name: "revoke_api_key",
  description: "Permanently revoke an API key. This cannot be undone.",
  parameters: {
    key_id: {
      type: "string",
      description: "The API key ID to revoke",
      required: true,
    },
  },
  method: "DELETE",
  path: "/api/keys/:key_id",
  pathArgs: ["key_id"],
};

// ========================== WEBHOOK TOOLS =================================

const list_webhooks: ToolDefinition = {
  name: "list_webhooks",
  description:
    "List registered webhooks for the authenticated tenant. Shows URL, event_types filter, active status, and creation date.",
  parameters: {},
  method: "GET",
  path: "/api/webhooks",
};

const register_webhook: ToolDefinition = {
  name: "register_webhook",
  description:
    "Register a new webhook endpoint. The webhook will receive HTTP POST requests for matching events. Supports event_type filtering and optional param-level filtering (e.g. only fire when environment=production). Returns the webhook secret ONLY ONCE — save it to verify signatures.",
  parameters: {
    url: {
      type: "string",
      description: "The HTTPS URL to receive webhook POSTs",
      required: true,
    },
    event_types: {
      type: "array",
      description:
        "Event types to subscribe to: ['click', 'postback', 'impression', 'config.updated'] or ['*'] for all (default: ['*'])",
      items: { type: "string" },
    },
    filter_param_key: {
      type: "string",
      description:
        "Optional: only dispatch when the event's params contain this key (e.g. 'environment', 'source', 'campaign_id')",
    },
    filter_param_value: {
      type: "string",
      description:
        "Optional: only dispatch when the param value matches exactly (e.g. 'production'). Requires filter_param_key.",
    },
  },
  method: "POST",
  path: "/api/webhooks",
  bodyArgs: ["url", "event_types", "filter_param_key", "filter_param_value"],
};

const update_webhook: ToolDefinition = {
  name: "update_webhook",
  description: "Update a webhook's URL, event_types, active status, or param filter.",
  parameters: {
    webhook_id: {
      type: "string",
      description: "The webhook ID to update",
      required: true,
    },
    url: { type: "string", description: "New webhook URL" },
    event_types: {
      type: "array",
      description: "New event type filter",
      items: { type: "string" },
    },
    active: {
      type: "boolean",
      description: "Enable (true) or disable (false) the webhook",
    },
    filter_param_key: {
      type: "string",
      description:
        "Set or update param key filter. Set to empty string to remove the filter.",
    },
    filter_param_value: {
      type: "string",
      description:
        "Set or update param value filter. Set to empty string to remove.",
    },
  },
  method: "PATCH",
  path: "/api/webhooks/:webhook_id",
  pathArgs: ["webhook_id"],
  bodyArgs: ["url", "event_types", "active", "filter_param_key", "filter_param_value"],
};

const delete_webhook: ToolDefinition = {
  name: "delete_webhook",
  description: "Permanently delete a webhook. This cannot be undone.",
  parameters: {
    webhook_id: {
      type: "string",
      description: "The webhook ID to delete",
      required: true,
    },
  },
  method: "DELETE",
  path: "/api/webhooks/:webhook_id",
  pathArgs: ["webhook_id"],
};

const test_webhook: ToolDefinition = {
  name: "test_webhook",
  description:
    "Send a test event to a webhook endpoint to verify it's working. Sends a synthetic event and returns the delivery result.",
  parameters: {
    webhook_id: {
      type: "string",
      description: "The webhook ID to test",
      required: true,
    },
  },
  method: "POST",
  path: "/api/webhooks/:webhook_id/test",
  pathArgs: ["webhook_id"],
};

// ========================== EVENT TOOLS ===================================

const list_events: ToolDefinition = {
  name: "list_events",
  description:
    "List recent tracking events. Returns events in reverse chronological order with event_type, timestamp, IP, user_agent, and custom params. Use param_key/param_value to filter by any custom parameter.",
  parameters: {
    limit: {
      type: "number",
      description: "Max events to return (default: 50, max: 200)",
    },
    offset: { type: "number", description: "Pagination offset (default: 0)" },
    event_type: {
      type: "string",
      description: "Filter by event type: 'click', 'postback', or 'impression'",
    },
    tu_id: {
      type: "string",
      description: "Filter by tracking URL ID (e.g. 'tu_019c3f8d-...')",
    },
    param_key: {
      type: "string",
      description:
        "Filter by a custom param key (e.g. 'sub1', 'geo', 'campaign_id')",
    },
    param_value: {
      type: "string",
      description:
        "Filter by param value — requires param_key (e.g. 'google_search', 'US')",
    },
  },
  method: "GET",
  path: "/api/events",
  queryArgs: ["limit", "offset", "event_type", "tu_id", "param_key", "param_value"],
};

const get_stats: ToolDefinition = {
  name: "get_stats",
  description:
    "Get aggregated event statistics (clicks, postbacks, impressions) for the last N hours. Returns hourly counts and summary totals by event_type. Use param_key/param_value to filter by any custom parameter.",
  parameters: {
    hours: {
      type: "number",
      description: "How many hours back to query (default: 24, max: 168)",
    },
    event_type: {
      type: "string",
      description: "Filter by event type: 'click', 'postback', or 'impression'",
    },
    tu_id: {
      type: "string",
      description: "Filter by tracking URL ID",
    },
    param_key: {
      type: "string",
      description:
        "Filter by a custom param key (e.g. 'sub1', 'geo', 'campaign_id')",
    },
    param_value: {
      type: "string",
      description: "Filter by param value — requires param_key",
    },
  },
  method: "GET",
  path: "/api/events/stats",
  queryArgs: ["hours", "event_type", "tu_id", "param_key", "param_value"],
};

const poll_new_events: ToolDefinition = {
  name: "poll_new_events",
  description:
    "Poll for new tracking events since a given timestamp. Returns only events newer than the provided `since` timestamp (Unix ms). Use the `server_time` from the response as the `since` value for the next poll.",
  parameters: {
    since: {
      type: "number",
      description:
        "Unix ms timestamp — only return events newer than this. Use server_time from the previous response.",
    },
    event_type: {
      type: "string",
      description: "Filter by event type: 'click', 'postback', or 'impression'",
    },
    limit: {
      type: "number",
      description: "Max events to return (default: 50, max: 200)",
    },
    tu_id: {
      type: "string",
      description: "Filter by tracking URL ID",
    },
    param_key: {
      type: "string",
      description: "Filter by a custom param key",
    },
    param_value: {
      type: "string",
      description: "Filter by param value — requires param_key",
    },
  },
  method: "GET",
  path: "/api/events",
  queryArgs: ["since", "event_type", "limit", "tu_id", "param_key", "param_value"],
};

const get_breakdown: ToolDefinition = {
  name: "get_breakdown",
  description:
    "Break down event stats by a custom param key. Answers: 'Which traffic source converts best?' (group_by: sub1), 'What are my top geos?' (group_by: geo), 'Which campaigns perform?' (group_by: campaign_id).",
  parameters: {
    group_by: {
      type: "string",
      description:
        "The param key to group by (e.g. 'sub1', 'geo', 'campaign_id', 'creative_id')",
      required: true,
    },
    hours: {
      type: "number",
      description: "How many hours back (default: 24, max: 168)",
    },
    event_type: {
      type: "string",
      description: "Filter by event type",
    },
    tu_id: {
      type: "string",
      description: "Filter by tracking URL ID",
    },
    param_key: {
      type: "string",
      description: "Additional param filter key",
    },
    param_value: {
      type: "string",
      description: "Additional param filter value — requires param_key",
    },
  },
  method: "GET",
  path: "/api/events/stats",
  queryArgs: ["hours", "event_type", "tu_id", "param_key", "param_value"],
};

// ======================== EVENT MATCHING ==================================

const match_events: ToolDefinition = {
  name: "match_events",
  description:
    "Match trigger events to result events by a shared param key. Business-agnostic: trigger=click, result=postback, on=click_id gives conversion rate. Returns matched pairs with time_delta_ms and summary (total_triggers, matched, unmatched, match_rate).",
  parameters: {
    trigger: {
      type: "string",
      description: "First event type (default: 'click')",
    },
    result: {
      type: "string",
      description: "Second event type (default: 'postback')",
    },
    on: {
      type: "string",
      description: "Param key linking them (default: 'click_id')",
    },
    hours: {
      type: "number",
      description: "Hours back (default: 24, max: 168)",
    },
    limit: {
      type: "number",
      description: "Max pairs (default: 50, max: 200)",
    },
    offset: { type: "number", description: "Pagination offset (default: 0)" },
    tu_id: {
      type: "string",
      description: "Filter by tracking URL ID",
    },
    param_key: {
      type: "string",
      description: "Additional param filter key (e.g. 'sub1', 'geo')",
    },
    param_value: {
      type: "string",
      description: "Param filter value — requires param_key",
    },
  },
  method: "GET",
  path: "/api/events/match",
  queryArgs: [
    "trigger",
    "result",
    "on",
    "hours",
    "limit",
    "offset",
    "tu_id",
    "param_key",
    "param_value",
  ],
};

// ====================== HISTORICAL QUERY TOOLS ============================

const query_history: ToolDefinition = {
  name: "query_history",
  description:
    "Query historical events from cold storage (R2 Parquet files via Polars). Use this for data older than 7 days. Requires date_from and date_to. Supports mode='events' for raw rows or mode='stats' for aggregated counts. Supports group_by: 'event_type', 'tu_id', 'date', or 'param:<key>'.",
  parameters: {
    date_from: {
      type: "string",
      description: "Start date (YYYY-MM-DD), e.g. '2026-01-01'",
      required: true,
    },
    date_to: {
      type: "string",
      description: "End date (YYYY-MM-DD), e.g. '2026-02-01'",
      required: true,
    },
    mode: {
      type: "string",
      description: "Query mode: 'stats' (aggregated counts, default) or 'events' (raw rows)",
    },
    event_type: {
      type: "string",
      description: "Filter by event type: 'click', 'postback', or 'impression'",
    },
    tu_id: {
      type: "string",
      description: "Filter by tracking URL ID",
    },
    group_by: {
      type: "string",
      description:
        "Group results by: 'event_type', 'tu_id', 'date', or 'param:<key>' (e.g. 'param:sub1')",
    },
    param_key: {
      type: "string",
      description: "Filter by a custom param key",
    },
    param_value: {
      type: "string",
      description: "Filter by param value — requires param_key",
    },
    limit: {
      type: "number",
      description: "Max rows to return (default: 1000)",
    },
  },
  method: "GET",
  path: "/api/events/history",
  queryArgs: [
    "date_from",
    "date_to",
    "mode",
    "event_type",
    "tu_id",
    "group_by",
    "param_key",
    "param_value",
    "limit",
  ],
};

const get_merged_stats: ToolDefinition = {
  name: "get_merged_stats",
  description:
    "Get merged hot+archive event statistics. Transparently queries RisingWave (recent, up to 7 days) and an archive tier (older data) and merges the results. Archive tier is automatically selected based on tenant plan: free plan → warm tier (polars-lite, 30-day aggregates), pro/enterprise/admin → cold tier (polars-query, full Delta Lake history). Use the 'tier' param to override. Pass hours > 168 or use date_from/date_to for explicit ranges. Response includes sources.tier ('warm'/'cold') and sources.plan.",
  parameters: {
    hours: {
      type: "number",
      description:
        "How many hours back to query. Values > 168 automatically trigger archive storage query. (default: 24)",
    },
    date_from: {
      type: "string",
      description: "Start date (YYYY-MM-DD) — alternative to hours",
    },
    date_to: {
      type: "string",
      description: "End date (YYYY-MM-DD) — alternative to hours",
    },
    event_type: {
      type: "string",
      description: "Filter by event type: 'click', 'postback', or 'impression'",
    },
    tu_id: {
      type: "string",
      description: "Filter by tracking URL ID",
    },
    group_by: {
      type: "string",
      description:
        "Group results by: 'event_type', 'tu_id', 'date', or 'param:<key>'",
    },
    tier: {
      type: "string",
      description:
        "Override automatic archive tier selection: 'warm' (polars-lite, 30-day aggregates) or 'cold' (polars-query, full Delta Lake). Default: auto based on tenant plan (free→warm, pro→cold).",
    },
  },
  method: "GET",
  path: "/api/events/stats/merged",
  queryArgs: ["hours", "date_from", "date_to", "event_type", "tu_id", "group_by", "tier"],
};

const query_warm: ToolDefinition = {
  name: "query_warm",
  description:
    "Query the warm tier directly (polars-lite, pre-aggregated hourly stats from R2). Returns stats only (no raw events). Max 30-day window — dates beyond 30 days are auto-clamped. Use this for free-tier monthly dashboards or when you only need aggregated counts. For raw events, use query_history (cold tier) instead.",
  parameters: {
    date_from: {
      type: "string",
      description: "Start date (YYYY-MM-DD), e.g. '2026-01-15'. Auto-clamped to 30-day warm window.",
      required: true,
    },
    date_to: {
      type: "string",
      description: "End date (YYYY-MM-DD), e.g. '2026-02-10'",
      required: true,
    },
    event_type: {
      type: "string",
      description: "Filter by event type: 'click', 'postback', or 'impression'",
    },
    group_by: {
      type: "string",
      description: "Group results by: 'event_type', 'date', or 'hour'",
    },
  },
  method: "GET",
  path: "/api/events/stats/warm",
  queryArgs: ["date_from", "date_to", "event_type", "group_by"],
};

// ======================== TRACKING URL TOOLS ==============================

const create_tracking_url: ToolDefinition = {
  name: "create_tracking_url",
  description:
    "Register a new tracking URL (naked link). Maps a stable ID to a destination URL. Admin must provide tenant_id; tenant-scoped keys use their own tenant.",
  parameters: {
    destination: {
      type: "string",
      description: "The destination URL to redirect to",
      required: true,
    },
    tenant_id: {
      type: "string",
      description:
        "Tenant ID (required for admin key, auto-resolved for tenant keys)",
    },
  },
  method: "POST",
  path: "/api/tracking-urls",
  bodyArgs: ["destination", "tenant_id"],
};

const list_tracking_urls: ToolDefinition = {
  name: "list_tracking_urls",
  description:
    "List tracking URLs with destination and event counts (clicks, postbacks, impressions).",
  parameters: {
    limit: {
      type: "number",
      description: "Max URLs to return (default: 50, max: 200)",
    },
    offset: { type: "number", description: "Pagination offset (default: 0)" },
  },
  method: "GET",
  path: "/api/tracking-urls",
  queryArgs: ["limit", "offset"],
};

const get_tracking_url: ToolDefinition = {
  name: "get_tracking_url",
  description: "Get a single tracking URL with its event counts.",
  parameters: {
    id: {
      type: "string",
      description: "The tracking URL ID (e.g. 'tu_019502a1-...')",
      required: true,
    },
  },
  method: "GET",
  path: "/api/tracking-urls/:id",
  pathArgs: ["id"],
};

const update_tracking_url: ToolDefinition = {
  name: "update_tracking_url",
  description:
    "Update a tracking URL's destination (link rotation). All existing distributed links with this ID will redirect to the new destination.",
  parameters: {
    id: {
      type: "string",
      description: "The tracking URL ID to update",
      required: true,
    },
    destination: {
      type: "string",
      description: "The new destination URL",
      required: true,
    },
  },
  method: "PATCH",
  path: "/api/tracking-urls/:id",
  pathArgs: ["id"],
  bodyArgs: ["destination"],
};

const delete_tracking_url: ToolDefinition = {
  name: "delete_tracking_url",
  description:
    "Permanently delete a tracking URL. Existing links using this ID will return 404.",
  parameters: {
    id: {
      type: "string",
      description: "The tracking URL ID to delete",
      required: true,
    },
  },
  method: "DELETE",
  path: "/api/tracking-urls/:id",
  pathArgs: ["id"],
};

// ======================== FILTER RULE TOOLS ===============================

const list_filter_rules: ToolDefinition = {
  name: "list_filter_rules",
  description:
    "List event filter rules. Admin sees all rules; tenant-scoped keys see their own rules plus global ('*') rules. Rules are used by the event-filter binary to drop bot traffic and unwanted events before they reach downstream consumers.",
  parameters: {},
  method: "GET",
  path: "/api/filter-rules",
};

const create_filter_rule: ToolDefinition = {
  name: "create_filter_rule",
  description:
    "Create a new event filter rule. Admin can create global rules (tenant_id='*') or tenant-specific rules. Non-admin creates rules for their own tenant. Rules are hot-reloaded by the event-filter binary every 30 seconds.",
  parameters: {
    field: {
      type: "string",
      description:
        "Event field to match: 'user_agent', 'referer', 'ip', 'event_type', 'request_path', 'request_host', or 'param:<key>' for custom params",
      required: true,
    },
    operator: {
      type: "string",
      description:
        "Match operator: 'contains', 'equals', 'is_empty', 'not_empty', 'starts_with'",
      required: true,
    },
    value: {
      type: "string",
      description: "Value to match against (not needed for is_empty/not_empty)",
    },
    action: {
      type: "string",
      description: "Action when matched: 'drop' (discard event, default) or 'flag' (pass but tag)",
    },
    description: {
      type: "string",
      description: "Human-readable description of what this rule does",
    },
    tenant_id: {
      type: "string",
      description: "Admin only: tenant ID to scope the rule to, or '*' for global (default: '*')",
    },
    active: {
      type: "boolean",
      description: "Whether the rule is active (default: true)",
    },
  },
  method: "POST",
  path: "/api/filter-rules",
  bodyArgs: ["field", "operator", "value", "action", "description", "tenant_id", "active"],
};

const update_filter_rule: ToolDefinition = {
  name: "update_filter_rule",
  description:
    "Update an existing filter rule. Can change field, operator, value, action, description, or active status. Changes take effect within 30 seconds.",
  parameters: {
    id: {
      type: "string",
      description: "The filter rule ID to update",
      required: true,
    },
    field: {
      type: "string",
      description: "New field to match",
    },
    operator: {
      type: "string",
      description: "New match operator",
    },
    value: {
      type: "string",
      description: "New match value",
    },
    action: {
      type: "string",
      description: "New action: 'drop' or 'flag'",
    },
    description: {
      type: "string",
      description: "New description",
    },
    active: {
      type: "boolean",
      description: "Enable or disable the rule",
    },
  },
  method: "PATCH",
  path: "/api/filter-rules/:id",
  pathArgs: ["id"],
  bodyArgs: ["field", "operator", "value", "action", "description", "active"],
};

const delete_filter_rule: ToolDefinition = {
  name: "delete_filter_rule",
  description:
    "Permanently delete a filter rule. The rule will stop being applied within 30 seconds.",
  parameters: {
    id: {
      type: "string",
      description: "The filter rule ID to delete",
      required: true,
    },
  },
  method: "DELETE",
  path: "/api/filter-rules/:id",
  pathArgs: ["id"],
};

// ====================== INGEST TOKEN TOOLS ================================

const create_ingest_token: ToolDefinition = {
  name: "create_ingest_token",
  description:
    "Create a new ingest token for POST /ingest authentication. Returns the token ONLY ONCE — save it immediately. Format: pt_{key_prefix}_{random}. Admin must provide tenant_id; tenant-scoped keys use their own tenant.",
  parameters: {
    tenant_id: {
      type: "string",
      description:
        "Tenant ID (required for admin key, auto-resolved for tenant keys)",
    },
    name: {
      type: "string",
      description:
        "Descriptive name for the token (e.g. 'Stripe Plugin', 'RAG Pipeline')",
    },
    plugin_id: {
      type: "string",
      description: "Optional plugin identifier this token is associated with",
    },
    expires_in_days: {
      type: "number",
      description:
        "Optional token expiry in days (e.g. 90). Omit for non-expiring tokens.",
    },
  },
  method: "POST",
  path: "/api/ingest-tokens",
  bodyArgs: ["tenant_id", "name", "plugin_id", "expires_in_days"],
};

const list_ingest_tokens: ToolDefinition = {
  name: "list_ingest_tokens",
  description:
    "List ingest tokens for the authenticated tenant. Shows token ID, name, key_prefix, plugin_id, expiry, and revocation status. Never shows the actual token value.",
  parameters: {
    key_prefix: {
      type: "string",
      description: "Admin only: filter by tenant key_prefix",
    },
  },
  method: "GET",
  path: "/api/ingest-tokens",
  queryArgs: ["key_prefix"],
};

const revoke_ingest_token: ToolDefinition = {
  name: "revoke_ingest_token",
  description:
    "Permanently revoke an ingest token. The token will immediately stop working for POST /ingest. This cannot be undone.",
  parameters: {
    token_id: {
      type: "string",
      description: "The ingest token ID to revoke",
      required: true,
    },
  },
  method: "DELETE",
  path: "/api/ingest-tokens/:token_id",
  pathArgs: ["token_id"],
};

// =================== ONBOARDING HELPER TOOLS ==============================

const generate_tracking_link: ToolDefinition = {
  name: "generate_tracking_link",
  description:
    "Generate a ready-to-use signed click tracking URL. The server loads the tenant's HMAC secret and signs the URL — no secret handling needed by the caller. Returns the complete tracking URL that redirects to the destination.",
  parameters: {
    destination: {
      type: "string",
      description: "The URL the user will be redirected to (e.g. 'https://offer.example.com/landing')",
      required: true,
    },
    params: {
      type: "string",
      description:
        "Optional JSON string of additional tracking params (e.g. '{\"sub1\":\"google\",\"campaign_id\":\"123\"}').",
    },
    tenant_id: {
      type: "string",
      description:
        "Tenant ID (required for admin key, auto-resolved for tenant keys)",
    },
  },
  method: "POST",
  path: "/api/tracking-urls/generate-link",
  bodyArgs: ["destination", "params", "tenant_id"],
};

const ingest_event: ToolDefinition = {
  name: "ingest_event",
  description:
    "Send a test event or a real event through the platform pipeline. Useful for verifying onboarding setup end-to-end. The event flows through Iggy, event-filter, and into all downstream consumers (RisingWave, R2 archiver, webhooks). Requires a valid ingest token for the tenant.",
  parameters: {
    token: {
      type: "string",
      description:
        "Ingest token (pt_{key_prefix}_{secret}) for authentication",
      required: true,
    },
    event_type: {
      type: "string",
      description:
        "Event type (e.g. 'test', 'message.sent', 'config.updated')",
      required: true,
    },
    params: {
      type: "string",
      description:
        "Optional JSON string of flat key-value params (e.g. '{\"source\":\"onboarding\",\"test\":\"true\"}').",
    },
    raw_payload: {
      type: "string",
      description:
        "Optional JSON string of the full nested payload from the external source.",
    },
  },
  method: "POST",
  path: "/api/ingest-event",
  bodyArgs: ["token", "event_type", "params", "raw_payload"],
};

const get_beacon_snippet: ToolDefinition = {
  name: "get_beacon_snippet",
  description:
    "Get the ready-to-paste browser beacon script tag for a tenant. Drop this into any HTML page to automatically track pageviews and outbound clicks. Zero cookies, zero fingerprinting.",
  parameters: {
    key_prefix: {
      type: "string",
      description: "The tenant's key_prefix (e.g. '6vct')",
      required: true,
    },
    host: {
      type: "string",
      description:
        "Optional tracker-core host URL (default: 'https://track.juicyapi.com')",
    },
  },
  method: "GET",
  path: "/api/beacon-snippet",
  queryArgs: ["key_prefix", "host"],
};

// ======================== AI QUERY TOOLS ==================================

const nl_query: ToolDefinition = {
  name: "nl_query",
  description:
    "Ask a natural language question about your data. The AI translates your question into SQL and runs it against the appropriate data source — Delta Lake (raw event history) or plugin Turso tables (structured business data like Stripe charges, Shopify orders). Returns the generated SQL, result rows, data source used, and query time.",
  parameters: {
    prompt: {
      type: "string",
      description:
        "Natural language question (e.g. 'How many clicks did I get last week?', 'Show me top Stripe charges over $100')",
      required: true,
    },
    key_prefix: {
      type: "string",
      description: "Tenant key_prefix for scoping the query",
      required: true,
    },
    limit: {
      type: "number",
      description: "Max rows to return (default: 100)",
    },
  },
  method: "POST",
  path: "/query/nl",
  bodyArgs: ["prompt", "key_prefix", "limit"],
};

const similar_events: ToolDefinition = {
  name: "similar_events",
  description:
    "Find events similar to a given event or text query using vector similarity search (LanceDB). Useful for anomaly detection, fraud pattern discovery, and 'more like this' features. NOTE: This endpoint is currently stubbed — LanceDB integration is pending.",
  parameters: {
    event_id: {
      type: "string",
      description: "Event ID to find similar events for",
    },
    query: {
      type: "string",
      description: "Text query to search for similar events",
    },
    key_prefix: {
      type: "string",
      description: "Tenant key_prefix for scoping",
      required: true,
    },
    limit: {
      type: "number",
      description: "Max results to return (default: 10)",
    },
  },
  method: "POST",
  path: "/query/similar",
  bodyArgs: ["event_id", "query", "key_prefix", "limit"],
};

const ai_chat: ToolDefinition = {
  name: "ai_chat",
  description:
    "Multi-turn AI conversation about your data. Send a conversation history and the AI will generate and execute queries as needed, returning both a natural language response and query results. Supports follow-up questions and context from previous messages.",
  parameters: {
    messages: {
      type: "array",
      description:
        "Conversation messages array. Each message has 'role' ('user' or 'assistant') and 'content' (text).",
      items: { type: "string" },
      required: true,
    },
    key_prefix: {
      type: "string",
      description: "Tenant key_prefix for scoping",
      required: true,
    },
    limit: {
      type: "number",
      description: "Max rows per query (default: 100)",
    },
  },
  method: "POST",
  path: "/chat",
  bodyArgs: ["messages", "key_prefix", "limit"],
};

// ========================== HEALTH TOOL ===================================

const health_check: ToolDefinition = {
  name: "health_check",
  description:
    "Check the health of the Platform API. Returns { status: 'ok' } if running.",
  parameters: {},
  method: "GET",
  path: "/health",
};

// ========================== EXPORT ALL ====================================

export const ALL_TOOLS: ToolDefinition[] = [
  // Tenants
  list_tenants,
  get_tenant,
  create_tenant,
  update_tenant,
  rotate_tenant_secrets,
  // API Keys
  list_api_keys,
  create_api_key,
  revoke_api_key,
  // Webhooks
  list_webhooks,
  register_webhook,
  update_webhook,
  delete_webhook,
  test_webhook,
  // Events
  list_events,
  get_stats,
  poll_new_events,
  get_breakdown,
  match_events,
  // Historical Queries (Hot+Archive, Cold, Warm)
  query_history,
  get_merged_stats,
  query_warm,
  // Tracking URLs
  create_tracking_url,
  list_tracking_urls,
  get_tracking_url,
  update_tracking_url,
  delete_tracking_url,
  // Filter Rules
  list_filter_rules,
  create_filter_rule,
  update_filter_rule,
  delete_filter_rule,
  // Ingest Tokens
  create_ingest_token,
  list_ingest_tokens,
  revoke_ingest_token,
  // Onboarding Helpers
  generate_tracking_link,
  ingest_event,
  get_beacon_snippet,
  // AI Query
  nl_query,
  similar_events,
  ai_chat,
  // Health
  health_check,
];
