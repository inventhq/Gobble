# Tracker Platform MCP Server

MCP (Model Context Protocol) server that exposes the Tracker Platform API as AI-callable tools. Enables AI assistants (Windsurf, Cursor, Claude Desktop, etc.) to manage tenants, API keys, webhooks, query events, and view stats through natural language.

## Tools (24 total)

### Tenant Management (admin)
| Tool | Description |
|------|-------------|
| `list_tenants` | List all tenants |
| `get_tenant` | Get tenant details by ID |
| `create_tenant` | Create a new tenant (returns secrets once) |
| `update_tenant` | Update tenant name or plan |
| `rotate_tenant_secrets` | Rotate HMAC/encryption keys |

### API Key Management
| Tool | Description |
|------|-------------|
| `list_api_keys` | List keys (prefix + name only) |
| `create_api_key` | Create a new API key (shown once) |
| `revoke_api_key` | Permanently revoke a key |

### Webhook Management
| Tool | Description |
|------|-------------|
| `list_webhooks` | List registered webhooks |
| `register_webhook` | Register a new webhook endpoint |
| `update_webhook` | Update URL, event_types, or active status |
| `delete_webhook` | Permanently delete a webhook |
| `test_webhook` | Send a test event to a webhook |

### Event Queries & Analytics
| Tool | Description |
|------|-------------|
| `list_events` | Recent events with filtering (event_type, tu_id, param_key/param_value) |
| `get_stats` | Hourly aggregated stats with param filtering |
| `poll_new_events` | Poll for new events since a timestamp (for live watching) |
| `get_breakdown` | **Break down stats by any param key** (e.g. group by sub1, geo, campaign_id) |
| `match_events` | **Generic event-pair matching** — join any two event types by a shared param key |

### Tracking URL Management
| Tool | Description |
|------|-------------|
| `create_tracking_url` | Register a new naked link (tu_id → destination) |
| `list_tracking_urls` | List tracking URLs with event counts |
| `get_tracking_url` | Get a single tracking URL with counts |
| `update_tracking_url` | Update destination (link rotation) |
| `delete_tracking_url` | Permanently delete a tracking URL |

### System
| Tool | Description |
|------|-------------|
| `health_check` | Check Platform API health |

### Param-Level Querying

All event tools support filtering by arbitrary params in the event's `params` map:

- **`param_key`** — filter by a custom param key (e.g. `sub1`, `geo`, `campaign_id`)
- **`param_value`** — match exact value (requires `param_key`)
- **`group_by`** (get_breakdown only) — break down stats by a param key's values

This enables AI to answer natural language questions like:
- "Which traffic source converts best?" → `get_breakdown({ group_by: "sub1", event_type: "postback" })`
- "How's US traffic doing?" → `get_stats({ param_key: "geo", param_value: "US" })`
- "Show me leads from organic" → `list_events({ param_key: "sub1", param_value: "organic", event_type: "postback" })`

### Event Matching (match_events)

The `match_events` tool is **intentionally business-agnostic**. The platform does not know what a "conversion" is. It provides a single generic primitive: join two event types by a shared param key and compute the time between them.

Three params define any matching pattern:
- **`trigger`** — the event_type of the first event (default: `click`)
- **`result`** — the event_type of the second event (default: `postback`)
- **`on`** — the param key that links them (default: `click_id`)

What the user's business calls this match is up to them:

| Vertical | trigger | result | on | They call it |
|---|---|---|---|---|
| Affiliate | click | postback | click_id | "conversion" |
| Lead buyer | impression | postback | lead_id | "lead capture" |
| E-commerce | click | postback | order_id | "purchase" |
| Call center | click | postback | caller_id | "connected call" |
| SaaS | click | postback | user_id | "signup" |

The response uses neutral field names: `trigger_event`, `result_event`, `time_delta_ms`, `matched`, `match_rate`. No domain-specific vocabulary.

## Setup

### 1. Install dependencies

```bash
cd packages/mcp-server
npm install
```

### 2. Environment variables

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `TRACKER_API_URL` | No | `http://localhost:8787` | Platform API base URL |
| `TRACKER_API_KEY` | **Yes** | — | API key (Bearer token) for authentication |

### 3. Run standalone

```bash
TRACKER_API_KEY=tk_admin_bootstrap_dev_only npm start
```

## IDE Integration

### Windsurf

Add to `~/.codeium/windsurf/mcp_config.json`:

```json
{
  "mcpServers": {
    "tracker-platform": {
      "command": "npx",
      "args": ["tsx", "/Users/YOUR_USER/Desktop/tracker/packages/mcp-server/src/index.ts"],
      "env": {
        "TRACKER_API_URL": "http://localhost:8787",
        "TRACKER_API_KEY": "tk_admin_bootstrap_dev_only"
      }
    }
  }
}
```

### Cursor

Add to `.cursor/mcp.json` in your project root:

```json
{
  "mcpServers": {
    "tracker-platform": {
      "command": "npx",
      "args": ["tsx", "./packages/mcp-server/src/index.ts"],
      "env": {
        "TRACKER_API_URL": "http://localhost:8787",
        "TRACKER_API_KEY": "tk_admin_bootstrap_dev_only"
      }
    }
  }
}
```

### Claude Desktop

Add to `~/Library/Application Support/Claude/claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "tracker-platform": {
      "command": "npx",
      "args": ["tsx", "/Users/YOUR_USER/Desktop/tracker/packages/mcp-server/src/index.ts"],
      "env": {
        "TRACKER_API_URL": "http://localhost:8787",
        "TRACKER_API_KEY": "tk_admin_bootstrap_dev_only"
      }
    }
  }
}
```

## Example AI Interactions

Once configured, you can ask your AI assistant:

- "List all tenants on the platform"
- "Create a new tenant called Acme Corp on the pro plan"
- "Show me the last 10 click events"
- "How many clicks did we get in the last 6 hours?"
- "Which traffic source had the most postbacks this week?"
- "Break down clicks by geo for the last 7 days"
- "Show me all events from US organic traffic"
- "What's the top campaign by click volume?"
- "What's my match rate for clicks to postbacks?"
- "How long from click to result for US traffic?"
- "Match impressions to postbacks by lead_id"
- "Register a webhook at https://my-app.com/hooks for click events"
- "Create a tracking URL pointing to https://offer.com/landing"
- "Is the Platform API healthy?"

## Security: API Key Scoping

**The `TRACKER_API_KEY` you configure determines what the MCP server can access.**

| Key Type | Example | Access Level |
|----------|---------|-------------|
| **Admin key** | `tk_admin_bootstrap_dev_only` | Full access — all tenants, all data, create/rotate/delete anything |
| **Tenant key** | `tk_live_sjdx...` | Scoped — only that tenant's events, stats, webhooks, and keys |

- **For you (platform owner):** Use the admin key. You see everything.
- **For your clients (tenants):** If you ever expose MCP to them, configure it with their tenant API key. They can only see their own data. Admin-only tools like `list_tenants` and `create_tenant` will return errors.

> **Never share your admin key with clients.** The MCP server is a passthrough — all access control is enforced by the Platform API based on the key provided.

## Architecture

```
AI Assistant (Windsurf/Cursor/Claude)
    ↓ stdio (JSON-RPC)
MCP Server (this package)
    ↓ HTTP (Bearer token auth)
Platform API (Hono/CF Workers)
    ↓ SQL
Turso (libSQL)
```

The MCP server is a thin adapter — no business logic, no state. It translates MCP tool calls into Platform API HTTP requests and returns the results.
