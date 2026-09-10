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

/// A topology that always returns a client for a single fixed connection.
///
/// Both [`ServerKind::Master`] and [`ServerKind::Replica`] resolve to the same
/// connection. This is the default topology for non-sentinel deployments.
///
/// The connection is built structurally (address, database, credentials) from
/// [`RedisEndpointConfig`] instead of a URL string: redis-rs parses `?db=` only
/// for unix-socket URLs, while a TCP database rides the URL path segment, so a
/// `redis://host:port?db=N` URL string silently drops db (bd rc-c5l7).
#[derive(Clone, Debug)]
pub struct StandaloneTopology {
    addr: redis::ConnectionAddr,
    settings: redis::RedisConnectionInfo,
    /// PEM-encoded CA bundle trusted as root for this connection; `None`
    /// uses the system truststore. Gated on `tls` because both the store
    /// and the read paths are tls-only, so a feature-less build carries no
    /// dead field.
    #[cfg(feature = "tls")]
    ca_pem: Option<Vec<u8>>,
}

impl StandaloneTopology {
    /// Create a new standalone topology for `config`.
    pub fn new(config: &RedisEndpointConfig) -> Self {
        #[cfg(feature = "tls")]
        let ca_pem = None;
        let (addr, settings) = standalone_conn_parts(config);
        Self {
            addr,
            settings,
            #[cfg(feature = "tls")]
            ca_pem,
        }
    }

    /// Create a standalone topology that trusts `ca_pem` (PEM bytes) as the
    /// root certificate for its TLS connections; `None` keeps the system
    /// truststore, matching [`StandaloneTopology::new`]. Requires the `tls`
    /// feature.
    #[cfg(feature = "tls")]
    pub(crate) fn new_with_ca(config: &RedisEndpointConfig, ca_pem: Option<Vec<u8>>) -> Self {
        let (addr, settings) = standalone_conn_parts(config);
        Self {
            addr,
            settings,
            ca_pem,
        }
    }

    /// PEM CA bundle stored as TLS root trust for this topology, if any.
    /// Test accessor: `topology_from_config` returns `Arc<dyn RedisTopology>`,
    /// which cannot expose it, so it is only compiled into test builds
    /// (otherwise the non-test tls build would flag it as dead code).
    #[cfg(all(test, feature = "tls"))]
    pub(crate) fn ca_pem(&self) -> Option<&[u8]> {
        self.ca_pem.as_deref()
    }
}

/// Build the address and node settings for a standalone connection from the
/// endpoint config. Shared by both constructors so the tls and feature-less
/// builds stay structurally identical.
fn standalone_conn_parts(
    config: &RedisEndpointConfig,
) -> (redis::ConnectionAddr, redis::RedisConnectionInfo) {
    let host = config.host.clone().unwrap_or_else(|| "localhost".into());
    let port = config.port.unwrap_or(6379);
    let addr = if config.is_ssl_enabled() {
        redis::ConnectionAddr::TcpTls {
            host,
            port,
            insecure: false,
            tls_params: None,
        }
    } else {
        redis::ConnectionAddr::Tcp(host, port)
    };
    (addr, node_redis_connection_info(config))
}

