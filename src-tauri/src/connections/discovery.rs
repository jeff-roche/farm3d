//! Bounded mDNS browsing for printers that advertise themselves.
//!
//! Discovery is an ACCELERATOR. Manual host/port entry is always available
//! and never gated behind it — a farm of printers at static IPs must be
//! fully configurable with mDNS finding no candidates. Failure to initialize
//! or browse is still reported so it is not confused with a valid empty scan.
//!
//! Results are returned from one bounded call rather than streamed as
//! events: the window is seconds, the result set is a handful of hosts, and
//! a returned vector needs no listener lifecycle and no cross-event
//! de-duplication.

use mdns_sd::{ServiceDaemon, ServiceEvent};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};
use ts_rs::TS;

pub const SERVICE_TYPES: &[&str] = &["_moonraker._tcp.local.", "_octoprint._tcp.local."];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiscoveryError;

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/DiscoveredPrinter.ts")]
pub struct DiscoveredPrinter {
    /// Matches `ConnectionConfig::kind`, so a suggestion can be applied
    /// directly without a lookup table in the UI.
    pub kind: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    /// Offered so a user on a network with broken `.local` resolution can
    /// still pick a working address.
    pub addresses: Vec<String>,
}

pub fn kind_for_service(service_type: &str) -> Option<&'static str> {
    match service_type {
        "_moonraker._tcp.local." => Some("moonraker"),
        "_octoprint._tcp.local." => Some("octoprint"),
        _ => None,
    }
}

pub fn clean_host(hostname: &str) -> String {
    hostname.trim_end_matches('.').to_string()
}

pub fn discover(window: Duration) -> Result<Vec<DiscoveredPrinter>, DiscoveryError> {
    let daemon = ServiceDaemon::new().map_err(|_| DiscoveryError)?;

    let mut receivers = Vec::new();
    for service_type in SERVICE_TYPES {
        let rx = daemon.browse(service_type).map_err(|_| DiscoveryError)?;
        receivers.push((service_type, rx));
    }

    // Keyed so a printer advertising on both service types, or re-announcing
    // within the window, appears once.
    let mut found: BTreeMap<String, DiscoveredPrinter> = BTreeMap::new();
    let deadline = Instant::now() + window;

    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        let mut progressed = false;
        for (service_type, rx) in &receivers {
            // Poll each receiver briefly rather than blocking on one, so a
            // silent service type cannot consume the whole window.
            let event = match rx.recv_timeout(remaining.min(Duration::from_millis(50))) {
                Ok(event) => event,
                Err(mdns_sd::RecvTimeoutError::Timeout) => continue,
                Err(mdns_sd::RecvTimeoutError::Disconnected) => return Err(DiscoveryError),
            };
            progressed = true;
            if let ServiceEvent::ServiceResolved(info) = event {
                let Some(kind) = kind_for_service(service_type) else {
                    continue;
                };
                let host = clean_host(info.get_hostname());
                let entry = DiscoveredPrinter {
                    kind: kind.to_string(),
                    name: info
                        .get_fullname()
                        .split('.')
                        .next()
                        .unwrap_or(&host)
                        .to_string(),
                    host: host.clone(),
                    port: info.get_port(),
                    addresses: info.get_addresses().iter().map(|a| a.to_string()).collect(),
                };
                found.insert(format!("{host}:{}", entry.port), entry);
            }
        }
        if !progressed && receivers.is_empty() {
            break;
        }
    }

    daemon.shutdown().map_err(|_| DiscoveryError)?;
    Ok(found.into_values().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_service_types_to_connection_kinds() {
        assert_eq!(
            kind_for_service("_moonraker._tcp.local."),
            Some("moonraker")
        );
        // Browsed now so phase 3's adapter needs no discovery changes.
        assert_eq!(
            kind_for_service("_octoprint._tcp.local."),
            Some("octoprint")
        );
        assert_eq!(kind_for_service("_http._tcp.local."), None);
    }

    #[test]
    fn browses_exactly_the_two_service_types_we_can_configure() {
        assert_eq!(SERVICE_TYPES.len(), 2);
        assert!(SERVICE_TYPES.iter().all(|t| kind_for_service(t).is_some()));
    }

    #[test]
    fn discovery_returning_nothing_is_a_normal_result_not_an_error() {
        let found = discover(Duration::from_millis(1)).unwrap_or_default();
        assert!(found.len() < 1000);
    }

    #[test]
    fn trailing_dots_are_trimmed_from_hostnames() {
        // mDNS hostnames are fully qualified with a trailing dot, which is
        // not what belongs in a `host` field being handed to a URL builder.
        assert_eq!(clean_host("voron.local."), "voron.local");
        assert_eq!(clean_host("voron.local"), "voron.local");
    }
}
