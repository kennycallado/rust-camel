use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use camel_api::error::CamelError;
use camel_auth::oauth2::TokenProvider;
use camel_component_api::NetworkRetryPolicy;
use serde::Deserialize;
use serde::de::{self, MapAccess, Visitor};
use tracing::error;

// ── Transport configuration (ADR-0033) ────────────────────────────────────

/// Outbound (client) TLS configuration. The client VERIFIES the server
/// (ca_cert) and optionally presents its own identity (mTLS).
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct ClientTlsConfig {
    #[serde(default)]
    pub server_name: Option<String>,
    #[serde(default)]
    pub ca_cert_path: Option<String>,
    #[serde(default)]
    pub client_cert_path: Option<String>,
    #[serde(default)]
    pub client_key_path: Option<String>,
    /// If true, skip server cert verification. Hard-errors today (fail-closed);
    /// the field is preserved so the gate is explicit, not silent.
    #[serde(default)]
    pub insecure_skip_verify: bool,
}

/// Outbound transport intent. Plaintext MUST be declared explicitly (ADR-0033).
#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
pub enum ClientTransport {
    #[default]
    Plaintext,
    Tls(ClientTlsConfig),
}

/// Inbound (server) TLS configuration. Server-auth by default; when
/// `client_ca_path` is set, the server verifies client certificates (mTLS).
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ServerTlsConfig {
    pub server_cert_path: String,
    pub server_key_path: String,
    /// Optional CA (PEM) to verify CLIENT certificates (mTLS). When set,
    /// the server rejects clients that don't present a valid cert signed
    /// by this CA. When absent, server-auth only (backward compatible).
    #[serde(default)]
    pub client_ca_path: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
pub enum ServerTransport {
    #[default]
    Plaintext,
    Tls(ServerTlsConfig),
}

/// Top-level transport intent — mirrors the URI `transport=` parameter.
#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
pub enum TransportIntent {
    #[default]
    Plaintext,
    Tls,
}

// ── Auth configuration (GRPC-007) ─────────────────────────────────────────

/// Authentication configuration for gRPC channels.
#[derive(Default)]
#[non_exhaustive]
pub enum AuthConfig {
    /// No authentication.
    #[default]
    None,
    /// Bearer token authentication. Adds `Authorization: Bearer <token>` to request metadata.
    Bearer { token: String },
    /// Google service account authentication (scaffold only).
    ///
    /// /// TODO(GRPC-007): Google service account token refresh not yet implemented
    GoogleServiceAccount { json_path: String },
    /// OAuth2 token provider for dynamic token injection.
    OAuth2 {
        token_provider: Arc<dyn TokenProvider>,
    },
}

impl<'de> Deserialize<'de> for AuthConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        struct AuthConfigVisitor;

        impl<'de> Visitor<'de> for AuthConfigVisitor {
            type Value = AuthConfig;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an AuthConfig variant")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                match v {
                    "None" => Ok(AuthConfig::None),
                    "OAuth2" => Err(de::Error::custom(
                        "OAuth2 variant cannot be deserialized; construct programmatically",
                    )),
                    other => Err(de::Error::unknown_variant(
                        other,
                        &["None", "Bearer", "GoogleServiceAccount", "OAuth2"],
                    )),
                }
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let variant = map
                    .next_key::<String>()?
                    .ok_or_else(|| de::Error::invalid_length(0, &self))?;
                match variant.as_str() {
                    "None" => Ok(AuthConfig::None),
                    "Bearer" => {
                        let mut token = None;
                        while let Some(key) = map.next_key::<String>()? {
                            if key == "token" {
                                token = Some(map.next_value()?);
                            } else {
                                let _: de::IgnoredAny = map.next_value()?;
                            }
                        }
                        let token = token.ok_or_else(|| de::Error::missing_field("token"))?;
                        Ok(AuthConfig::Bearer { token })
                    }
                    "GoogleServiceAccount" => {
                        let mut json_path = None;
                        while let Some(key) = map.next_key::<String>()? {
                            if key == "json_path" {
                                json_path = Some(map.next_value()?);
                            } else {
                                let _: de::IgnoredAny = map.next_value()?;
                            }
                        }
                        let json_path =
                            json_path.ok_or_else(|| de::Error::missing_field("json_path"))?;
                        Ok(AuthConfig::GoogleServiceAccount { json_path })
                    }
                    "OAuth2" => Err(de::Error::custom(
                        "OAuth2 variant cannot be deserialized; construct programmatically",
                    )),
                    other => Err(de::Error::unknown_variant(
                        other,
                        &["None", "Bearer", "GoogleServiceAccount"],
                    )),
                }
            }
        }

        deserializer.deserialize_any(AuthConfigVisitor)
    }
}

impl fmt::Debug for AuthConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AuthConfig::None => write!(f, "None"), // allow-secret
            AuthConfig::Bearer { .. } => write!(f, "Bearer {{ token: \"[REDACTED]\" }}"), // allow-secret
            AuthConfig::GoogleServiceAccount { .. } => {
                write!(f, "GoogleServiceAccount {{ json_path: \"[REDACTED]\" }}") // allow-secret
            }
            AuthConfig::OAuth2 { .. } => write!(f, "OAuth2 {{ token_provider: \"[REDACTED]\" }}"), // allow-secret
        }
    }
}

impl Clone for AuthConfig {
    fn clone(&self) -> Self {
        match self {
            AuthConfig::None => AuthConfig::None,
            AuthConfig::Bearer { token } => AuthConfig::Bearer {
                token: token.clone(),
            },
            AuthConfig::GoogleServiceAccount { json_path } => AuthConfig::GoogleServiceAccount {
                json_path: json_path.clone(),
            },
            AuthConfig::OAuth2 { token_provider } => AuthConfig::OAuth2 {
                token_provider: Arc::clone(token_provider),
            },
        }
    }
}

// ── Interceptor placeholder (GRPC-008) ────────────────────────────────────

/// Placeholder for future gRPC interceptor registration.
///
/// This field stores interceptor class/type names as strings. Actual wiring
/// into the tonic service stack is not yet implemented.
///
/// /// TODO(GRPC-008): interceptor registry not yet implemented
#[derive(Debug, Clone, Default, Deserialize)]
#[non_exhaustive]
pub struct InterceptorConfig {
    #[serde(default)]
    pub interceptors: Vec<String>,
}

// ── Consumer / Producer strategies (GRPC-009) ────────────────────────────

/// Strategy for selecting among multiple gRPC consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConsumerStrategy {
    #[default]
    RoundRobin,
    First,
    Last,
}

