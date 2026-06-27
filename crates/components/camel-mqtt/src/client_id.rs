// MQTT client ID generation: stable, prefix-based, with 23-byte limit.

use sha2::{Digest, Sha256};

const MAX_CLIENT_ID_LEN: usize = 23;

/// Build a stable MQTT client_id ≤23 bytes.
/// Format: "{prefix}-{route_id}-{uri_hash_6}"
/// If total > 23 bytes, the whole string is SHA-256 hashed and hex-truncated to 23.
pub fn build_client_id(prefix: &str, route_id: &str, uri: &str) -> String {
    build_client_id_with_override(prefix, route_id, uri, None)
}

/// Build an MQTT client_id, optionally overriding with an explicit id.
/// When an override is supplied, it is returned verbatim (no truncation, no hashing).
pub fn build_client_id_with_override(
    prefix: &str,
    route_id: &str,
    uri: &str,
    override_id: Option<&str>,
) -> String {
    if let Some(id) = override_id {
        return id.to_string();
    }

    // 6-char stable hash of the full URI (3 bytes → 6 hex chars)
    let mut hasher = Sha256::new();
    hasher.update(uri.as_bytes());
    let hash = hasher.finalize();
    let uri_hash = hex::encode(&hash[..3]);

    let candidate = format!("{prefix}-{route_id}-{uri_hash}");
    if candidate.len() <= MAX_CLIENT_ID_LEN {
        candidate
    } else {
        // Whole string too long; hash+truncate to 23 bytes.
        // hex output is always 2×input bytes, so 23 ≤ 64 is safe.
        let mut hasher2 = Sha256::new();
        hasher2.update(candidate.as_bytes());
        let h = hasher2.finalize();
        hex::encode(h)[..MAX_CLIENT_ID_LEN].to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_id_under_23_bytes() {
        let id = build_client_id("camel", "my-route", "mqtt://local/temp");
        assert!(id.len() <= 23, "client_id too long: {id}");
    }

    #[test]
    fn explicit_override_respected() {
        let id = build_client_id_with_override(
            "camel",
            "my-route",
            "mqtt://local/temp",
            Some("my-device-001"),
        );
        assert_eq!(id, "my-device-001");
    }

    #[test]
    fn different_uris_produce_different_ids() {
        let id1 = build_client_id("camel", "route-1", "mqtt://local/temp");
        let id2 = build_client_id("camel", "route-1", "mqtt://local/humidity");
        assert_ne!(id1, id2);
    }

    #[test]
    fn same_inputs_produce_same_id() {
        let id1 = build_client_id("camel", "route-1", "mqtt://local/temp");
        let id2 = build_client_id("camel", "route-1", "mqtt://local/temp");
        assert_eq!(id1, id2);
    }

    #[test]
    fn long_route_id_is_truncated_under_23() {
        let id = build_client_id(
            "camel",
            "a-very-very-long-route-id-name",
            "mqtt://local/temp",
        );
        assert!(id.len() <= 23, "client_id too long: {id}");
    }
}
