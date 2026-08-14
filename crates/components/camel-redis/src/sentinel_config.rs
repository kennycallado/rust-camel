//! Sentinel configuration types and URI parsing for Redis Sentinel failover.
//!
//! `SentinelConfig` and `TopologyKind` are NOT feature-gated so the config loader
//! can always recognize a sentinel config and emit a fail-closed error when the
//! `sentinel` cargo feature is disabled. Only converting to a live
//! `SentinelTopology` requires the feature.

use camel_component_api::CamelError;
use camel_component_api::parse_uri;

/// Configuration for Redis Sentinel failover.
///
/// This struct is NOT feature-gated so the config loader can always recognize
/// a sentinel config and emit a fail-closed error when the `sentinel` feature
/// is disabled. Only converting it to a live `SentinelTopology` requires the
/// `sentinel` feature.
#[derive(Clone, PartialEq, Default, serde::Deserialize)]
#[serde(default)]
pub struct SentinelConfig {
    /// Sentinel node URLs (e.g. `["redis://s-a:26379", "redis://s-b:26379"]`).
    /// Each entry carries the `redis://` or `rediss://` prefix so they are valid
    /// redis-rs connection URLs.
    pub nodes: Vec<String>,
    /// The master name to track in the Sentinel cluster.
    pub master_name: String,
    /// Optional username for sentinel authentication.
    pub username: Option<String>,
    /// Optional password for sentinel authentication.
    pub password: Option<String>,
}

// Manual Debug (not derived) so sentinel credentials never leak in plaintext.
// `username`/`password` are redacted to `***` (Some) / `None` (None), matching
// the `redacted_opt` style used by `RedisConfig`/`RedisEndpointConfig` (ADR-0051).
fn redacted_opt(opt: &Option<String>) -> Option<&'static str> {
    if opt.is_some() { Some("***") } else { None }
}

impl std::fmt::Debug for SentinelConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SentinelConfig")
            .field("nodes", &self.nodes)
            .field("master_name", &self.master_name)
            .field("username", &redacted_opt(&self.username))
            .field("password", &redacted_opt(&self.password))
            .finish()
    }
}

impl SentinelConfig {
    /// Returns true when no sentinel fields are configured.
    ///
    /// Used to decide whether a `[components.redis.sentinel]` block is "empty"
    /// (standalone mode) or "non-empty" (sentinel mode + fail-closed checks).
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
            && self.master_name.is_empty()
            && self.username.is_none()
            && self.password.is_none()
    }

    /// Set the sentinel node URLs.
    pub fn with_nodes(mut self, nodes: Vec<String>) -> Self {
        self.nodes = nodes;
        self
    }

    /// Set the master name to track.
    pub fn with_master_name(mut self, name: impl Into<String>) -> Self {
        self.master_name = name.into();
        self
    }

    /// Set sentinel authentication credentials.
    pub fn with_sentinel_credentials(
        mut self,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        self.username = Some(username.into());
        self.password = Some(password.into());
        self
    }
}

/// Identifies the Redis topology kind.
///
/// - `Standalone` — single-node Redis.
/// - `Sentinel(SentinelConfig)` — Redis Sentinel with failover.
/// - `Cluster` — Redis Cluster (requires `cluster` feature).
///
/// The `Sentinel` variant is always present (not feature-gated) so config
/// parsing and validation work without the `sentinel` feature.
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
pub enum TopologyKind {
    /// Single-node Redis (default).
    Standalone,
    /// Redis Sentinel with failover. Always deserializable; requires the
    /// `sentinel` feature to convert to a live `SentinelTopology`.
    Sentinel(SentinelConfig),
    /// Redis Cluster mode. Requires the `cluster` feature.
    #[cfg(feature = "cluster")]
    Cluster,
}

// Manual Default (not derived) so the default topology is explicitly Standalone.
#[allow(clippy::derivable_impls)]
impl Default for TopologyKind {
    fn default() -> Self {
        Self::Standalone
    }
}

/// Result of parsing a `redis-sentinel://` or `rediss-sentinel://` URI.
///
/// Carries both the topology and the database number so the caller consumes a
/// single authority instead of re-splitting the path (DRY).
pub struct ParsedSentinelUri {
    /// The parsed topology (always `TopologyKind::Sentinel`).
    pub topology: TopologyKind,
    /// The database number from the second path segment (defaults to 0 if absent).
    pub db: u8,
}

