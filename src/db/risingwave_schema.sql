-- RisingWave schema for tracker event analytics.
--
-- This replaces the Turso recent_events + stats tables with a proper
-- streaming database. The risingwave-consumer writes raw events here,
-- and materialized views provide sub-second aggregated stats.

-- Raw events table — every tracking event lands here.
CREATE TABLE IF NOT EXISTS events (
    event_id       VARCHAR PRIMARY KEY,
    tenant_id      VARCHAR NOT NULL,
    event_type     VARCHAR NOT NULL,
    timestamp_ms   BIGINT NOT NULL,
    ip             VARCHAR NOT NULL,
    user_agent     VARCHAR NOT NULL,
    referer        VARCHAR,
    request_path   VARCHAR NOT NULL,
    request_host   VARCHAR NOT NULL,
    params         JSONB NOT NULL DEFAULT '{}'
);

-- Materialized view: hourly stats per tenant per event type.
-- Equivalent to the Turso `stats` table but always fresh.
CREATE MATERIALIZED VIEW IF NOT EXISTS stats_hourly AS
SELECT
    tenant_id,
    event_type,
    (timestamp_ms / 1000 / 3600 * 3600) AS hour,
    COUNT(*) AS count
FROM events
GROUP BY tenant_id, event_type, (timestamp_ms / 1000 / 3600 * 3600);

-- Materialized view: hourly stats per tracking URL (link-level analytics).
-- Used by the /api/events/stats?tu_id=X endpoint for per-link charts.
CREATE MATERIALIZED VIEW IF NOT EXISTS stats_hourly_by_link AS
SELECT
    tenant_id,
    params->>'tu_id' AS tu_id,
    event_type,
    (timestamp_ms / 1000 / 3600 * 3600) AS hour,
    COUNT(*) AS count
FROM events
WHERE params->>'tu_id' IS NOT NULL
GROUP BY tenant_id, params->>'tu_id', event_type, (timestamp_ms / 1000 / 3600 * 3600);

-- Materialized view: total stats per tenant per event type (all time).
CREATE MATERIALIZED VIEW IF NOT EXISTS stats_total AS
SELECT
    tenant_id,
    event_type,
    COUNT(*) AS total
FROM events
GROUP BY tenant_id, event_type;
