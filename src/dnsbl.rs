use crate::reputation::{dnsbl_zones, reverse_ipv4};
use anyhow::Result;
use hickory_resolver::TokioResolver;
use tracing::debug;

/// Query public DNSBLs. Lookup failure is "unknown", not listed.
pub async fn listed_on(ip: &str) -> Result<Vec<String>> {
    let Some(rev) = reverse_ipv4(ip) else {
        return Ok(Vec::new());
    };
    let resolver = TokioResolver::builder_tokio()?.build();
    let mut hits = Vec::new();
    for zone in dnsbl_zones() {
        let name = format!("{rev}.{zone}");
        match resolver.ipv4_lookup(name.as_str()).await {
            Ok(lookup) if lookup.iter().next().is_some() => hits.push((*zone).to_string()),
            Ok(_) => {}
            Err(e) => debug!(ip, zone, error = %e, "dnsbl miss or refused"),
        }
    }
    Ok(hits)
}