impl fmt::Display for ConsumerStrategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConsumerStrategy::RoundRobin => write!(f, "roundRobin"),
            ConsumerStrategy::First => write!(f, "first"),
            ConsumerStrategy::Last => write!(f, "last"),
        }
    }
}

impl FromStr for ConsumerStrategy {
    type Err = CamelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "roundRobin" | "round_robin" => Ok(Self::RoundRobin),
            "first" => Ok(Self::First),
            "last" => Ok(Self::Last),
            _ => Err(CamelError::Config(format!("invalid ConsumerStrategy: {s}"))),
        }
    }
}

/// Strategy for gRPC producer invocation mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProducerStrategy {
    FireAndForget,
    #[default]
    RequestReply,
}

impl fmt::Display for ProducerStrategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProducerStrategy::FireAndForget => write!(f, "fireAndForget"),
            ProducerStrategy::RequestReply => write!(f, "requestReply"),
        }
    }
}

impl FromStr for ProducerStrategy {
    type Err = CamelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "fireAndForget" | "fire_and_forget" => Ok(Self::FireAndForget),
            "requestReply" | "request_reply" => Ok(Self::RequestReply),
            _ => Err(CamelError::Config(format!("invalid ProducerStrategy: {s}"))),
        }
    }
}

// ── Main config ───────────────────────────────────────────────────────────

#[derive(Clone, Deserialize)]
pub struct GrpcConfig {
    #[serde(rename = "protoFile")]
    pub proto_file: Option<String>,
    pub service: Option<String>,
    pub method: Option<String>,
    #[serde(default)]
    pub reflection: bool,
    #[serde(default)]
    pub transport_intent: TransportIntent,
    pub client_transport: ClientTransport,
    pub server_transport: ServerTransport,
    #[serde(default = "default_max_msg_len")]
    pub max_receive_message_length: usize,
    pub deadline_ms: Option<u64>,
    pub metadata: Option<String>,

    // H14: default connect timeout and deadline
    #[serde(default = "default_connect_timeout_ms")]
    pub connect_timeout_ms: u64,
    #[serde(default = "default_deadline_ms")]
    pub default_deadline_ms: u64,

    // GRPC-007: Auth support
    #[serde(default)]
    pub auth: AuthConfig,

    // GRPC-008: Interceptor placeholder
    #[serde(default)]
    pub interceptors: InterceptorConfig,

    // GRPC-009: Strategy configuration
    #[serde(default)]
    pub consumer_strategy: ConsumerStrategy,
    #[serde(default)]
    pub producer_strategy: ProducerStrategy,

    /// Transient error retry policy for gRPC producer RPC calls.
    ///
    /// Controls how the producer retries `Unavailable`, `DeadlineExceeded`,
    /// `ResourceExhausted`, `Aborted`, and transport-level errors. Permanent
    /// codes (`InvalidArgument`, `NotFound`, `PermissionDenied`, etc.) are
    /// never retried.
    #[serde(default)]
    pub retry: NetworkRetryPolicy,
}

/// Custom Debug that redacts sensitive fields (GRPC-013).
impl fmt::Debug for GrpcConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GrpcConfig")
            .field("proto_file", &self.proto_file)
            .field("service", &self.service)
            .field("method", &self.method)
            .field("reflection", &self.reflection)
            .field("client_transport", &self.client_transport)
            .field("server_transport", &self.server_transport)
            .field(
                "max_receive_message_length",
                &self.max_receive_message_length,
            )
            .field("deadline_ms", &self.deadline_ms)
            .field("connect_timeout_ms", &self.connect_timeout_ms)
            .field("default_deadline_ms", &self.default_deadline_ms)
            .field("metadata", &self.metadata.as_ref().map(|_| "[REDACTED]"))
            .field("auth", &self.auth)
            .field("interceptors", &self.interceptors)
            .field("consumer_strategy", &self.consumer_strategy)
            .field("producer_strategy", &self.producer_strategy)
            .field("retry", &self.retry)
            .finish()
    }
}

fn default_max_msg_len() -> usize {
    4 * 1024 * 1024
}

fn default_connect_timeout_ms() -> u64 {
    10_000
}

fn default_deadline_ms() -> u64 {
    30_000
}

/// Server-side configuration for the gRPC transport layer.
#[derive(Debug, Clone)]
pub struct GrpcServerConfig {
    /// Maximum incoming message size in bytes. None means use tonic/hyper default.
    pub max_receive_message_len: Option<usize>,
    /// Transport mode for inbound connections (plaintext or TLS).
    pub transport: ServerTransport,
}

impl Default for GrpcServerConfig {
    fn default() -> Self {
        Self {
            max_receive_message_len: None,
            transport: ServerTransport::Plaintext,
        }
    }
}

// ── TLS file I/O helper ───────────────────────────────────────────────────

pub(crate) fn read_tls_file(
    path: &str,
    label: &str,
    runtime: &Arc<dyn camel_component_api::RuntimeObservability>,
    route_id: &str,
) -> Result<Vec<u8>, CamelError> {
    std::fs::read(path).map_err(|e| {
        runtime.health().force_unhealthy_for_route(
            route_id,
            "g:grpc:tls-read",
            &format!("failed to read {label}: {e}"),
        );
        // log-policy: outside-contract
        error!(error = %e, "grpc TLS file read failed");
        CamelError::EndpointCreationFailed(format!("failed to read {label}: {e}"))
    })
}

// ── URI param parsing helpers ──────────────────────────────────────────────

fn parse_bool_param(val: &str) -> Result<bool, CamelError> {
    match val.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Ok(true),
        "false" | "0" | "no" => Ok(false),
        _ => Err(CamelError::Config(format!("invalid bool value: {val}"))),
    }
}

fn parse_numeric_param<T: std::str::FromStr>(val: &str, field: &str) -> Result<T, CamelError>
where
    T::Err: std::fmt::Display,
{
    val.parse::<T>()
        .map_err(|e| CamelError::Config(format!("invalid numeric value for {field}: {val} ({e})")))
}

