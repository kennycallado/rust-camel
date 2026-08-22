//! Topology abstraction for Redis connections.
//!
//! Provides a seam between Redis client creation and the rest of the component,
//! enabling sentinel-based failover, standalone mode, and test fakes.

use crate::config::RedisEndpointConfig;
use crate::sentinel_config::TopologyKind;
use async_trait::async_trait;
use camel_component_api::CamelError;
use redis::{Client, IntoConnectionInfo};
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

/// Identifies which role a Redis endpoint should resolve to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServerKind {
    /// The Redis master node.
    Master,
    /// A Redis replica (read-only) node.
    Replica,
}

/// Strategy for resolving a Redis [`Client`] for a given [`ServerKind`].
///
/// Implementations are free to return the same client for both kinds (standalone),
/// different clients (sentinel), or synthetic clients (tests).
#[async_trait]
pub trait RedisTopology: Send + Sync {
    /// Resolve a Redis client suitable for the given server role.
    async fn resolve(&self, kind: ServerKind) -> Result<Client, CamelError>;
}

/// A topology that always returns a client for a single fixed URL.
///
/// Both [`ServerKind::Master`] and [`ServerKind::Replica`] resolve to the same
/// connection. This is the default topology for non-sentinel deployments.
#[derive(Clone, Debug)]
pub struct StandaloneTopology {
    url: String,
}

impl StandaloneTopology {
    /// Create a new standalone topology pointing at `url`.
    pub fn new(url: impl Into<String>) -> Self {
        Self { url: url.into() }
    }
}

#[async_trait]
impl RedisTopology for StandaloneTopology {
    async fn resolve(&self, _kind: ServerKind) -> Result<Client, CamelError> {
        Client::open(self.url.as_str())
            .map_err(|e| CamelError::ProcessorError(format!("failed to open Redis client: {e}")))
    }
}

/// A topology that returns pre-programmed outcomes for testing.
///
/// Each call to [`resolve`](RedisTopology::resolve) advances through the outcome
/// list. When exhausted the last outcome is repeated. An empty outcome list
/// always returns `Err(CamelError::ProcessorError("fake topology exhausted"))`.
#[cfg(test)]
#[derive(Debug)]
pub struct FakeTopology {
    outcomes: Vec<Result<String, CamelError>>,
    counter: AtomicUsize,
}

#[cfg(test)]
impl FakeTopology {
    /// Create a fake topology from a list of explicit outcomes.
    ///
    /// Each element is either `Ok(address)` or `Err(error)`. The address is
    /// passed to [`Client::open`] on resolution.
    pub fn new(outcomes: Vec<Result<String, CamelError>>) -> Self {
        Self {
            outcomes,
            counter: AtomicUsize::new(0),
        }
    }

    /// Convenience constructor that wraps each address in `Ok(..)`.
    pub fn addrs(addresses: Vec<String>) -> Self {
        let outcomes = addresses.into_iter().map(Ok).collect();
        Self::new(outcomes)
    }

