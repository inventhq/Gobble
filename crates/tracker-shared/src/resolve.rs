//! DNS resolution helper for Iggy server addresses.

use tracing::warn;

/// Resolve a `host:port` address, converting DNS hostnames to IP addresses.
/// The Iggy SDK only accepts raw IP addresses, so K8s service DNS names
/// like `iggy.tracker.svc.cluster.local:8090` must be resolved first.
/// If the host is already an IP address, it is returned unchanged.
pub async fn resolve_server_addr(addr: &str) -> String {
    use tokio::net::lookup_host;

    // Try parsing as-is first (already an IP:port)
    if addr.parse::<std::net::SocketAddr>().is_ok() {
        return addr.to_string();
    }

    // Resolve DNS hostname to IP
    match lookup_host(addr).await {
        Ok(mut addrs) => {
            if let Some(resolved) = addrs.next() {
                format!("{}:{}", resolved.ip(), resolved.port())
            } else {
                warn!("DNS resolution returned no addresses for {}, using as-is", addr);
                addr.to_string()
            }
        }
        Err(e) => {
            warn!("DNS resolution failed for {}: {} — using as-is", addr, e);
            addr.to_string()
        }
    }
}