/// Parse query pairs into a typed `GrpcConfig`, handling bool and numeric
/// params natively instead of relying on serde string→bool/number coercion.
fn parse_grpc_query_params(
    pairs: impl Iterator<Item = (String, String)>,
) -> Result<GrpcConfig, CamelError> {
    let mut map: HashMap<String, String> = HashMap::new();
    for (k, v) in pairs {
        map.insert(k, v);
    }

    let proto_file = map.remove("protoFile");
    let service = map.remove("service");
    let method = map.remove("method");
    let metadata = map.remove("metadata");

    let reflection = map
        .remove("reflection")
        .map(|v| parse_bool_param(&v))
        .transpose()?
        .unwrap_or(false);

    let transport_val = map.remove("transport").ok_or_else(|| {
        CamelError::Config(
            "gRPC URI missing required 'transport' parameter. \
                 Use transport=plaintext (explicit h2c) or transport=tls \
                 (TLS). Omitted transport is rejected (ADR-0033)."
                .to_string(),
        )
    })?;

    if map.remove("tls").is_some() {
        return Err(CamelError::Config(
            "gRPC URI uses legacy 'tls' parameter — removed. \
             Use transport=plaintext|tls (ADR-0033)."
                .to_string(),
        ));
    }

    let ca_cert_path = map.remove("caCertPath");
    let client_cert_path = map.remove("clientCertPath");
    let client_key_path = map.remove("clientKeyPath");
    let client_server_name = map.remove("serverName");
    let server_cert_path = map.remove("serverCertPath");
    let server_key_path = map.remove("serverKeyPath");
    let client_ca_path = map.remove("clientCaPath");

    let (transport_intent, client_transport, server_transport) = match transport_val.as_str() {
        "plaintext" => {
            if [
                ca_cert_path.as_ref(),
                client_cert_path.as_ref(),
                client_key_path.as_ref(),
                client_server_name.as_ref(),
                client_ca_path.as_ref(),
            ]
            .iter()
            .any(|v| v.is_some())
            {
                return Err(CamelError::Config(
                    "gRPC transport=plaintext with client cert params — conflicting intent. \
                     Remove caCertPath/clientCertPath/clientKeyPath/serverName/clientCaPath."
                        .to_string(),
                ));
            }
            if [server_cert_path.as_ref(), server_key_path.as_ref()]
                .iter()
                .any(|v| v.is_some())
            {
                return Err(CamelError::Config(
                    "gRPC transport=plaintext with server cert params — conflicting intent. \
                     Remove serverCertPath/serverKeyPath."
                        .to_string(),
                ));
            }
            (
                TransportIntent::Plaintext,
                ClientTransport::Plaintext,
                ServerTransport::Plaintext,
            )
        }
        "tls" => {
            let client = ClientTransport::Tls(ClientTlsConfig {
                server_name: client_server_name,
                ca_cert_path,
                client_cert_path,
                client_key_path,
                insecure_skip_verify: false,
            });
            let server = match (server_cert_path, server_key_path) {
                (Some(cert), Some(key)) => ServerTransport::Tls(ServerTlsConfig {
                    server_cert_path: cert,
                    server_key_path: key,
                    client_ca_path,
                }),
                (None, None) => {
                    if client_ca_path.is_some() {
                        return Err(CamelError::Config(
                            "gRPC transport=tls with clientCaPath requires serverCertPath + \
                             serverKeyPath (inbound TLS). clientCaPath cannot apply to \
                             plaintext serve."
                                .to_string(),
                        ));
                    }
                    ServerTransport::Plaintext
                }
                _ => {
                    return Err(CamelError::Config(
                        "gRPC transport=tls inbound requires BOTH serverCertPath and \
                         serverKeyPath (or neither for plaintext serve)."
                            .to_string(),
                    ));
                }
            };
            (TransportIntent::Tls, client, server)
        }
        other => {
            return Err(CamelError::Config(format!(
                "gRPC invalid transport='{other}'. Use transport=plaintext|tls."
            )));
        }
    };

    let max_receive_message_length = map
        .remove("max_receive_message_length")
        .map(|v| parse_numeric_param(&v, "max_receive_message_length"))
        .transpose()?
        .unwrap_or_else(default_max_msg_len);

    let deadline_ms = map
        .remove("deadline_ms")
        .map(|v| parse_numeric_param(&v, "deadline_ms"))
        .transpose()?;

    let connect_timeout_ms = map
        .remove("connectTimeoutMs")
        .map(|v| parse_numeric_param(&v, "connectTimeoutMs"))
        .transpose()?
        .unwrap_or_else(default_connect_timeout_ms);

    let default_deadline_ms = map
        .remove("defaultDeadlineMs")
        .map(|v| parse_numeric_param(&v, "defaultDeadlineMs"))
        .transpose()?
        .unwrap_or_else(default_deadline_ms);

    // GRPC-007: Parse auth from query params
    let auth = if let Some(token) = map.remove("bearerToken") {
        AuthConfig::Bearer { token }
    } else if let Some(json_path) = map.remove("googleServiceAccount") {
        AuthConfig::GoogleServiceAccount { json_path }
    } else {
        AuthConfig::None
    };

    // GRPC-009: Parse strategies
    let consumer_strategy = map
        .remove("consumerStrategy")
        .map(|v| ConsumerStrategy::from_str(&v))
        .transpose()?
        .unwrap_or_default();

    let producer_strategy = map
        .remove("producerStrategy")
        .map(|v| ProducerStrategy::from_str(&v))
        .transpose()?
        .unwrap_or_default();

    // Warn about any unrecognized params
    for (k, v) in &map {
        tracing::warn!("unrecognized gRPC URI parameter '{k}={v}' — ignored");
    }

    Ok(GrpcConfig {
        proto_file,
        service,
        method,
        reflection,
        transport_intent,
        client_transport,
        server_transport,
        max_receive_message_length,
        deadline_ms,
        metadata,
        connect_timeout_ms,
        default_deadline_ms,
        auth,
        interceptors: InterceptorConfig::default(),
        consumer_strategy,
        producer_strategy,
        retry: NetworkRetryPolicy::default(),
    })
}

pub fn parse_grpc_uri(uri: &str) -> Result<(String, u16, String, String, GrpcConfig), CamelError> {
    let parsed = url::Url::parse(uri).map_err(|e| CamelError::RouteError(e.to_string()))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| CamelError::RouteError("missing host".to_string()))?
        .to_string();
    let port = parsed
        .port()
        .ok_or_else(|| CamelError::RouteError("missing port".to_string()))?;
    let path = parsed.path().trim_start_matches('/');
    let (service, method) = path.rsplit_once('/').ok_or_else(|| {
        CamelError::RouteError("URI path must be package.Service/Method".to_string())
    })?;
    let config = parse_grpc_query_params(
        parsed
            .query_pairs()
            .map(|(k, v)| (k.to_string(), v.to_string())),
    )?;
    if let Some(ref proto) = config.proto_file
        && (proto.starts_with('/') || proto.contains(".."))
    {
        return Err(CamelError::RouteError(format!(
            "proto path '{}' must be relative and cannot contain '..'",
            proto
        )));
    }
    if config.reflection {
        tracing::warn!("gRPC reflection is not supported in v1 — parameter ignored");
    }
    Ok((host, port, service.to_string(), method.to_string(), config))
}

