use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use camel_api::error::CamelError;
use camel_auth::oauth2::TokenProvider;
use camel_component_api::NetworkRetryPolicy;
use serde::Deserialize;
use serde::de::{self, MapAccess, Visitor};

// ── TLS configuration (GRPC-006) ──────────────────────────────────────────

/// TLS/mTLS configuration for gRPC channels.
///
/// When `tls_enabled` is `true`, the producer will attempt to establish a
/// TLS-secured connection using the provided certificate paths.
///
/// /// TODO(GRPC-006): load cert files at runtime
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct TlsConfig {
    #[serde(default)]
    pub tls_enabled: bool,
    pub ca_cert_path: Option<String>,
    pub client_cert_path: Option<String>,
    pub client_key_path: Option<String>,
    #[serde(default)]
    pub insecure_skip_verify: bool,
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
    pub tls: bool,
    #[serde(default = "default_max_msg_len")]
    pub max_receive_message_length: usize,
    pub deadline_ms: Option<u64>,
    pub metadata: Option<String>,

    // GRPC-006: TLS/mTLS support
    #[serde(default)]
    pub tls_config: Option<TlsConfig>,

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
            .field("tls", &self.tls)
            .field(
                "max_receive_message_length",
                &self.max_receive_message_length,
            )
            .field("deadline_ms", &self.deadline_ms)
            .field("metadata", &self.metadata.as_ref().map(|_| "[REDACTED]"))
            .field("tls_config", &self.tls_config)
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

/// Server-side configuration for the gRPC transport layer.
#[derive(Debug, Clone, Default)]
pub struct GrpcServerConfig {
    /// Maximum incoming message size in bytes. None means use tonic/hyper default.
    pub max_receive_message_len: Option<usize>,
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

    let tls = map
        .remove("tls")
        .map(|v| parse_bool_param(&v))
        .transpose()?
        .unwrap_or(false);

    let max_receive_message_length = map
        .remove("max_receive_message_length")
        .map(|v| parse_numeric_param(&v, "max_receive_message_length"))
        .transpose()?
        .unwrap_or_else(default_max_msg_len);

    let deadline_ms = map
        .remove("deadline_ms")
        .map(|v| parse_numeric_param(&v, "deadline_ms"))
        .transpose()?;

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
        tls,
        max_receive_message_length,
        deadline_ms,
        metadata,
        tls_config: None,
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
    if config.tls {
        tracing::warn!("gRPC TLS is not supported in v1 — parameter ignored");
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
        let uri = "grpc://localhost:50051/com.example.MyService/MyMethod";
        let (host, port, service, method, config) = parse_grpc_uri(uri).unwrap();
        assert_eq!(host, "localhost");
        assert_eq!(port, 50051);
        assert_eq!(service, "com.example.MyService");
        assert_eq!(method, "MyMethod");
        assert_eq!(config.max_receive_message_length, 4 * 1024 * 1024);
        assert!(!config.reflection);
        assert!(!config.tls);
    }

    #[test]
    fn test_parse_grpc_uri_bool_query_params_case_insensitive() {
        let uri = "grpc://localhost:50051/pkg.Svc/Method?reflection=true";
        let (_, _, _, _, config) = parse_grpc_uri(uri).unwrap();
        assert!(config.reflection);

        let uri = "grpc://localhost:50051/pkg.Svc/Method?reflection=TRUE";
        let (_, _, _, _, config) = parse_grpc_uri(uri).unwrap();
        assert!(config.reflection);

        let uri = "grpc://localhost:50051/pkg.Svc/Method?reflection=1";
        let (_, _, _, _, config) = parse_grpc_uri(uri).unwrap();
        assert!(config.reflection);

        let uri = "grpc://localhost:50051/pkg.Svc/Method?reflection=yes";
        let (_, _, _, _, config) = parse_grpc_uri(uri).unwrap();
        assert!(config.reflection);

        let uri = "grpc://localhost:50051/pkg.Svc/Method?reflection=false";
        let (_, _, _, _, config) = parse_grpc_uri(uri).unwrap();
        assert!(!config.reflection);

        let uri = "grpc://localhost:50051/pkg.Svc/Method?reflection=0";
        let (_, _, _, _, config) = parse_grpc_uri(uri).unwrap();
        assert!(!config.reflection);

        let uri = "grpc://localhost:50051/pkg.Svc/Method?reflection=no";
        let (_, _, _, _, config) = parse_grpc_uri(uri).unwrap();
        assert!(!config.reflection);
    }

    #[test]
    fn test_parse_grpc_uri_bool_query_params_invalid() {
        let uri = "grpc://localhost:50051/pkg.Svc/Method?reflection=maybe";
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
        let uri = "grpc://localhost:50051/pkg.Svc/Method?protoFile=my.proto";
        let (_, _, _, _, config) = parse_grpc_uri(uri).unwrap();
        assert_eq!(config.proto_file, Some("my.proto".to_string()));
    }

    #[test]
    fn test_parse_grpc_uri_numeric_query_params_work() {
        let uri = "grpc://localhost:50051/pkg.Svc/Method?max_receive_message_length=8388608";
        let (_, _, _, _, config) = parse_grpc_uri(uri).unwrap();
        assert_eq!(config.max_receive_message_length, 8388608);

        let uri = "grpc://localhost:50051/pkg.Svc/Method?deadline_ms=5000";
        let (_, _, _, _, config) = parse_grpc_uri(uri).unwrap();
        assert_eq!(config.deadline_ms, Some(5000));
    }

    #[test]
    fn test_parse_grpc_uri_numeric_query_params_invalid() {
        let uri = "grpc://localhost:50051/pkg.Svc/Method?deadline_ms=notanumber";
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
        let uri = "grpc://localhost:50051/pkg.Svc/Method?metadata=some-value";
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
        let result = parse_grpc_uri("grpc://localhost/pkg.Svc/Method");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("missing port"));
    }

    #[test]
    fn test_parse_grpc_uri_missing_method_separator() {
        let result = parse_grpc_uri("grpc://localhost:50051/NoSlashHere");
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
        let uri = "grpc://localhost:50051/pkg.Svc/Method?protoFile=/etc/passwd";
        let result = parse_grpc_uri(uri);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("proto path"));
    }

    #[test]
    fn test_parse_grpc_uri_proto_traversal_rejected() {
        let uri = "grpc://localhost:50051/pkg.Svc/Method?protoFile=../secret.proto";
        let result = parse_grpc_uri(uri);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains(".."));
    }

    #[test]
    fn test_parse_grpc_uri_tls_bool_param() {
        let uri = "grpc://localhost:50051/pkg.Svc/Method?tls=true";
        let (_, _, _, _, config) = parse_grpc_uri(uri).unwrap();
        assert!(config.tls);
    }

    #[test]
    fn test_grpc_config_defaults_via_deserialize() {
        let config: GrpcConfig = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(config.max_receive_message_length, 4 * 1024 * 1024);
        assert!(!config.reflection);
        assert!(!config.tls);
        assert!(config.proto_file.is_none());
        assert!(config.service.is_none());
        assert!(config.method.is_none());
        assert!(config.deadline_ms.is_none());
        assert!(config.metadata.is_none());
    }

    #[test]
    fn test_grpc_config_deserialize_all_fields() {
        let config: GrpcConfig = serde_json::from_value(serde_json::json!({
            "protoFile": "test.proto",
            "service": "MyService",
            "method": "MyMethod",
            "reflection": true,
            "tls": true,
            "max_receive_message_length": 1024,
            "deadline_ms": 3000,
            "metadata": "auth-token"
        }))
        .unwrap();
        assert_eq!(config.proto_file, Some("test.proto".to_string()));
        assert_eq!(config.service, Some("MyService".to_string()));
        assert_eq!(config.method, Some("MyMethod".to_string()));
        assert!(config.reflection);
        assert!(config.tls);
        assert_eq!(config.max_receive_message_length, 1024);
        assert_eq!(config.deadline_ms, Some(3000));
        assert_eq!(config.metadata, Some("auth-token".to_string()));
    }

    #[test]
    fn test_grpc_config_clone_and_debug() {
        let config: GrpcConfig = serde_json::from_value(serde_json::json!({
            "protoFile": "test.proto"
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
    }

    #[test]
    fn test_server_config_max_receive_message_len_applied() {
        let cfg = GrpcServerConfig {
            max_receive_message_len: Some(4096),
        };
        assert_eq!(cfg.max_receive_message_len, Some(4096));
    }

    #[test]
    fn test_server_config_clone_and_debug() {
        let cfg = GrpcServerConfig {
            max_receive_message_len: Some(8192),
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
            "metadata": "x-custom=hello"
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
            "metadata": "x-a=1, x-b=2"
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
            serde_json::from_value(serde_json::json!({"metadata": ""})).unwrap();
        let mut request = tonic::Request::new(());
        apply_config_metadata(&config, &mut request);
        assert!(request.metadata().is_empty());
    }

    // ── GRPC-006: TlsConfig tests ──────────────────────────────────────────

    #[test]
    fn test_tls_config_default() {
        let tls = TlsConfig::default();
        assert!(!tls.tls_enabled);
        assert!(tls.ca_cert_path.is_none());
        assert!(tls.client_cert_path.is_none());
        assert!(tls.client_key_path.is_none());
        assert!(!tls.insecure_skip_verify);
    }

    #[test]
    fn test_tls_config_deserialize_enabled() {
        let tls: TlsConfig = serde_json::from_value(serde_json::json!({
            "tls_enabled": true,
            "ca_cert_path": "/path/to/ca.pem",
            "client_cert_path": "/path/to/client.pem",
            "client_key_path": "/path/to/client.key",
            "insecure_skip_verify": true
        }))
        .unwrap();
        assert!(tls.tls_enabled);
        assert_eq!(tls.ca_cert_path, Some("/path/to/ca.pem".to_string()));
        assert_eq!(
            tls.client_cert_path,
            Some("/path/to/client.pem".to_string())
        );
        assert_eq!(tls.client_key_path, Some("/path/to/client.key".to_string()));
        assert!(tls.insecure_skip_verify);
    }

    #[test]
    fn test_tls_config_clone_and_debug() {
        let tls = TlsConfig {
            tls_enabled: true,
            ca_cert_path: Some("/ca.pem".to_string()),
            client_cert_path: None,
            client_key_path: None,
            insecure_skip_verify: false,
        };
        let cloned = tls.clone();
        assert_eq!(tls.tls_enabled, cloned.tls_enabled);
        let debug_str = format!("{tls:?}");
        assert!(debug_str.contains("TlsConfig"));
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
            "metadata": "secret-key=value"
        }))
        .unwrap();
        let debug_str = format!("{config:?}");
        assert!(debug_str.contains("[REDACTED]"));
        assert!(!debug_str.contains("secret-key=value"));
    }

    #[test]
    fn test_grpc_config_debug_no_redaction_without_secrets() {
        let config: GrpcConfig = serde_json::from_value(serde_json::json!({
            "protoFile": "test.proto"
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
        let uri = "grpc://localhost:50051/pkg.Svc/Method?bearerToken=my-token";
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
        let uri = "grpc://localhost:50051/pkg.Svc/Method?consumerStrategy=first";
        let (_, _, _, _, config) = parse_grpc_uri(uri).unwrap();
        assert_eq!(config.consumer_strategy, ConsumerStrategy::First);
    }

    #[test]
    fn test_parse_grpc_uri_producer_strategy() {
        let uri = "grpc://localhost:50051/pkg.Svc/Method?producerStrategy=fireAndForget";
        let (_, _, _, _, config) = parse_grpc_uri(uri).unwrap();
        assert_eq!(config.producer_strategy, ProducerStrategy::FireAndForget);
    }
}
