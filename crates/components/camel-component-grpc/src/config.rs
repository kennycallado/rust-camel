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