/// Apply config-level metadata to a tonic request (GRPC-004).
///
/// Parses `config.metadata` as `key1=value1,key2=value2` and injects
/// each entry into the request's metadata map.
pub fn apply_config_metadata<T>(config: &GrpcConfig, request: &mut tonic::Request<T>) {
    if let Some(ref metadata_str) = config.metadata {
        for pair in metadata_str.split(',') {
            let pair = pair.trim();
            if let Some((key, value)) = pair.split_once('=') {
                let key = key.trim();
                let value = value.trim();
                if let Ok(name) = tonic::metadata::MetadataKey::from_bytes(key.as_bytes())
                    && let Ok(meta_val) = tonic::metadata::MetadataValue::try_from(value)
                {
                    request.metadata_mut().insert(name, meta_val);
                    tracing::debug!(key = key, "applied config metadata to gRPC request");
                }
            }
        }
    }
}

/// Apply auth headers from `AuthConfig` to a tonic request (GRPC-007).
///
/// Returns `Err` if auth is configured but cannot be applied (e.g. OAuth2
/// token acquisition fails). Fail-closed: callers that configured auth
/// expect authenticated requests.
pub async fn apply_auth_metadata<T>(
    auth: &AuthConfig,
    request: &mut tonic::Request<T>,
) -> Result<(), camel_api::CamelError> {
    match auth {
        AuthConfig::Bearer { token } => {
            if let Ok(name) = tonic::metadata::MetadataKey::from_bytes("authorization".as_bytes()) {
                let value = format!("Bearer {token}"); // allow-secret
                if let Ok(meta_val) = tonic::metadata::MetadataValue::try_from(value.as_str()) {
                    request.metadata_mut().insert(name, meta_val);
                    tracing::debug!("applied bearer auth to gRPC request");
                } else {
                    return Err(camel_api::CamelError::ProcessorError(
                        "bearer token contains invalid characters".into(),
                    ));
                }
            }
        }
        AuthConfig::OAuth2 { token_provider } => {
            let token = token_provider.get_token().await.map_err(|e| {
                let message = format!("failed to acquire OAuth2 token for gRPC producer: {e}"); // allow-secret
                camel_api::CamelError::ProcessorError(message)
            })?;
            if let Ok(name) = tonic::metadata::MetadataKey::from_bytes("authorization".as_bytes()) {
                let value = format!("Bearer {token}"); // allow-secret
                if let Ok(meta_val) = tonic::metadata::MetadataValue::try_from(value.as_str()) {
                    request.metadata_mut().insert(name, meta_val);
                }
            }
        }
        AuthConfig::None | AuthConfig::GoogleServiceAccount { .. } => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_grpc_uri_valid() {
        let uri = "grpc://localhost:50051/com.example.MyService/MyMethod?transport=plaintext";
        let (host, port, service, method, config) = parse_grpc_uri(uri).unwrap();
        assert_eq!(host, "localhost");
        assert_eq!(port, 50051);
        assert_eq!(service, "com.example.MyService");
        assert_eq!(method, "MyMethod");
        assert_eq!(config.max_receive_message_length, 4 * 1024 * 1024);
        assert!(!config.reflection);
        assert_eq!(config.transport_intent, TransportIntent::Plaintext);
    }

    #[test]
    fn test_parse_grpc_uri_bool_query_params_case_insensitive() {
        let uri = "grpc://localhost:50051/pkg.Svc/Method?reflection=true&transport=plaintext";
        let (_, _, _, _, config) = parse_grpc_uri(uri).unwrap();
        assert!(config.reflection);

        let uri = "grpc://localhost:50051/pkg.Svc/Method?reflection=TRUE&transport=plaintext";
        let (_, _, _, _, config) = parse_grpc_uri(uri).unwrap();
        assert!(config.reflection);

        let uri = "grpc://localhost:50051/pkg.Svc/Method?reflection=1&transport=plaintext";
        let (_, _, _, _, config) = parse_grpc_uri(uri).unwrap();
        assert!(config.reflection);

        let uri = "grpc://localhost:50051/pkg.Svc/Method?reflection=yes&transport=plaintext";
        let (_, _, _, _, config) = parse_grpc_uri(uri).unwrap();
        assert!(config.reflection);

        let uri = "grpc://localhost:50051/pkg.Svc/Method?reflection=false&transport=plaintext";
        let (_, _, _, _, config) = parse_grpc_uri(uri).unwrap();
        assert!(!config.reflection);

        let uri = "grpc://localhost:50051/pkg.Svc/Method?reflection=0&transport=plaintext";
        let (_, _, _, _, config) = parse_grpc_uri(uri).unwrap();
        assert!(!config.reflection);

        let uri = "grpc://localhost:50051/pkg.Svc/Method?reflection=no&transport=plaintext";
        let (_, _, _, _, config) = parse_grpc_uri(uri).unwrap();
        assert!(!config.reflection);
    }

    #[test]
    fn test_parse_grpc_uri_bool_query_params_invalid() {
        let uri = "grpc://localhost:50051/pkg.Svc/Method?reflection=maybe&transport=plaintext";
        let result = parse_grpc_uri(uri);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("invalid bool value")
        );
    }

    #[test]
    fn test_parse_grpc_uri_with_proto_file() {
        let uri = "grpc://localhost:50051/pkg.Svc/Method?protoFile=my.proto&transport=plaintext";
        let (_, _, _, _, config) = parse_grpc_uri(uri).unwrap();
        assert_eq!(config.proto_file, Some("my.proto".to_string()));
    }

    #[test]
    fn test_parse_grpc_uri_numeric_query_params_work() {
        let uri = "grpc://localhost:50051/pkg.Svc/Method?max_receive_message_length=8388608&transport=plaintext";
        let (_, _, _, _, config) = parse_grpc_uri(uri).unwrap();
        assert_eq!(config.max_receive_message_length, 8388608);

        let uri = "grpc://localhost:50051/pkg.Svc/Method?deadline_ms=5000&transport=plaintext";
        let (_, _, _, _, config) = parse_grpc_uri(uri).unwrap();
        assert_eq!(config.deadline_ms, Some(5000));
    }

    #[test]
    fn test_parse_grpc_uri_connect_timeout_and_default_deadline() {
        let uri = "grpc://localhost:50051/pkg.Svc/Method?connectTimeoutMs=5000&defaultDeadlineMs=15000&transport=plaintext";
        let (_, _, _, _, config) = parse_grpc_uri(uri).unwrap();
        assert_eq!(config.connect_timeout_ms, 5000);
        assert_eq!(config.default_deadline_ms, 15000);
    }

    #[test]
    fn test_parse_grpc_uri_numeric_query_params_invalid() {
        let uri =
            "grpc://localhost:50051/pkg.Svc/Method?deadline_ms=notanumber&transport=plaintext";
        let result = parse_grpc_uri(uri);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("invalid numeric value")
        );
    }

    #[test]
    fn test_parse_grpc_uri_with_metadata() {
        let uri = "grpc://localhost:50051/pkg.Svc/Method?metadata=some-value&transport=plaintext";
        let (_, _, _, _, config) = parse_grpc_uri(uri).unwrap();
        assert_eq!(config.metadata, Some("some-value".to_string()));
    }

    #[test]
    fn test_parse_grpc_uri_invalid_uri() {
        let result = parse_grpc_uri("not-a-valid-uri");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("relative URL"));
    }

    #[test]
    fn test_parse_grpc_uri_missing_host() {
        let result = parse_grpc_uri("grpc:/pkg.Svc/Method");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("missing host") || err.contains("empty host"));
    }

    #[test]
    fn test_parse_grpc_uri_missing_port() {
        let result = parse_grpc_uri("grpc://localhost/pkg.Svc/Method?transport=plaintext");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("missing port"));
    }

    #[test]
    fn test_parse_grpc_uri_missing_method_separator() {
        let result = parse_grpc_uri("grpc://localhost:50051/NoSlashHere?transport=plaintext");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("package.Service/Method")
        );
    }

    #[test]
    fn test_parse_grpc_uri_proto_absolute_path_rejected() {
        let uri = "grpc://localhost:50051/pkg.Svc/Method?protoFile=/etc/passwd&transport=plaintext";
        let result = parse_grpc_uri(uri);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("proto path"));
    }

    #[test]
    fn test_parse_grpc_uri_proto_traversal_rejected() {
        let uri =
            "grpc://localhost:50051/pkg.Svc/Method?protoFile=../secret.proto&transport=plaintext";
        let result = parse_grpc_uri(uri);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains(".."));
    }

    /// C1 Batch 1: `tls=true` via URI is rejected at parse time — a URI
    /// cannot carry a tls_config (certificates), so `tls=true` from a URI
    /// can never be satisfied and MUST fail-closed instead of silently
    /// running h2c. Explicit plaintext (`tls=false` or omitted) still parses.
    #[test]
    fn test_parse_grpc_uri_tls_true_without_config_rejected() {
        let uri = "grpc://localhost:50051/pkg.Svc/Method?tls=true&transport=plaintext";
        let result = parse_grpc_uri(uri);
        assert!(
            result.is_err(),
            "tls=true via URI must fail-closed at parse"
        );
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("tls"), "error must mention tls: {msg}");
    }

    #[test]
    fn test_grpc_config_defaults_via_deserialize() {
        // ADR-0033: transport fields are REQUIRED — omitting them must error.
        let result: Result<GrpcConfig, _> = serde_json::from_value(serde_json::json!({}));
        assert!(
            result.is_err(),
            "GrpcConfig without explicit transport must fail (ADR-0033)"
        );

        // With explicit transports, defaults work:
        let config: GrpcConfig = serde_json::from_value(serde_json::json!({
            "client_transport": "Plaintext",
            "server_transport": "Plaintext"
        }))
        .unwrap();
        assert_eq!(config.max_receive_message_length, 4 * 1024 * 1024);
        assert!(!config.reflection);
        assert_eq!(config.transport_intent, TransportIntent::Plaintext);
        assert!(config.proto_file.is_none());
        assert!(config.service.is_none());
        assert!(config.method.is_none());
        assert!(config.deadline_ms.is_none());
        assert!(config.metadata.is_none());
        assert_eq!(config.connect_timeout_ms, 10_000);
        assert_eq!(config.default_deadline_ms, 30_000);
    }

    #[test]
    fn test_grpc_config_deserialize_all_fields() {
        let config: GrpcConfig = serde_json::from_value(serde_json::json!({
            "protoFile": "test.proto",
            "service": "MyService",
            "method": "MyMethod",
            "reflection": true,
            "transport_intent": "Tls",
            "client_transport": {"Tls": {"server_name": "example.com"}},
            "server_transport": "Plaintext",
            "max_receive_message_length": 1024,
            "deadline_ms": 3000,
            "metadata": "auth-token"
        }))
        .unwrap();
        assert_eq!(config.proto_file, Some("test.proto".to_string()));
        assert_eq!(config.service, Some("MyService".to_string()));
        assert_eq!(config.method, Some("MyMethod".to_string()));
        assert!(config.reflection);
        assert_eq!(config.transport_intent, TransportIntent::Tls);
        assert_eq!(config.max_receive_message_length, 1024);
        assert_eq!(config.deadline_ms, Some(3000));
        assert_eq!(config.metadata, Some("auth-token".to_string()));
    }

    #[test]
    fn test_grpc_config_clone_and_debug() {
        let config: GrpcConfig = serde_json::from_value(serde_json::json!({
            "protoFile": "test.proto",
            "client_transport": "Plaintext",
            "server_transport": "Plaintext"
        }))
        .unwrap();
        let cloned = config.clone();
        assert_eq!(config.proto_file, cloned.proto_file);
        let debug_str = format!("{config:?}");
        assert!(debug_str.contains("GrpcConfig"));
    }

    // ── GrpcServerConfig tests ─────────────────────────────────────────────

    #[test]
    fn test_server_config_default() {
        let cfg = GrpcServerConfig::default();
        assert!(cfg.max_receive_message_len.is_none());
        assert!(matches!(cfg.transport, ServerTransport::Plaintext));
    }

    #[test]
    fn test_server_config_max_receive_message_len_applied() {
        let cfg = GrpcServerConfig {
            max_receive_message_len: Some(4096),
            transport: ServerTransport::Plaintext,
        };
        assert_eq!(cfg.max_receive_message_len, Some(4096));
    }

    #[test]
    fn test_server_config_clone_and_debug() {
        let cfg = GrpcServerConfig {
            max_receive_message_len: Some(8192),
            transport: ServerTransport::Plaintext,
        };
        let cloned = cfg.clone();
        assert_eq!(cfg.max_receive_message_len, cloned.max_receive_message_len);
        let debug_str = format!("{cfg:?}");
        assert!(debug_str.contains("GrpcServerConfig"));
    }

    // ── parse_bool_param tests ─────────────────────────────────────────────

    #[test]
    fn test_bool_param_case_insensitive() {
        assert!(parse_bool_param("True").unwrap());
        assert!(!parse_bool_param("FALSE").unwrap());
        assert!(parse_bool_param("1").unwrap());
        assert!(!parse_bool_param("0").unwrap());
        assert!(parse_bool_param("yes").unwrap());
        assert!(!parse_bool_param("no").unwrap());
        assert!(parse_bool_param("YES").unwrap());
        assert!(!parse_bool_param("NO").unwrap());
    }

    #[test]
    fn test_bool_param_invalid_values() {
        assert!(parse_bool_param("maybe").is_err());
        assert!(parse_bool_param("").is_err());
        assert!(parse_bool_param("2").is_err());
        assert!(parse_bool_param("-1").is_err());
    }

    // ── GRPC-004: apply_config_metadata tests ──────────────────────────────

    #[test]
    fn test_apply_config_metadata_single_pair() {
        let config: GrpcConfig = serde_json::from_value(serde_json::json!({
            "metadata": "x-custom=hello",
            "client_transport": "Plaintext",
            "server_transport": "Plaintext"
        }))
        .unwrap();
        let mut request = tonic::Request::new(());
        apply_config_metadata(&config, &mut request);
        assert_eq!(
            request
                .metadata()
                .get("x-custom")
                .unwrap()
                .to_str()
                .unwrap(),
            "hello"
        );
    }

    #[test]
    fn test_apply_config_metadata_multiple_pairs() {
        let config: GrpcConfig = serde_json::from_value(serde_json::json!({
            "metadata": "x-a=1, x-b=2",
            "client_transport": "Plaintext",
            "server_transport": "Plaintext"
        }))
        .unwrap();
        let mut request = tonic::Request::new(());
        apply_config_metadata(&config, &mut request);
        assert_eq!(
            request.metadata().get("x-a").unwrap().to_str().unwrap(),
            "1"
        );
        assert_eq!(
            request.metadata().get("x-b").unwrap().to_str().unwrap(),
            "2"
        );
    }

    #[test]
    fn test_apply_config_metadata_empty_metadata() {
        let config: GrpcConfig =
            serde_json::from_value(serde_json::json!({"metadata": "", "client_transport": "Plaintext", "server_transport": "Plaintext"})).unwrap();
        let mut request = tonic::Request::new(());
        apply_config_metadata(&config, &mut request);
        assert!(request.metadata().is_empty());
    }

    // ── ADR-0033: ClientTlsConfig tests ─────────────────────────────────────

    #[test]
    fn test_client_tls_config_default() {
        let tls = ClientTlsConfig::default();
        assert!(tls.ca_cert_path.is_none());
        assert!(tls.client_cert_path.is_none());
        assert!(tls.client_key_path.is_none());
        assert!(!tls.insecure_skip_verify);
        assert!(tls.server_name.is_none());
    }

    #[test]
    fn test_client_tls_config_deserialize() {
        let tls: ClientTlsConfig = serde_json::from_value(serde_json::json!({
            "ca_cert_path": "/path/to/ca.pem",
            "client_cert_path": "/path/to/client.pem",
            "client_key_path": "/path/to/client.key",
            "insecure_skip_verify": true,
            "server_name": "grpc.example.com"
        }))
        .unwrap();
        assert_eq!(tls.ca_cert_path, Some("/path/to/ca.pem".to_string()));
        assert_eq!(
            tls.client_cert_path,
            Some("/path/to/client.pem".to_string())
        );
        assert_eq!(tls.client_key_path, Some("/path/to/client.key".to_string()));
        assert!(tls.insecure_skip_verify);
        assert_eq!(tls.server_name, Some("grpc.example.com".to_string()));
    }

    #[test]
    fn test_client_tls_config_clone_and_debug() {
        let tls = ClientTlsConfig {
            ca_cert_path: Some("/ca.pem".to_string()),
            client_cert_path: None,
            client_key_path: None,
            insecure_skip_verify: false,
            server_name: None,
        };
        let cloned = tls.clone();
        assert_eq!(tls.ca_cert_path, cloned.ca_cert_path);
        let debug_str = format!("{tls:?}");
        assert!(debug_str.contains("ClientTlsConfig"));
    }

    // ── GRPC-007: AuthConfig tests ─────────────────────────────────────────

    #[test]
    fn test_auth_config_default_is_none() {
        let auth = AuthConfig::default();
        assert!(matches!(auth, AuthConfig::None));
    }

    #[tokio::test]
    async fn test_auth_config_bearer_applies_metadata() {
        let auth = AuthConfig::Bearer {
            token: "my-secret-token".to_string(),
        };
        let mut request = tonic::Request::new(());
        apply_auth_metadata(&auth, &mut request).await.unwrap();
        let val = request
            .metadata()
            .get("authorization")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(val, "Bearer my-secret-token");
    }

    #[tokio::test]
    async fn test_auth_config_none_no_metadata() {
        let auth = AuthConfig::None;
        let mut request = tonic::Request::new(());
        apply_auth_metadata(&auth, &mut request).await.unwrap();
        assert!(request.metadata().get("authorization").is_none());
    }

    #[tokio::test]
    async fn test_auth_config_google_scaffold_no_metadata() {
        let auth = AuthConfig::GoogleServiceAccount {
            json_path: "/path/to/sa.json".to_string(),
        };
        let mut request = tonic::Request::new(());
        apply_auth_metadata(&auth, &mut request).await.unwrap();
        assert!(request.metadata().get("authorization").is_none());
    }

    #[tokio::test]
    async fn test_auth_config_oauth2_sets_bearer() {
        #[derive(Debug)]
        struct MockProvider;
        #[async_trait::async_trait]
        impl camel_auth::TokenProvider for MockProvider {
            async fn get_token(&self) -> Result<String, camel_auth::AuthError> {
                Ok("mock-oauth2-token".to_string())
            }
        }
        let auth = AuthConfig::OAuth2 {
            token_provider: std::sync::Arc::new(MockProvider),
        };
        let mut request = tonic::Request::new(());
        apply_auth_metadata(&auth, &mut request).await.unwrap();
        let auth_header = request.metadata().get("authorization").unwrap();
        assert_eq!(auth_header, "Bearer mock-oauth2-token");
    }

    #[tokio::test]
    async fn test_auth_config_oauth2_failure_returns_error() {
        #[derive(Debug)]
        struct FailingProvider;
        #[async_trait::async_trait]
        impl camel_auth::TokenProvider for FailingProvider {
            async fn get_token(&self) -> Result<String, camel_auth::AuthError> {
                Err(camel_auth::AuthError::ProviderUnavailable(
                    "mock failure".into(),
                ))
            }
        }
        let auth = AuthConfig::OAuth2 {
            token_provider: std::sync::Arc::new(FailingProvider),
        };
        let mut request = tonic::Request::new(());
        let result = apply_auth_metadata(&auth, &mut request).await;
        assert!(result.is_err());
        assert!(request.metadata().get("authorization").is_none());
    }

    // ── GRPC-008: InterceptorConfig tests ──────────────────────────────────

    #[test]
    fn test_interceptor_config_default_empty() {
        let ic = InterceptorConfig::default();
        assert!(ic.interceptors.is_empty());
    }

    #[test]
    fn test_interceptor_config_deserialize() {
        let ic: InterceptorConfig = serde_json::from_value(serde_json::json!({
            "interceptors": ["logging", "auth"]
        }))
        .unwrap();
        assert_eq!(ic.interceptors.len(), 2);
        assert_eq!(ic.interceptors[0], "logging");
        assert_eq!(ic.interceptors[1], "auth");
    }

    // ── GRPC-009: ConsumerStrategy tests ───────────────────────────────────

    #[test]
    fn test_consumer_strategy_default() {
        assert_eq!(ConsumerStrategy::default(), ConsumerStrategy::RoundRobin);
    }

    #[test]
    fn test_consumer_strategy_from_str() {
        assert_eq!(
            ConsumerStrategy::from_str("roundRobin").unwrap(),
            ConsumerStrategy::RoundRobin
        );
        assert_eq!(
            ConsumerStrategy::from_str("first").unwrap(),
            ConsumerStrategy::First
        );
        assert_eq!(
            ConsumerStrategy::from_str("last").unwrap(),
            ConsumerStrategy::Last
        );
    }

    #[test]
    fn test_consumer_strategy_display() {
        assert_eq!(ConsumerStrategy::RoundRobin.to_string(), "roundRobin");
        assert_eq!(ConsumerStrategy::First.to_string(), "first");
        assert_eq!(ConsumerStrategy::Last.to_string(), "last");
    }

    #[test]
    fn test_consumer_strategy_invalid() {
        assert!(ConsumerStrategy::from_str("invalid").is_err());
    }

    // ── GRPC-009: ProducerStrategy tests ───────────────────────────────────

    #[test]
    fn test_producer_strategy_default() {
        assert_eq!(ProducerStrategy::default(), ProducerStrategy::RequestReply);
    }

    #[test]
    fn test_producer_strategy_from_str() {
        assert_eq!(
            ProducerStrategy::from_str("fireAndForget").unwrap(),
            ProducerStrategy::FireAndForget
        );
        assert_eq!(
            ProducerStrategy::from_str("requestReply").unwrap(),
            ProducerStrategy::RequestReply
        );
    }

    #[test]
    fn test_producer_strategy_display() {
        assert_eq!(ProducerStrategy::FireAndForget.to_string(), "fireAndForget");
        assert_eq!(ProducerStrategy::RequestReply.to_string(), "requestReply");
    }

    #[test]
    fn test_producer_strategy_invalid() {
        assert!(ProducerStrategy::from_str("invalid").is_err());
    }

    // ── GRPC-013: Debug redaction tests ────────────────────────────────────

    #[test]
    fn test_grpc_config_debug_redacts_metadata() {
        let config: GrpcConfig = serde_json::from_value(serde_json::json!({
            "metadata": "secret-key=value",
            "client_transport": "Plaintext",
            "server_transport": "Plaintext"
        }))
        .unwrap();
        let debug_str = format!("{config:?}");
        assert!(debug_str.contains("[REDACTED]"));
        assert!(!debug_str.contains("secret-key=value"));
    }

    #[test]
    fn test_grpc_config_debug_no_redaction_without_secrets() {
        let config: GrpcConfig = serde_json::from_value(serde_json::json!({
            "protoFile": "test.proto",
            "client_transport": "Plaintext",
            "server_transport": "Plaintext"
        }))
        .unwrap();
        let debug_str = format!("{config:?}");
        assert!(debug_str.contains("test.proto"));
    }

    #[test]
    fn test_auth_config_debug_redacts_bearer_token() {
        let auth = AuthConfig::Bearer {
            token: "super-secret-token".to_string(),
        };
        let debug_str = format!("{auth:?}");
        assert!(debug_str.contains("[REDACTED]"));
        assert!(!debug_str.contains("super-secret-token"));
    }

    #[test]
    fn test_auth_config_debug_redacts_google_json_path() {
        let auth = AuthConfig::GoogleServiceAccount {
            json_path: "/secret/sa.json".to_string(),
        };
        let debug_str = format!("{auth:?}");
        assert!(debug_str.contains("[REDACTED]"));
        assert!(!debug_str.contains("/secret/sa.json"));
    }

    #[test]
    fn test_auth_config_debug_none_is_clean() {
        let auth = AuthConfig::None;
        let debug_str = format!("{auth:?}");
        assert_eq!(debug_str, "None");
    }

    // ── GRPC-007: Bearer token parsed from URI ─────────────────────────────

    #[test]
    fn test_parse_grpc_uri_bearer_token() {
        let uri = "grpc://localhost:50051/pkg.Svc/Method?bearerToken=my-token&transport=plaintext";
        let (_, _, _, _, config) = parse_grpc_uri(uri).unwrap();
        match config.auth {
            AuthConfig::Bearer { ref token } => assert_eq!(token, "my-token"),
            _ => panic!("expected Bearer auth"),
        }
    }

    // ── retry: NetworkRetryPolicy tests ────────────────────────────────────

    #[test]
    fn grpc_config_has_retry_policy_toml() {
        let toml_str = r#"
            protoFile = "helloworld.proto"
            service = "Greeter"
            method = "SayHello"
            client_transport = "Plaintext"
            server_transport = "Plaintext"
            [retry]
            max_attempts = 3
            initial_delay_ms = 500
        "#;
        let cfg: GrpcConfig = toml::from_str(toml_str).expect("parse");
        assert_eq!(cfg.retry.max_attempts, 3);
        assert_eq!(
            cfg.retry.initial_delay,
            std::time::Duration::from_millis(500)
        );
    }

    #[test]
    fn grpc_config_retry_defaults_when_not_specified() {
        let toml_str = r#"
            protoFile = "helloworld.proto"
            client_transport = "Plaintext"
            server_transport = "Plaintext"
        "#;
        let cfg: GrpcConfig = toml::from_str(toml_str).expect("parse");
        assert_eq!(
            cfg.retry,
            camel_component_api::NetworkRetryPolicy::default()
        );
    }

    // ── GRPC-009: Strategy parsed from URI ─────────────────────────────────

    #[test]
    fn test_parse_grpc_uri_consumer_strategy() {
        let uri =
            "grpc://localhost:50051/pkg.Svc/Method?consumerStrategy=first&transport=plaintext";
        let (_, _, _, _, config) = parse_grpc_uri(uri).unwrap();
        assert_eq!(config.consumer_strategy, ConsumerStrategy::First);
    }

    #[test]
    fn test_parse_grpc_uri_producer_strategy() {
        let uri = "grpc://localhost:50051/pkg.Svc/Method?producerStrategy=fireAndForget&transport=plaintext";
        let (_, _, _, _, config) = parse_grpc_uri(uri).unwrap();
        assert_eq!(config.producer_strategy, ProducerStrategy::FireAndForget);
    }

    // ── Fail-closed regression tests (rc-j0gl, rc-vnrl, rc-xp76) ───────────

    /// Regression: omitted transport must error (ADR-0033).
    #[test]
    fn test_parse_rejects_omitted_transport() {
        let res =
            parse_grpc_uri("grpc://127.0.0.1:50051/helloworld.Greeter/SayHello?protoFile=a.proto");
        assert!(res.is_err(), "omitted transport must fail-closed");
        assert!(
            res.unwrap_err().to_string().contains("transport"),
            "error must mention transport"
        );
    }

    /// Regression: legacy tls=true key rejected even alongside transport=tls.
    #[test]
    fn test_parse_rejects_legacy_tls_key_alone() {
        let res = parse_grpc_uri(
            "grpc://127.0.0.1:50051/helloworld.Greeter/SayHello?protoFile=a.proto&tls=true",
        );
        assert!(res.is_err(), "legacy tls= alone must fail");
        assert!(
            res.unwrap_err().to_string().contains("tls"),
            "error must mention tls"
        );
    }

    /// Regression: clientCaPath without server certs rejected at parse time.
    #[test]
    fn test_parse_rejects_client_ca_without_server_certs() {
        let res = parse_grpc_uri(
            "grpc://127.0.0.1:50051/pkg.Svc/Method?transport=tls&clientCaPath=/ca.pem",
        );
        assert!(
            res.is_err(),
            "clientCaPath without serverCertPath/serverKeyPath must fail at parse"
        );
        let msg = res.unwrap_err().to_string();
        assert!(
            msg.contains("clientCaPath") || msg.contains("serverCertPath"),
            "error must name the missing params: {msg}"
        );
    }

    /// Regression: transport=tls with serverCertPath but not serverKeyPath → error.
    #[test]
    fn test_parse_rejects_tls_with_server_cert_only() {
        let res = parse_grpc_uri(
            "grpc://127.0.0.1:50051/pkg.Svc/Method?transport=tls&serverCertPath=/c.pem",
        );
        assert!(
            res.is_err(),
            "serverCertPath without serverKeyPath must fail"
        );
        let msg = res.unwrap_err().to_string();
        assert!(
            msg.contains("serverCertPath") && msg.contains("serverKeyPath"),
            "error must name both params: {msg}"
        );
    }

    /// Regression: transport=plaintext with caCertPath → conflicting-intent error.
    #[test]
    fn test_parse_rejects_plaintext_with_ca_cert() {
        let res = parse_grpc_uri(
            "grpc://127.0.0.1:50051/pkg.Svc/Method?transport=plaintext&caCertPath=/ca.pem",
        );
        assert!(res.is_err(), "plaintext + caCertPath must conflict");
        assert!(
            res.unwrap_err().to_string().contains("conflicting"),
            "error must mention conflicting intent"
        );
    }

    /// Regression: transport=plaintext with serverCertPath → conflicting-intent error.
    #[test]
    fn test_parse_rejects_plaintext_with_server_cert() {
        let res = parse_grpc_uri(
            "grpc://127.0.0.1:50051/pkg.Svc/Method?transport=plaintext&serverCertPath=/cert.pem",
        );
        assert!(res.is_err(), "plaintext + serverCertPath must conflict");
        assert!(
            res.unwrap_err().to_string().contains("conflicting"),
            "error must mention conflicting intent"
        );
    }

    /// Regression: transport=plaintext with clientCertPath → conflicting-intent error.
    #[test]
    fn test_parse_rejects_plaintext_with_client_cert() {
        let res = parse_grpc_uri(
            "grpc://127.0.0.1:50051/pkg.Svc/Method?transport=plaintext&clientCertPath=/cert.pem",
        );
        assert!(res.is_err(), "plaintext + clientCertPath must conflict");
        assert!(
            res.unwrap_err().to_string().contains("conflicting"),
            "error must mention conflicting intent"
        );
    }

    /// Regression: transport=plaintext with clientKeyPath → conflicting-intent error.
    #[test]
    fn test_parse_rejects_plaintext_with_client_key() {
        let res = parse_grpc_uri(
            "grpc://127.0.0.1:50051/pkg.Svc/Method?transport=plaintext&clientKeyPath=/key.pem",
        );
        assert!(res.is_err(), "plaintext + clientKeyPath must conflict");
        assert!(
            res.unwrap_err().to_string().contains("conflicting"),
            "error must mention conflicting intent"
        );
    }

    /// Regression: transport=plaintext with serverName → conflicting-intent error.
    #[test]
    fn test_parse_rejects_plaintext_with_server_name() {
        let res = parse_grpc_uri(
            "grpc://127.0.0.1:50051/pkg.Svc/Method?transport=plaintext&serverName=example.com",
        );
        assert!(res.is_err(), "plaintext + serverName must conflict");
        assert!(
            res.unwrap_err().to_string().contains("conflicting"),
            "error must mention conflicting intent"
        );
    }

    /// Regression: transport=plaintext with clientCaPath → conflicting-intent error.
    #[test]
    fn test_parse_rejects_plaintext_with_client_ca() {
        let res = parse_grpc_uri(
            "grpc://127.0.0.1:50051/pkg.Svc/Method?transport=plaintext&clientCaPath=/ca.pem",
        );
        assert!(res.is_err(), "plaintext + clientCaPath must conflict");
        assert!(
            res.unwrap_err().to_string().contains("conflicting"),
            "error must mention conflicting intent"
        );
    }
}
