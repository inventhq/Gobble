//! Shared aggregate schema for the Warm tier (polars-lite + r2-archiver).
//!
//! Defines column names, types, and R2 path conventions for hourly aggregate
//! Parquet files. Both the writer (r2-archiver) and reader (polars-lite) depend
//! on this crate to prevent schema drift.
//!
//! Zero heavy dependencies — safe to use from both chrono 0.4.41 and 0.4.42+ worlds.

/// Column names in the hourly aggregate Parquet files.
pub mod columns {
    pub const TENANT_ID: &str = "tenant_id";
    pub const EVENT_TYPE: &str = "event_type";
    pub const DATE_PATH: &str = "date_path";
    pub const HOUR: &str = "hour";
    pub const COUNT: &str = "count";
}

/// R2 path conventions for aggregate files.
///
/// Layout: `aggregates/tenant_id={tenant}/date_path={YYYY-MM-DD}/hour={HH}/agg_{flush_id}.parquet`
///
/// Uses Hive-style partitioning so both object_store prefix listing and
/// Polars `scan_parquet` with partition discovery work correctly.
pub mod paths {
    /// Root prefix for all aggregate files on R2.
    pub const AGGREGATES_PREFIX: &str = "aggregates";

    /// Build the full R2 key for an aggregate Parquet file.
    ///
    /// Example: `aggregates/tenant_id=acme/date_path=2026-02-09/hour=14/agg_1739145600.parquet`
    pub fn aggregate_key(tenant_id: &str, date_path: &str, hour: &str, flush_id: u64) -> String {
        format!(
            "{}/tenant_id={}/date_path={}/hour={}/agg_{}.parquet",
            AGGREGATES_PREFIX, tenant_id, date_path, hour, flush_id
        )
    }

    /// Build the R2 prefix for listing all aggregate files for a tenant + date.
    ///
    /// Example: `aggregates/tenant_id=acme/date_path=2026-02-09/`
    pub fn tenant_date_prefix(tenant_id: &str, date_path: &str) -> String {
        format!(
            "{}/tenant_id={}/date_path={}/",
            AGGREGATES_PREFIX, tenant_id, date_path
        )
    }

    /// Build the R2 prefix for listing all aggregate files for a tenant.
    ///
    /// Example: `aggregates/tenant_id=acme/`
    pub fn tenant_prefix(tenant_id: &str) -> String {
        format!("{}/tenant_id={}/", AGGREGATES_PREFIX, tenant_id)
    }

    /// Build the temporary key for atomic overwrite during reconciliation.
    ///
    /// Example: `aggregates/tenant_id=acme/date_path=2026-02-09/hour=14/agg_1739145600.parquet.tmp`
    pub fn aggregate_tmp_key(tenant_id: &str, date_path: &str, hour: &str, flush_id: u64) -> String {
        format!("{}.tmp", aggregate_key(tenant_id, date_path, hour, flush_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aggregate_key() {
        let key = paths::aggregate_key("acme", "2026-02-09", "14", 1739145600);
        assert_eq!(
            key,
            "aggregates/tenant_id=acme/date_path=2026-02-09/hour=14/agg_1739145600.parquet"
        );
    }

    #[test]
    fn test_tenant_date_prefix() {
        let prefix = paths::tenant_date_prefix("acme", "2026-02-09");
        assert_eq!(prefix, "aggregates/tenant_id=acme/date_path=2026-02-09/");
    }

    #[test]
    fn test_tmp_key() {
        let key = paths::aggregate_tmp_key("acme", "2026-02-09", "14", 1739145600);
        assert!(key.ends_with(".tmp"));
    }
}
