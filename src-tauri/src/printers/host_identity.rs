//! D2 (host identity): the canonical `host:port` key used to detect two
//! Printer Connections that point at the same physical network endpoint,
//! independent of adapter kind or TLS. See the P2 design spec, section D2.

/// Builds the canonical `host:port` identity for a Connection, or `None`
/// when `host` is empty once trimmed.
///
/// Protocol and TLS are deliberately excluded — two adapters on the same
/// host:port are the same physical endpoint. The identity is not
/// DNS-resolved: `printer.local` and `192.168.1.20` remain distinct even
/// when they name the same device.
pub fn canonical_host_identity(host: &str, port: u16) -> Option<String> {
    let trimmed = host.trim();
    let unbracketed = trimmed
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(trimmed);
    let lowered = unbracketed.to_ascii_lowercase();
    let host = lowered.strip_suffix('.').unwrap_or(&lowered);
    if host.is_empty() {
        return None;
    }
    Some(match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V6(v6)) => format!("[{v6}]:{port}"),
        Ok(std::net::IpAddr::V4(v4)) => format!("{v4}:{port}"),
        Err(_) => format!("{host}:{port}"),
    })
}

#[cfg(test)]
mod tests {
    use super::canonical_host_identity;
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct Vector {
        host: String,
        port: u16,
        expected: Option<String>,
    }

    /// The shared Rust/TypeScript conformance vectors for D2 — see
    /// `tests/fixtures/host-identity.json`.
    #[test]
    fn matches_every_fixture_vector() {
        let raw = include_str!("../../tests/fixtures/host-identity.json");
        let vectors: Vec<Vector> = serde_json::from_str(raw).expect("fixture vectors parse");

        for vector in vectors {
            assert_eq!(
                canonical_host_identity(&vector.host, vector.port),
                vector.expected,
                "host={:?} port={}",
                vector.host,
                vector.port
            );
        }
    }
}
