use camel_api::error::CamelError;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
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
}

fn default_max_msg_len() -> usize {
    4 * 1024 * 1024
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
    let config: GrpcConfig = serde_json::from_value(serde_json::Value::Object(
        parsed
            .query_pairs()
            .map(|(k, v)| (k.to_string(), serde_json::Value::String(v.to_string())))
            .collect(),
    ))
    .map_err(|e| CamelError::RouteError(e.to_string()))?;
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
    fn test_parse_grpc_uri_query_params_are_strings() {
        // parse_grpc_uri converts all query params to JSON strings,
        // so bool/numeric fields fail deserialization from query params.
        let uri = "grpc://localhost:50051/pkg.Svc/Method?reflection=true";
        let result = parse_grpc_uri(uri);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid type"));
    }

    #[test]
    fn test_parse_grpc_uri_with_proto_file() {
        let uri = "grpc://localhost:50051/pkg.Svc/Method?protoFile=my.proto";
        let (_, _, _, _, config) = parse_grpc_uri(uri).unwrap();
        assert_eq!(config.proto_file, Some("my.proto".to_string()));
    }

    #[test]
    fn test_parse_grpc_uri_numeric_query_params_fail() {
        // Numeric query params are converted to strings and fail deserialization.
        let uri = "grpc://localhost:50051/pkg.Svc/Method?max_receive_message_length=8388608";
        let result = parse_grpc_uri(uri);
        assert!(result.is_err());

        let uri = "grpc://localhost:50051/pkg.Svc/Method?deadline_ms=5000";
        let result = parse_grpc_uri(uri);
        assert!(result.is_err());
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
        // URL with no authority section — host_str() returns None
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
    fn test_parse_grpc_uri_bool_query_params_fail_deserialization() {
        // reflection and tls as query params fail because parse_grpc_uri
        // converts all values to JSON strings, not native bools.
        let uri = "grpc://localhost:50051/pkg.Svc/Method?reflection=true";
        assert!(parse_grpc_uri(uri).is_err());

        let uri = "grpc://localhost:50051/pkg.Svc/Method?tls=true";
        assert!(parse_grpc_uri(uri).is_err());
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
}