/// Parse a `redis-sentinel://` or `rediss-sentinel://` URI.
///
/// URI format:
/// ```text
/// redis-sentinel://node1:26379,node2:26379/<master-name>/<db>?command=...
/// ```
///
/// The nodes are comma-separated in the authority portion. Each node is prefixed
/// with `redis://` (or `rediss://` for TLS) to produce valid redis-rs connection
/// URLs. The master name is the first path segment. The database number is the
/// second path segment (defaults to 0 if absent, errors if present but not an
/// integer in 0-255).
///
/// Query parameters (command, key, etc.) are NOT parsed here — the caller handles
/// them via the existing `from_uri` query-param parsing.
pub fn parse_sentinel_uri(uri: &str) -> Result<ParsedSentinelUri, CamelError> {
    let parts = parse_uri(uri)?;

    let is_tls = parts.scheme == "rediss-sentinel";

    // Path format: //node1:port,node2:port,.../master_name/db
    let path = parts.path.strip_prefix("//").unwrap_or(&parts.path);
    let segments: Vec<&str> = path.split('/').collect();

    // First segment: comma-separated node addresses
    let nodes_str = segments
        .first()
        .ok_or_else(|| CamelError::InvalidUri("missing sentinel nodes in URI".to_string()))?;

    if nodes_str.is_empty() {
        return Err(CamelError::InvalidUri(
            "empty sentinel nodes in URI".to_string(),
        ));
    }

    let scheme_prefix = if is_tls { "rediss://" } else { "redis://" };
    let nodes: Vec<String> = nodes_str
        .split(',')
        .map(|n| format!("{}{}", scheme_prefix, n.trim()))
        .collect();

    // Second segment: master name
    let master_name = segments
        .get(1)
        .ok_or_else(|| CamelError::InvalidUri("missing master name in sentinel URI".to_string()))?;

    if master_name.is_empty() {
        return Err(CamelError::InvalidUri(
            "empty master name in sentinel URI".to_string(),
        ));
    }

    // Third segment: database number (defaults to 0 if absent)
    let db = match segments.get(2) {
        Some(s) if !s.is_empty() => s.parse::<u8>().map_err(|_| {
            CamelError::InvalidUri(format!("invalid db '{}': expected integer 0-255", s))
        })?,
        _ => 0,
    };

    Ok(ParsedSentinelUri {
        topology: TopologyKind::Sentinel(SentinelConfig {
            nodes,
            master_name: master_name.to_string(),
            username: None,
            password: None,
        }),
        db,
    })
}