#[async_trait]
impl RedisTopology for StandaloneTopology {
    async fn resolve(&self, _kind: ServerKind) -> Result<Client, CamelError> {
        let info = self
            .addr
            .clone()
            .into_connection_info()
            .map_err(|e| {
                CamelError::ProcessorError(format!("failed to build Redis connection info: {e}"))
            })?
            .set_redis_settings(self.settings.clone());

        // TLS with a configured CA: build the client with the stored PEM as
        // root trust instead of the system truststore. `TcpTls` renders as a
        // `rediss://` conn info, which `build_with_tls` requires.
        // `TlsCertificates::root_cert` is already `Option<Vec<u8>>`, so the
        // cloned field assigns directly.
        #[cfg(feature = "tls")]
        if matches!(self.addr, redis::ConnectionAddr::TcpTls { .. }) && self.ca_pem.is_some() {
            return Client::build_with_tls(
                info,
                redis::TlsCertificates {
                    client_tls: None,
                    root_cert: self.ca_pem.clone(),
                },
            )
            .map_err(|e| CamelError::ProcessorError(format!("failed to open Redis client: {e}")));
        }

        Client::open(info)
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
    // Fail-closed TLS feature check (bd rc-ayy11): every client-building path
    // (producer, PubSub consumer, queue consumer, health, sentinel) funnels
    // through here, so an endpoint that resolved to TLS without the `tls`
    // cargo feature dies with a clear Config error at creation time instead
    // of the redis crate's InvalidClientConfig inside a retry loop.
    config.validate_tls()?;
    match &config.topology_kind {
        TopologyKind::Standalone => {
            // The CA file is read ONLY here and ONLY for a TLS-enabled
            // standalone endpoint: a configured CA on a plaintext endpoint
            // or a sentinel endpoint is ignored without any filesystem
            // access (CA trust on the sentinel surface is follow-up bd
            // rc-hbde6). Feature-less builds never reach the CA path —
            // validate_tls above already rejected TLS endpoints.
            build_standalone_topology(config)
        }
        #[cfg(feature = "sentinel")]
        TopologyKind::Sentinel(s) => {
            // CA trust on the sentinel surface is follow-up bd rc-hbde6; a
            // configured tls_ca_cert is ignored for sentinel endpoints.
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

/// Build the standalone [`RedisTopology`] for `config`. With the `tls`
/// feature the configured CA (`tls_ca_cert`) is read for TLS-enabled
/// endpoints and handed to [`StandaloneTopology::new_with_ca`]; feature-less
/// builds use the default constructor (no CA path is ever needed there —
/// `validate_tls` rejects TLS endpoints first).
#[cfg(feature = "tls")]
fn build_standalone_topology(
    config: &RedisEndpointConfig,
) -> Result<Arc<dyn RedisTopology>, CamelError> {
    let ca_pem = read_standalone_ca_pem(config)?;
    Ok(Arc::new(StandaloneTopology::new_with_ca(config, ca_pem)))
}

#[cfg(not(feature = "tls"))]
fn build_standalone_topology(
    config: &RedisEndpointConfig,
) -> Result<Arc<dyn RedisTopology>, CamelError> {
    Ok(Arc::new(StandaloneTopology::new(config)))
}

/// Read the configured TLS CA bundle for a standalone endpoint.
///
/// Returns `None` unless the endpoint is TLS-enabled AND `tls_ca_cert` is
/// set — a configured CA on a plaintext endpoint is ignored without any
/// filesystem access (the sentinel surface is out of scope, follow-up bd
/// rc-hbde6). An unreadable file fails closed with a `Config` error naming
/// the path; the message never contains file contents and deliberately
/// avoids transient-classifier words so `is_transient_redis_error` never
/// retries it (ADR-0012).
#[cfg(feature = "tls")]
fn read_standalone_ca_pem(config: &RedisEndpointConfig) -> Result<Option<Vec<u8>>, CamelError> {
    let Some(path) = config.tls_ca_cert.as_deref() else {
        return Ok(None);
    };
    if !config.is_ssl_enabled() {
        return Ok(None);
    }
    let pem = std::fs::read(path).map_err(|e| {
        CamelError::Config(format!(
            "failed to read the TLS CA certificate file '{path}': {e}"
        ))
    })?;
    Ok(Some(pem))
}

/// Build the [`redis::RedisConnectionInfo`] for a Redis node (not the
/// sentinels) from the endpoint's node credentials: username, password, and
/// database number. Shared by the standalone and sentinel topologies.
fn node_redis_connection_info(config: &RedisEndpointConfig) -> redis::RedisConnectionInfo {
    let mut redis_info = redis::RedisConnectionInfo::default().set_db(config.db as i64);
    if let Some(u) = &config.username {
        redis_info = redis_info.set_username(u);
    }
    if let Some(p) = &config.password {
        redis_info = redis_info.set_password(p);
    }
    redis_info
}

/// Build the [`redis::sentinel::SentinelNodeConnectionInfo`] for the Redis
/// nodes (not the sentinels) from the endpoint's node credentials.
///
/// When the endpoint resolved to TLS (`rediss-sentinel://`), the resolved
/// master/replica connections also use TLS (`TlsMode::Secure`) — the scheme
/// must not encrypt only the sentinel discovery hop (bd rc-ayy11).
/// `redis::TlsMode` is not feature-gated, so this compiles without the
/// `tls` cargo feature; `RedisEndpointConfig::validate_tls` guards the
/// feature-absent case at `topology_from_config` before any connect.
#[cfg(feature = "sentinel")]
fn sentinel_node_conn_info(
    config: &RedisEndpointConfig,
) -> Option<redis::sentinel::SentinelNodeConnectionInfo> {
    let mut info = redis::sentinel::SentinelNodeConnectionInfo::default()
        .set_redis_connection_info(node_redis_connection_info(config));
    if config.is_ssl_enabled() {
        info = info.set_tls_mode(redis::TlsMode::Secure);
    }
    Some(info)
}

#[cfg(test)]
#[path = "topology_tests.rs"]
mod tests;