    /// Number of times [`resolve`](RedisTopology::resolve) has been called.
    pub fn resolve_call_count(&self) -> usize {
        self.counter.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
#[async_trait]
impl RedisTopology for FakeTopology {
    async fn resolve(&self, _kind: ServerKind) -> Result<Client, CamelError> {
        let idx = self.counter.fetch_add(1, Ordering::SeqCst);
        let outcome = match self.outcomes.get(idx) {
            Some(o) => o.clone(),
            None => self
                .outcomes
                .last()
                .cloned()
                .unwrap_or(Err(CamelError::ProcessorError(
                    "fake topology exhausted".into(),
                ))),
        };

        match outcome {
            Ok(addr) => Client::open(addr.as_str()).map_err(|e| {
                CamelError::ProcessorError(format!("failed to open Redis client: {e}"))
            }),
            Err(e) => Err(e),
        }
    }
}

/// Inject sentinel credentials into a node URL.
///
/// When `creds` is `Some((user, pass))`, parses `node` via redis-rs's
/// [`IntoConnectionInfo`](redis::IntoConnectionInfo) so the scheme, host, port,
/// and database are preserved (including `rediss://` TLS), then returns a URL
/// with the credentials percent-encoded and embedded. When `creds` is `None`,
/// returns `node` unchanged.
///
/// Fails closed when the node cannot be parsed or its address kind has no URL
/// form this function can rewrite (unix sockets, future
/// `#[non_exhaustive]` variants of `ConnectionAddr`): returning the node
/// unchanged would silently DROP the configured credentials and fail
/// authentication later, so an [`CamelError::Config`] naming the redacted
/// node is returned instead.
///
/// This is a pure function (no I/O, no DNS) and is deliberately NOT behind
/// `#[cfg(feature = "sentinel")]` so it can be unit-tested without the feature.
#[cfg_attr(not(feature = "sentinel"), allow(dead_code))]
pub(crate) fn embed_sentinel_creds(
    node: &str,
    creds: &Option<(String, String)>,
) -> Result<String, CamelError> {
    let Some((user, pass)) = creds else {
        return Ok(node.to_string());
    };

    // Parse through redis-rs so the scheme (Tcp vs TcpTls) and db are preserved.
    let info = node.into_connection_info().map_err(|e| {
        CamelError::Config(format!(
            "cannot inject sentinel credentials into node '{}': {e}",
            redact_userinfo(node)
        ))
    })?;

    let (scheme, host, port) = match info.addr() {
        redis::ConnectionAddr::Tcp(host, port) => ("redis://", host.as_str(), *port),
        redis::ConnectionAddr::TcpTls { host, port, .. } => ("rediss://", host.as_str(), *port),
        // `ConnectionAddr` is `#[non_exhaustive]`, so a wildcard arm is
        // required for forward compatibility; today it also covers unix
        // sockets. Neither has a URL form `embed_sentinel_creds` can rewrite
        // with percent-encoded credentials — fail closed instead of dropping
        // them (see the function docs).
        other => {
            return Err(CamelError::Config(format!(
                "cannot inject sentinel credentials into node '{}': unsupported address kind {other:?}",
                redact_userinfo(node)
            )));
        }
    };

    let db = info.redis_settings().db();

    let user = percent_encoding::utf8_percent_encode(user, percent_encoding::NON_ALPHANUMERIC);
    let pass = percent_encoding::utf8_percent_encode(pass, percent_encoding::NON_ALPHANUMERIC);

    let mut url = format!("{scheme}{user}:{pass}@{host}:{port}");
    if db != 0 {
        url.push_str(&format!("/{db}"));
    }
    Ok(url)
}

/// Strip any `user:pass@` authority from a node URL so the URL is safe to
/// embed in error messages.
#[cfg_attr(not(feature = "sentinel"), allow(dead_code))]
fn redact_userinfo(node: &str) -> String {
    let Some(idx) = node.find("://") else {
        return node.to_string();
    };
    let authority_start = idx + 3;
    let rest = &node[authority_start..];
    match rest.find('@') {
        Some(at) => format!("{}{}", &node[..authority_start], &rest[at + 1..]),
        None => node.to_string(),
    }
}

/// A topology that resolves Redis master addresses through Sentinel.
///
/// Each call to [`resolve(ServerKind::Master)`](RedisTopology::resolve) re-queries
/// the Sentinel cluster for the current master address. The master address is
/// never cached, so failover is detected on the next resolution.
///
/// Requires the `sentinel` feature.
#[cfg(feature = "sentinel")]
pub struct SentinelTopology {
    client: Arc<std::sync::Mutex<redis::sentinel::SentinelClient>>,
}

#[cfg(feature = "sentinel")]
impl std::fmt::Debug for SentinelTopology {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SentinelTopology")
            .field("client", &"<redis::sentinel::SentinelClient>")
            .finish()
    }
}

#[cfg(feature = "sentinel")]
impl SentinelTopology {
    /// Create a new sentinel topology.
    ///
    /// * `sentinel_nodes` — Sentinel node URLs (e.g. `redis://s1:26379`).
    /// * `master_name` — The master name to track.
    /// * `sentinel_creds` — Optional credentials to inject into each sentinel URL.
    /// * `node_conn_info` — Optional connection info for the Redis nodes (not the sentinels).
    ///
    /// Returns `Err(CamelError::Config(_))` if `sentinel_nodes` or `master_name` is empty.
    /// No network I/O is performed during construction.
    pub fn new(
        sentinel_nodes: Vec<String>,
        master_name: String,
        sentinel_creds: Option<(String, String)>,
        node_conn_info: Option<redis::sentinel::SentinelNodeConnectionInfo>,
    ) -> Result<Self, CamelError> {
        if sentinel_nodes.is_empty() {
            return Err(CamelError::Config(
                "sentinel requires nodes and master_name".into(),
            ));
        }
        if master_name.is_empty() {
            return Err(CamelError::Config(
                "sentinel requires nodes and master_name".into(),
            ));
        }

        let nodes_with_creds: Vec<String> = sentinel_nodes
            .into_iter()
            .map(|node| embed_sentinel_creds(&node, &sentinel_creds))
            .collect::<Result<Vec<_>, _>>()?;

        let client = redis::sentinel::SentinelClient::build(
            nodes_with_creds,
            master_name,
            node_conn_info,
            redis::sentinel::SentinelServerType::Master,
        )
        .map_err(|e| CamelError::ProcessorError(format!("failed to build sentinel client: {e}")))?;

        Ok(Self {
            client: Arc::new(std::sync::Mutex::new(client)),
        })
    }
}

#[cfg(feature = "sentinel")]
#[async_trait]
impl RedisTopology for SentinelTopology {
    async fn resolve(&self, kind: ServerKind) -> Result<Client, CamelError> {
        match kind {
            ServerKind::Master => {
                // SentinelClient::get_client performs blocking TCP + SENTINEL
                // queries, so offload it off the Tokio runtime. The std Mutex is
                // held only briefly inside the blocking thread.
                let arc = self.client.clone();
                let client = tokio::task::spawn_blocking(move || match arc.lock() {
                    Ok(mut guard) => guard
                        .get_client()
                        .map_err(|e| format!("sentinel resolve: {e}")),
                    Err(_) => Err("sentinel mutex poisoned".to_string()),
                })
                .await
                .map_err(|e| CamelError::ProcessorError(format!("sentinel resolve join: {e}")))?
                .map_err(CamelError::ProcessorError)?;
                Ok(client)
            }
            ServerKind::Replica => Err(CamelError::ProcessorError(
                "replica reads not yet supported".into(),
            )),
        }
    }
}

/// Build the [`RedisTopology`] for `config`.
///
/// - `Standalone` → a fixed-URL topology.
/// - `Sentinel` (feature-gated) → a sentinel topology that re-queries the
///   sentinel cluster for the current master on every resolve.
/// - `Cluster` → not yet implemented (REDIS-012).
///
/// Shared by the producer, the queue consumer, and the pubsub consumer so all
/// three resolve the master through the same factory.
pub fn topology_from_config(
    config: &RedisEndpointConfig,
) -> Result<Arc<dyn RedisTopology>, CamelError> {
    match &config.topology_kind {
        TopologyKind::Standalone => Ok(Arc::new(StandaloneTopology::new(config.redis_url()))),
        #[cfg(feature = "sentinel")]
        TopologyKind::Sentinel(s) => {
            let sentinel_creds = Some((s.username.clone(), s.password.clone()))
                .filter(|(u, p)| u.is_some() || p.is_some())
                .map(|(u, p)| (u.unwrap_or_default(), p.unwrap_or_default()));
            let node_conn_info = sentinel_node_conn_info(config);
            let topology = SentinelTopology::new(
                s.nodes.clone(),
                s.master_name.clone(),
                sentinel_creds,
                node_conn_info,
            )?;
            Ok(Arc::new(topology))
        }
        #[cfg(not(feature = "sentinel"))]
        TopologyKind::Sentinel(_) => Err(CamelError::Config(
            "sentinel topology requires the 'sentinel' cargo feature".into(),
        )),
        #[cfg(feature = "cluster")]
        TopologyKind::Cluster => Err(CamelError::Config(
            "cluster topology not yet implemented (REDIS-012)".into(),
        )),
    }
}

/// Build the [`redis::sentinel::SentinelNodeConnectionInfo`] for the Redis
/// nodes (not the sentinels) from the endpoint's node credentials.
///
/// The endpoint config carries the node password and database; there is no
/// node username field, so only password + db are propagated.
#[cfg(feature = "sentinel")]
fn sentinel_node_conn_info(
    config: &RedisEndpointConfig,
) -> Option<redis::sentinel::SentinelNodeConnectionInfo> {
    let mut redis_info = redis::RedisConnectionInfo::default().set_db(config.db as i64);
    if let Some(p) = &config.password {
        redis_info = redis_info.set_password(p);
    }
    Some(
        redis::sentinel::SentinelNodeConnectionInfo::default()
            .set_redis_connection_info(redis_info),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn standalone_topology_resolve_returns_fixed_client() {
        let topology = StandaloneTopology::new("redis://127.0.0.1:6379");

        let r1 = topology.resolve(ServerKind::Master).await;
        let r2 = topology.resolve(ServerKind::Master).await;

        let c1 = r1.expect("first resolve should succeed");
        let c2 = r2.expect("second resolve should succeed");
        assert_eq!(
            c1.get_connection_info().addr().to_string(),
            "127.0.0.1:6379"
        );
        assert_eq!(
            c2.get_connection_info().addr().to_string(),
            "127.0.0.1:6379"
        );
    }

    #[tokio::test]
    async fn fake_topology_returns_address_sequence() {
        let topology = FakeTopology::addrs(vec!["redis://a:6379".into(), "redis://b:6379".into()]);

        let r1 = topology.resolve(ServerKind::Master).await;
        let r2 = topology.resolve(ServerKind::Master).await;
        let r3 = topology.resolve(ServerKind::Master).await;

        let c1 = r1.expect("first resolve should succeed");
        let c2 = r2.expect("second resolve should succeed");
        let c3 = r3.expect("third resolve should succeed (reuse last)");
        assert_eq!(c1.get_connection_info().addr().to_string(), "a:6379");
        assert_eq!(c2.get_connection_info().addr().to_string(), "b:6379");
        assert_eq!(c3.get_connection_info().addr().to_string(), "b:6379");
        assert_eq!(topology.resolve_call_count(), 3);
    }

    #[tokio::test]
    async fn fake_topology_returns_programmed_error() {
        let topology = FakeTopology::new(vec![Err(CamelError::ProcessorError("no master".into()))]);

        let result = topology.resolve(ServerKind::Master).await;

        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("no master"),
            "error should contain 'no master'"
        );
        assert_eq!(topology.resolve_call_count(), 1);
    }

    #[test]
    fn embed_sentinel_creds_injects_credentials() {
        let result = embed_sentinel_creds("redis://s-a:26379", &Some(("su".into(), "sp".into())))
            .expect("tcp node with creds should embed");
        assert!(
            result.contains("su:sp"),
            "expected credentials in URL, got: {result}"
        );
        assert!(
            result.contains("s-a:26379"),
            "expected host:port preserved, got: {result}"
        );
    }

    #[test]
    fn embed_sentinel_creds_preserves_node_when_no_creds() {
        let node = "redis://s-b:26379";
        let result =
            embed_sentinel_creds(node, &None).expect("no creds should pass the node through");
        assert_eq!(result, node);
    }

    // M2 fail-closed: an unparsable node with credentials configured must
    // return Err (naming the node, redacted), NOT the node unchanged — the
    // old silent pass-through dropped the credentials and auth failed later.
    #[test]
    fn embed_sentinel_creds_fails_closed_on_unparsable_node() {
        let result = embed_sentinel_creds("", &Some(("su".into(), "sp".into())));
        let err = result.expect_err("empty node URL must fail closed");
        let msg = err.to_string();
        assert!(
            msg.contains("cannot inject sentinel credentials"),
            "error must name the failure: {msg}"
        );
    }

    // M2 fail-closed: unix-socket nodes have no URL form to rewrite with
    // credentials — fail closed instead of silently dropping them.
    #[test]
    fn embed_sentinel_creds_fails_closed_on_unix_socket() {
        let result =
            embed_sentinel_creds("unix:///tmp/redis.sock", &Some(("su".into(), "sp".into())));
        let err = result.expect_err("unix node with creds must fail closed");
        let msg = err.to_string();
        assert!(
            msg.contains("unsupported address kind"),
            "error must name the unsupported kind: {msg}"
        );
        assert!(
            !msg.contains("sp"),
            "error must not leak the sentinel secret: {msg}"
        );
    }

    // Redaction helper: any pre-existing userinfo is stripped for logs/errors.
    #[test]
    fn redact_userinfo_strips_credentials() {
        assert_eq!(
            redact_userinfo("redis://user:pass@host:26379"),
            "redis://host:26379"
        );
        assert_eq!(redact_userinfo("redis://host:26379"), "redis://host:26379");
    }

    #[cfg(feature = "sentinel")]
    #[test]
    fn sentinel_topology_rejects_empty_nodes() {
        let result = SentinelTopology::new(vec![], "m".into(), None, None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("sentinel requires"),
            "error should mention sentinel requires"
        );
    }

    #[cfg(feature = "sentinel")]
    #[test]
    fn sentinel_topology_rejects_empty_master_name() {
        let result = SentinelTopology::new(vec!["redis://s:26379".into()], "".into(), None, None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("sentinel requires"),
            "error should mention sentinel requires"
        );
    }

    #[test]
    fn embed_sentinel_creds_keeps_credentials_separate() {
        // Sentinel creds and node creds must not cross-contaminate: injecting
        // one pair on a node must not leak the other pair.
        let sentinel = embed_sentinel_creds("redis://s-a:26379", &Some(("su".into(), "sp".into())))
            .expect("tcp node should embed");
        let node = embed_sentinel_creds("redis://s-a:26379", &Some(("nu".into(), "np".into())))
            .expect("tcp node should embed");
        assert!(
            sentinel.contains("su:sp"),
            "expected sentinel creds in URL, got: {sentinel}"
        );
        assert!(
            node.contains("nu:np"),
            "expected node creds in URL, got: {node}"
        );
        assert!(
            !sentinel.contains("nu:np"),
            "sentinel URL leaked node creds: {sentinel}"
        );
        assert!(
            !node.contains("su:sp"),
            "node URL leaked sentinel creds: {node}"
        );
    }

    // redis-rs only parses `rediss://` URLs when a TLS feature is enabled, so
    // this test needs the `tls` feature to exercise the TLS-preserving path.
    #[cfg(feature = "tls")]
    #[test]
    fn embed_sentinel_creds_preserves_tls_scheme() {
        // rediss:// must stay TLS after cred injection.
        let result = embed_sentinel_creds("rediss://s-a:26379", &Some(("su".into(), "sp".into())))
            .expect("tls node should embed");
        assert!(
            result.starts_with("rediss://"),
            "expected rediss scheme preserved, got: {result}"
        );
        assert!(
            result.contains("su:sp"),
            "expected creds in URL, got: {result}"
        );
        assert!(
            result.contains("s-a:26379"),
            "expected host:port preserved, got: {result}"
        );
    }

    #[test]
    fn embed_sentinel_creds_percent_encodes_special_chars() {
        let result = embed_sentinel_creds(
            "redis://s-a:26379",
            &Some(("u".into(), "p@ss:word/evil".into())),
        )
        .expect("tcp node should embed");
        // Verify percent-encoding of special characters via NON_ALPHANUMERIC
        assert!(
            result.contains("p%40ss"),
            "expected @ encoded as %40, got: {result}"
        );
        assert!(
            result.contains("%3Aword"),
            "expected : encoded as %3A, got: {result}"
        );
        assert!(
            result.contains("%2Fevil"),
            "expected / encoded as %2F, got: {result}"
        );
        // Round-trip: parse back and verify original creds
        let info = result
            .into_connection_info()
            .expect("should parse back as valid connection info");
        assert_eq!(
            info.redis_settings().password(),
            Some("p@ss:word/evil"),
            "round-trip password mismatch"
        );
        assert_eq!(
            info.redis_settings().username(),
            Some("u"),
            "round-trip username mismatch"
        );
    }

    #[cfg(feature = "sentinel")]
    #[tokio::test]
    async fn sentinel_topology_replica_resolve_errors() {
        let topology =
            SentinelTopology::new(vec!["redis://s:26379".into()], "m".into(), None, None)
                .expect("construction should succeed without network");
        let result = topology.resolve(ServerKind::Replica).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("replica reads not yet supported"),
            "error should mention replica reads, got: {err}"
        );
    }
}
