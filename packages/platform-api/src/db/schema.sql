-- Platform API schema for Turso/libSQL
-- Run via: node scripts/migrate.js

CREATE TABLE IF NOT EXISTS tenants (
  id            TEXT PRIMARY KEY,
  name          TEXT NOT NULL,
  plan          TEXT NOT NULL DEFAULT 'free',
  key_prefix    TEXT NOT NULL UNIQUE,
  hmac_secret   TEXT NOT NULL,
  encryption_key TEXT NOT NULL,
  email         TEXT,
  rate_limit_rps INTEGER NOT NULL DEFAULT 100,
  created_at    INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS api_keys (
  id            TEXT PRIMARY KEY,
  tenant_id     TEXT NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
  key_hash      TEXT NOT NULL UNIQUE,
  key_prefix    TEXT NOT NULL,
  name          TEXT NOT NULL DEFAULT 'Default',
  scopes        TEXT NOT NULL DEFAULT '["*"]',
  last_used_at  INTEGER,
  created_at    INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS webhooks (
  id                TEXT PRIMARY KEY,
  tenant_id         TEXT NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
  url               TEXT NOT NULL,
  event_types       TEXT NOT NULL DEFAULT '["*"]',
  secret            TEXT NOT NULL,
  active            INTEGER NOT NULL DEFAULT 1,
  filter_param_key  TEXT,  -- optional: only dispatch when event.params[key] exists
  filter_param_value TEXT, -- optional: only dispatch when event.params[key] == value
  created_at        INTEGER NOT NULL DEFAULT (unixepoch())
);

-- Migration: add filter columns to existing webhooks table
ALTER TABLE webhooks ADD COLUMN filter_param_key TEXT;
ALTER TABLE webhooks ADD COLUMN filter_param_value TEXT;

CREATE TABLE IF NOT EXISTS webhook_deliveries (
  id            TEXT PRIMARY KEY,
  webhook_id    TEXT NOT NULL REFERENCES webhooks(id) ON DELETE CASCADE,
  event_id      TEXT NOT NULL,
  status_code   INTEGER,
  attempt       INTEGER NOT NULL DEFAULT 1,
  error         TEXT,
  delivered_at  INTEGER NOT NULL DEFAULT (unixepoch())
);

-- Pre-aggregated stats: one row per tenant per hour per event_type
CREATE TABLE IF NOT EXISTS stats (
  tenant_id     TEXT NOT NULL,
  event_type    TEXT NOT NULL,
  hour          INTEGER NOT NULL,
  count         INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (tenant_id, event_type, hour)
);

-- Rolling window of recent events per tenant (capped by consumer)
CREATE TABLE IF NOT EXISTS recent_events (
  id            TEXT PRIMARY KEY,
  tenant_id     TEXT NOT NULL,
  event_id      TEXT NOT NULL,
  event_type    TEXT NOT NULL,
  timestamp     INTEGER NOT NULL,
  ip            TEXT,
  user_agent    TEXT,
  referer       TEXT,
  request_path  TEXT,
  request_host  TEXT,
  params        TEXT,
  created_at    INTEGER NOT NULL DEFAULT (unixepoch())
);

-- Tracking URLs: naked link registry (ID → destination)
-- tracker-core resolves tu_id → destination for short URL redirects
CREATE TABLE IF NOT EXISTS tracking_urls (
  id            TEXT PRIMARY KEY,
  tenant_id     TEXT NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
  destination   TEXT NOT NULL,
  created_at    INTEGER NOT NULL DEFAULT (unixepoch())
);

-- Filter rules: per-tenant event filtering (used by event-filter binary)
-- Rules are hot-reloaded every 30s. Built-in rules (bot UA, empty UA, IP rate)
-- are always active; these are additional custom rules per tenant.
CREATE TABLE IF NOT EXISTS filter_rules (
  id            TEXT PRIMARY KEY,
  tenant_id     TEXT NOT NULL,  -- "*" for global rules, or specific tenant ID
  field         TEXT NOT NULL,  -- "user_agent", "referer", "ip", "event_type", "param:<key>"
  operator      TEXT NOT NULL,  -- "contains", "equals", "is_empty", "not_empty", "starts_with"
  value         TEXT NOT NULL DEFAULT '',
  action        TEXT NOT NULL DEFAULT 'drop',  -- "drop" or "flag"
  description   TEXT,
  active        INTEGER NOT NULL DEFAULT 1,
  created_at    INTEGER NOT NULL DEFAULT (unixepoch())
);

-- Stats ledger: one row per (tenant, event) for idempotent stats counting.
-- INSERT OR IGNORE ensures replayed events during consumer rebalance are never
-- double-counted. The stats table is recomputed from this ledger.
CREATE TABLE IF NOT EXISTS stats_ledger (
  event_id      TEXT NOT NULL,
  tenant_id     TEXT NOT NULL,
  event_type    TEXT NOT NULL,
  hour          INTEGER NOT NULL,
  PRIMARY KEY (tenant_id, event_id)
);

-- Ingest tokens: authenticate POST /ingest calls from plugin-runtime and
-- direct programmatic ingestion. Token format: pt_{key_prefix}_{random}.
-- tracker-core validates the SHA-256 hash and injects key_prefix from the
-- token record (prevents tenant spoofing — callers cannot choose their own
-- key_prefix). Tracking endpoints (/t /p /i) remain unauthenticated.
CREATE TABLE IF NOT EXISTS ingest_tokens (
  id            TEXT PRIMARY KEY,
  tenant_id     TEXT NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
  key_prefix    TEXT NOT NULL,
  token_hash    TEXT NOT NULL UNIQUE,
  name          TEXT NOT NULL DEFAULT 'Default',
  plugin_id     TEXT,              -- optional: scope to a specific plugin
  expires_at    INTEGER,           -- optional: unix epoch seconds, NULL = no expiry
  revoked       INTEGER NOT NULL DEFAULT 0,
  last_used_at  INTEGER,
  created_at    INTEGER NOT NULL DEFAULT (unixepoch())
);

-- Indexes for common query patterns
CREATE INDEX IF NOT EXISTS idx_tracking_urls_tenant ON tracking_urls(tenant_id);
CREATE INDEX IF NOT EXISTS idx_api_keys_tenant ON api_keys(tenant_id);
CREATE INDEX IF NOT EXISTS idx_api_keys_hash ON api_keys(key_hash);
CREATE INDEX IF NOT EXISTS idx_webhooks_tenant ON webhooks(tenant_id);
CREATE INDEX IF NOT EXISTS idx_recent_events_tenant_ts ON recent_events(tenant_id, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_recent_events_tenant_type ON recent_events(tenant_id, event_type);
CREATE INDEX IF NOT EXISTS idx_stats_tenant_hour ON stats(tenant_id, hour DESC);
CREATE INDEX IF NOT EXISTS idx_stats_ledger_hour ON stats_ledger(tenant_id, event_type, hour);
CREATE INDEX IF NOT EXISTS idx_ingest_tokens_tenant ON ingest_tokens(tenant_id);
CREATE INDEX IF NOT EXISTS idx_ingest_tokens_hash ON ingest_tokens(token_hash);