/// Validate topology configuration.
///
/// Returns `Err(CamelError::Config(...))` when:
/// - `kind` is `Sentinel(..)` and `cluster_nodes_present` is true (mutual exclusion).
/// - `kind` is `Sentinel(s)` with empty `master_name`.
/// - `kind` is `Sentinel(s)` with empty `nodes`.
pub fn validate_topology(
    kind: &TopologyKind,
    cluster_nodes_present: bool,
) -> Result<(), CamelError> {
    match kind {
        TopologyKind::Sentinel(s) => {
            if cluster_nodes_present {
                return Err(CamelError::Config(
                    "sentinel and cluster modes are mutually exclusive".to_string(),
                ));
            }
            if s.master_name.is_empty() {
                return Err(CamelError::Config(
                    "sentinel requires a non-empty master_name".to_string(),
                ));
            }
            if s.nodes.is_empty() {
                return Err(CamelError::Config(
                    "sentinel requires at least one node".to_string(),
                ));
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redis_sentinel_uri_parses_to_sentinel_topology() {
        let result =
            parse_sentinel_uri("redis-sentinel://s-a:26379,s-b:26379/orders/0?command=GET");
        let parsed = result.expect("sentinel URI should parse");
        assert_eq!(parsed.db, 0, "db should default to 0 when absent");
        match parsed.topology {
            TopologyKind::Sentinel(s) => {
                assert_eq!(
                    s.nodes,
                    vec![
                        "redis://s-a:26379".to_string(),
                        "redis://s-b:26379".to_string()
                    ],
                    "nodes should carry redis:// prefix"
                );
                assert_eq!(s.master_name, "orders");
                assert_eq!(s.username, None);
                assert_eq!(s.password, None);
            }
            other => panic!("expected Sentinel topology, got {:?}", other),
        }
    }

    #[test]
    fn rediss_sentinel_uri_enables_tls() {
        let result = parse_sentinel_uri("rediss-sentinel://s-a:26379/orders/0");
        let parsed = result.expect("rediss-sentinel URI should parse");
        match parsed.topology {
            TopologyKind::Sentinel(s) => {
                assert_eq!(
                    s.nodes,
                    vec!["rediss://s-a:26379".to_string()],
                    "nodes should carry rediss:// prefix for TLS"
                );
                assert_eq!(s.master_name, "orders");
            }
            other => panic!("expected Sentinel topology, got {:?}", other),
        }
    }

    #[test]
    fn sentinel_and_cluster_together_rejected() {
        let config = SentinelConfig {
            nodes: vec!["redis://s-a:26379".into()],
            master_name: "m".into(),
            ..Default::default()
        };
        let result = validate_topology(&TopologyKind::Sentinel(config), true);
        assert!(
            result.is_err(),
            "sentinel + cluster_nodes_present should be rejected"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("mutually exclusive"),
            "error should mention mutual exclusion: {err}"
        );
    }

    #[test]
    fn sentinel_missing_master_name_rejected() {
        let config = SentinelConfig {
            nodes: vec!["redis://s-a:26379".into()],
            master_name: "".into(),
            ..Default::default()
        };
        let result = validate_topology(&TopologyKind::Sentinel(config), false);
        assert!(result.is_err(), "empty master_name should be rejected");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("master_name"),
            "error should mention master_name: {err}"
        );
    }

    #[test]
    fn sentinel_empty_nodes_rejected() {
        let config = SentinelConfig {
            nodes: vec![],
            master_name: "m".into(),
            ..Default::default()
        };
        let result = validate_topology(&TopologyKind::Sentinel(config), false);
        assert!(result.is_err(), "empty nodes should be rejected");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("at least one node"),
            "error should mention nodes: {err}"
        );
    }

    #[test]
    fn standalone_topology_validates_ok() {
        let result = validate_topology(&TopologyKind::Standalone, false);
        assert!(result.is_ok(), "Standalone should always validate");
    }

    #[test]
    fn sentinel_uri_missing_master_name_errors() {
        let result = parse_sentinel_uri("redis-sentinel://s-a:26379");
        assert!(result.is_err(), "URI without master name should error");
    }

    #[test]
    fn sentinel_uri_empty_nodes_errors() {
        let result = parse_sentinel_uri("redis-sentinel:///orders/0");
        assert!(result.is_err(), "URI with empty nodes should error");
    }

    #[test]
    fn sentinel_config_default_is_empty() {
        let config = SentinelConfig::default();
        assert!(config.nodes.is_empty());
        assert!(config.master_name.is_empty());
        assert_eq!(config.username, None);
        assert_eq!(config.password, None);
    }

    #[test]
    fn sentinel_config_builders() {
        let config = SentinelConfig::default()
            .with_nodes(vec!["redis://s-a:26379".into()])
            .with_master_name("orders")
            .with_sentinel_credentials("user", "pass");
        assert_eq!(config.nodes, vec!["redis://s-a:26379"]);
        assert_eq!(config.master_name, "orders");
        assert_eq!(config.username, Some("user".into()));
        assert_eq!(config.password, Some("pass".into()));
    }

    #[test]
    fn topology_kind_default_is_standalone() {
        assert_eq!(TopologyKind::default(), TopologyKind::Standalone);
    }

    #[test]
    fn sentinel_config_debug_redacts_credentials() {
        let config = SentinelConfig {
            username: Some("secret-user".into()),
            password: Some("secret-pass".into()),
            ..Default::default()
        };
        let debug = format!("{:?}", config);
        assert!(
            !debug.contains("secret-user"),
            "username must be redacted in Debug: {debug}"
        );
        assert!(
            !debug.contains("secret-pass"),
            "password must be redacted in Debug: {debug}"
        );
        assert!(
            debug.contains("***"),
            "redacted marker should appear: {debug}"
        );
    }

    #[test]
    fn sentinel_config_debug_redacts_none_credentials() {
        let config = SentinelConfig::default();
        let debug = format!("{:?}", config);
        assert!(
            debug.contains("username: None") && debug.contains("password: None"),
            "None credentials should print as None: {debug}"
        );
    }
}
