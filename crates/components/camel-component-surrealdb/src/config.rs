//! Endpoint configuration types and URI parser for the SurrealDB component.

use std::collections::HashMap;

use camel_api::CamelError;
use url::form_urlencoded;

use crate::error::SurrealDbError;

const DEFAULT_VECTOR_FIELD: &str = "embedding";

/// The type of SurrealDB operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurrealDbOperation {
    Query,
    Select,
    Create,
    Update,
    Upsert,
    Delete,
    Patch,
    Relate,
    Vector,
    Search,
    Run,
    Live,
}

impl std::fmt::Display for SurrealDbOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Query => write!(f, "query"),
            Self::Select => write!(f, "select"),
            Self::Create => write!(f, "create"),
            Self::Update => write!(f, "update"),
            Self::Upsert => write!(f, "upsert"),
            Self::Delete => write!(f, "delete"),
            Self::Patch => write!(f, "patch"),
            Self::Relate => write!(f, "relate"),
            Self::Vector => write!(f, "vector"),
            Self::Search => write!(f, "search"),
            Self::Run => write!(f, "run"),
            Self::Live => write!(f, "live"),
        }
    }
}

/// Output format for query results.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum OutputType {
    /// Materialize results as `Body::Json(Value::Array(...))`.
    #[default]
    Json,
    /// Stream results as `Body::Stream(StreamBody)` — same pattern as SQL `StreamList`.
    StreamList,
}

/// Distance metric for vector similarity search.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum VectorMetric {
    #[default]
    Cosine,
    Euclidean,
    Manhattan,
}

impl VectorMetric {
    /// SurrealQL KNN operator metric name.
    pub fn as_surrealql(&self) -> &'static str {
        match self {
            Self::Cosine => "COSINE",
            Self::Euclidean => "EUCLIDEAN",
            Self::Manhattan => "MANHATTAN",
        }
    }
}

/// Parsed endpoint configuration derived from a URI like `surrealdb:<op>?datasource=<name>&...`.
#[derive(Clone, Debug)]
pub struct SurrealDbEndpointConfig {
    pub operation: SurrealDbOperation,
    pub datasource: String,
    pub table: Option<String>,
    pub id: Option<String>,
    pub from: Option<String>,
    pub edge: Option<String>,
    pub to: Option<String>,
    pub to_table: Option<String>,
    pub top_k: Option<usize>,
    pub metric: Option<VectorMetric>,
    pub vector_field: Option<String>,
    pub limit: Option<usize>,
    pub query: Option<String>,
    pub function: Option<String>,
    pub output: OutputType,
}

impl Default for SurrealDbEndpointConfig {
    fn default() -> Self {
        Self {
            operation: SurrealDbOperation::Query,
            datasource: String::new(),
            table: None,
            id: None,
            from: None,
            edge: None,
            to: None,
            to_table: None,
            top_k: None,
            metric: None,
            vector_field: Some(DEFAULT_VECTOR_FIELD.to_string()),
            limit: None,
            query: None,
            function: None,
            output: OutputType::Json,
        }
    }
}

impl SurrealDbEndpointConfig {
    /// Parse an endpoint configuration from a URI string.
    ///
    /// # Format
    /// `surrealdb:{operation}?datasource={name}&table={table}&...`
    ///
    /// # Errors
    /// Returns [`CamelError::InvalidUri`] if the operation is unrecognized or
    /// `datasource` parameter is missing.
    pub fn from_uri(uri: &str) -> Result<Self, CamelError> {
        // Split path and query (LLM-component pattern)
        let (operation_str, query) = match uri.split_once('?') {
            Some((path, q)) => (path, q),
            None => (uri, ""),
        };

        // Verify scheme prefix was actually present
        if !uri.starts_with("surrealdb:") {
            return Err(CamelError::InvalidUri(format!(
                "expected surrealdb scheme, got: {uri}"
            )));
        }

        let operation_str = operation_str.trim_start_matches("surrealdb:");

        let operation = match operation_str.trim() {
            "query" => SurrealDbOperation::Query,
            "select" => SurrealDbOperation::Select,
            "create" => SurrealDbOperation::Create,
            "update" => SurrealDbOperation::Update,
            "upsert" => SurrealDbOperation::Upsert,
            "delete" => SurrealDbOperation::Delete,
            "patch" => SurrealDbOperation::Patch,
            "relate" => SurrealDbOperation::Relate,
            "vector" => SurrealDbOperation::Vector,
            "search" => SurrealDbOperation::Search,
            "run" => SurrealDbOperation::Run,
            "live" => SurrealDbOperation::Live,
            other => {
                return Err(CamelError::InvalidUri(format!(
                    "unknown surrealdb operation: '{other}'"
                )));
            }
        };

        // Parse query params (form_urlencoded like LLM component)
        let params: HashMap<String, String> = form_urlencoded::parse(query.as_bytes())
            .into_owned()
            .collect();

        // datasource is REQUIRED
        let datasource = params.get("datasource").cloned().ok_or_else(|| {
            CamelError::InvalidUri("surrealdb URI requires 'datasource' parameter".to_string())
        })?;

        let output = match params.get("output").map(|s| s.as_str()) {
            Some("stream") => OutputType::StreamList,
            _ => OutputType::Json,
        };

        let metric = params.get("metric").and_then(|s| match s.as_str() {
            "cosine" => Some(VectorMetric::Cosine),
            "euclidean" => Some(VectorMetric::Euclidean),
            "manhattan" => Some(VectorMetric::Manhattan),
            _ => None,
        });

        let top_k = match params.get("top_k") {
            Some(v) => Some(v.parse::<usize>().map_err(|_| {
                CamelError::InvalidUri(format!("top_k must be a positive integer, got '{v}'"))
            })?),
            None => None,
        };
        let limit = match params.get("limit") {
            Some(v) => Some(v.parse::<usize>().map_err(|_| {
                CamelError::InvalidUri(format!("limit must be a positive integer, got '{v}'"))
            })?),
            None => None,
        };

        let vector_field = Some(
            params
                .get("vector_field")
                .cloned()
                .unwrap_or_else(|| DEFAULT_VECTOR_FIELD.to_string()),
        );

        let query = params.get("query").cloned();
        let function = params.get("function").cloned();

        Ok(Self {
            operation,
            datasource,
            table: params.get("table").cloned(),
            id: params.get("id").cloned(),
            from: params.get("from").cloned(),
            edge: params.get("edge").cloned(),
            to: params.get("to").cloned(),
            to_table: params.get("to_table").cloned(),
            top_k,
            metric,
            vector_field,
            limit,
            query,
            function,
            output,
        })
    }

    /// Validate that required parameters for the operation are present.
    /// Called during `create_endpoint` (fail-fast).
    ///
    /// Applies identifier validation to `table`, `edge`, and `vector_field` parameters
    /// (they must be valid ASCII identifiers — no injection/unicode).
    /// Record key fields (`id`, `from`, `to`) are NOT validated as identifiers
    /// because they go through `type::record()` param binding.
    pub fn validate(&self) -> Result<(), SurrealDbError> {
        use SurrealDbOperation::*;
        match self.operation {
            Query => {}
            Select | Create => {
                Self::require_field(self.table.as_deref(), "table")?;
                crate::query::validate_identifier(self.table.as_deref().unwrap_or(""))?;
            }
            Update | Delete | Upsert | Patch => {
                Self::require_field(self.table.as_deref(), "table")?;
                crate::query::validate_identifier(self.table.as_deref().unwrap_or(""))?;
                Self::require_field(self.id.as_deref(), "id")?;
            }
            Relate => {
                Self::require_field(self.table.as_deref(), "table")?;
                crate::query::validate_identifier(self.table.as_deref().unwrap_or(""))?;
                Self::require_field(self.edge.as_deref(), "edge")?;
                crate::query::validate_identifier(self.edge.as_deref().unwrap_or(""))?;
                Self::require_field(self.from.as_deref(), "from")?;
                Self::require_field(self.to.as_deref(), "to")?;
            }
            Vector => {
                self.validate_table_and_vector_field()?;
            }
            Search => {
                self.validate_table_and_vector_field()?;
                if self.top_k.is_none() {
                    return Err(SurrealDbError::MissingParam("top_k".into()));
                }
            }
            Run => {
                Self::require_field(self.function.as_deref(), "function")?;
            }
            Live => {
                Self::require_field(self.table.as_deref(), "table")?;
                crate::query::validate_identifier(self.table.as_deref().unwrap_or(""))?;
            }
        }
        Ok(())
    }

    /// Validate that `table` and `vector_field` are present and valid identifiers.
    fn validate_table_and_vector_field(&self) -> Result<(), SurrealDbError> {
        Self::require_field(self.table.as_deref(), "table")?;
        crate::query::validate_identifier(self.table.as_deref().unwrap_or(""))?;
        crate::query::validate_identifier(self.vector_field.as_deref().unwrap_or(""))?;
        Ok(())
    }

    fn require_field(opt: Option<&str>, name: &str) -> Result<(), SurrealDbError> {
        if opt.map(str::is_empty).unwrap_or(true) {
            return Err(SurrealDbError::MissingParam(name.into()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Operation parsing ---

    #[test]
    fn test_parse_query() {
        let cfg = SurrealDbEndpointConfig::from_uri("surrealdb:query?datasource=mydb").unwrap();
        assert_eq!(cfg.operation, SurrealDbOperation::Query);
        assert_eq!(cfg.datasource, "mydb");
    }

    #[test]
    fn test_parse_select() {
        let cfg = SurrealDbEndpointConfig::from_uri("surrealdb:select?datasource=mydb&table=user")
            .unwrap();
        assert_eq!(cfg.operation, SurrealDbOperation::Select);
        assert_eq!(cfg.datasource, "mydb");
        assert_eq!(cfg.table.as_deref(), Some("user"));
    }

    #[test]
    fn test_parse_create() {
        let cfg = SurrealDbEndpointConfig::from_uri("surrealdb:create?datasource=mydb&table=user")
            .unwrap();
        assert_eq!(cfg.operation, SurrealDbOperation::Create);
        assert_eq!(cfg.table.as_deref(), Some("user"));
    }

    #[test]
    fn test_parse_update() {
        let cfg =
            SurrealDbEndpointConfig::from_uri("surrealdb:update?datasource=mydb&table=user&id=123")
                .unwrap();
        assert_eq!(cfg.operation, SurrealDbOperation::Update);
        assert_eq!(cfg.table.as_deref(), Some("user"));
        assert_eq!(cfg.id.as_deref(), Some("123"));
    }

    #[test]
    fn test_parse_delete() {
        let cfg =
            SurrealDbEndpointConfig::from_uri("surrealdb:delete?datasource=mydb&table=user&id=123")
                .unwrap();
        assert_eq!(cfg.operation, SurrealDbOperation::Delete);
        assert_eq!(cfg.table.as_deref(), Some("user"));
        assert_eq!(cfg.id.as_deref(), Some("123"));
    }

    #[test]
    fn test_parse_relate() {
        let cfg = SurrealDbEndpointConfig::from_uri(
            "surrealdb:relate?datasource=mydb&table=user&edge=friend&from=user:1&to=user:2",
        )
        .unwrap();
        assert_eq!(cfg.operation, SurrealDbOperation::Relate);
        assert_eq!(cfg.edge.as_deref(), Some("friend"));
        assert_eq!(cfg.from.as_deref(), Some("user:1"));
        assert_eq!(cfg.to.as_deref(), Some("user:2"));
    }

    #[test]
    fn test_parse_vector() {
        let cfg =
            SurrealDbEndpointConfig::from_uri("surrealdb:vector?datasource=mydb&table=article")
                .unwrap();
        assert_eq!(cfg.operation, SurrealDbOperation::Vector);
        assert_eq!(cfg.table.as_deref(), Some("article"));
    }

    #[test]
    fn test_parse_search() {
        let cfg = SurrealDbEndpointConfig::from_uri(
            "surrealdb:search?datasource=mydb&table=article&top_k=5",
        )
        .unwrap();
        assert_eq!(cfg.operation, SurrealDbOperation::Search);
        assert_eq!(cfg.table.as_deref(), Some("article"));
        assert_eq!(cfg.top_k, Some(5));
    }

    #[test]
    fn test_parse_live() {
        let cfg =
            SurrealDbEndpointConfig::from_uri("surrealdb:live?datasource=mydb&table=user").unwrap();
        assert_eq!(cfg.operation, SurrealDbOperation::Live);
        assert_eq!(cfg.table.as_deref(), Some("user"));
    }

    #[test]
    fn test_parse_upsert() {
        let cfg =
            SurrealDbEndpointConfig::from_uri("surrealdb:upsert?datasource=mydb&table=user&id=42")
                .unwrap();
        assert_eq!(cfg.operation, SurrealDbOperation::Upsert);
        assert_eq!(cfg.table.as_deref(), Some("user"));
        assert_eq!(cfg.id.as_deref(), Some("42"));
    }

    #[test]
    fn test_parse_patch() {
        let cfg =
            SurrealDbEndpointConfig::from_uri("surrealdb:patch?datasource=mydb&table=user&id=42")
                .unwrap();
        assert_eq!(cfg.operation, SurrealDbOperation::Patch);
        assert_eq!(cfg.table.as_deref(), Some("user"));
        assert_eq!(cfg.id.as_deref(), Some("42"));
    }

    #[test]
    fn test_parse_run() {
        let cfg = SurrealDbEndpointConfig::from_uri(
            "surrealdb:run?datasource=mydb&function=fn%3A%3Agreet",
        )
        .unwrap();
        assert_eq!(cfg.operation, SurrealDbOperation::Run);
        assert_eq!(cfg.function.as_deref(), Some("fn::greet"));
    }

    // --- Error cases ---

    #[test]
    fn test_missing_datasource() {
        let result = SurrealDbEndpointConfig::from_uri("surrealdb:query");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, CamelError::InvalidUri(_)));
        assert!(err.to_string().contains("datasource"));
    }

    #[test]
    fn test_unknown_operation() {
        let result = SurrealDbEndpointConfig::from_uri("surrealdb:unknown?datasource=mydb");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, CamelError::InvalidUri(_)));
        assert!(err.to_string().contains("unknown"));
    }

    #[test]
    fn test_wrong_scheme() {
        let result = SurrealDbEndpointConfig::from_uri("foo:bar");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, CamelError::InvalidUri(_)));
        assert!(err.to_string().contains("surrealdb"));
    }

    // --- Defaults ---

    #[test]
    fn test_default_vector_field() {
        let cfg =
            SurrealDbEndpointConfig::from_uri("surrealdb:vector?datasource=mydb&table=t").unwrap();
        assert_eq!(cfg.vector_field.as_deref(), Some("embedding"));
    }

    #[test]
    fn test_default_output_json() {
        let cfg = SurrealDbEndpointConfig::from_uri("surrealdb:query?datasource=mydb").unwrap();
        assert_eq!(cfg.output, OutputType::Json);
    }

    #[test]
    fn test_output_stream_parsed() {
        let cfg =
            SurrealDbEndpointConfig::from_uri("surrealdb:query?datasource=mydb&output=stream")
                .unwrap();
        assert_eq!(cfg.output, OutputType::StreamList);
    }

    #[test]
    fn test_metric_euclidean_parsed() {
        let cfg = SurrealDbEndpointConfig::from_uri(
            "surrealdb:vector?datasource=mydb&table=t&metric=euclidean",
        )
        .unwrap();
        assert_eq!(cfg.metric, Some(VectorMetric::Euclidean));
    }

    // --- validate() ---

    #[test]
    fn validate_select_missing_table_fails() {
        let cfg = SurrealDbEndpointConfig::from_uri("surrealdb:select?datasource=main")
            .expect("uri without table parses");
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("table"));
    }

    #[test]
    fn validate_update_missing_id_fails() {
        let cfg = SurrealDbEndpointConfig::from_uri("surrealdb:update?datasource=main&table=users")
            .expect("uri without id parses");
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("id"));
    }

    #[test]
    fn validate_search_missing_top_k_fails() {
        let cfg = SurrealDbEndpointConfig::from_uri("surrealdb:search?datasource=main&table=emb")
            .expect("uri without top_k parses");
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("top_k"));
    }

    #[test]
    fn validate_relate_with_all_params_succeeds() {
        let cfg = SurrealDbEndpointConfig::from_uri(
            "surrealdb:relate?datasource=main&from=1&edge=knows&to=2&table=user",
        )
        .expect("uri parses");
        cfg.validate().expect("all relate params present");
    }

    #[test]
    fn validate_query_no_params_succeeds() {
        let cfg = SurrealDbEndpointConfig::from_uri("surrealdb:query?datasource=main")
            .expect("uri parses");
        cfg.validate().expect("query needs no extra params");
    }

    #[test]
    fn validate_upsert_missing_id_fails() {
        let cfg = SurrealDbEndpointConfig::from_uri("surrealdb:upsert?datasource=main&table=users")
            .expect("uri without id parses");
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("id"));
    }

    #[test]
    fn validate_patch_missing_id_fails() {
        let cfg = SurrealDbEndpointConfig::from_uri("surrealdb:patch?datasource=main&table=users")
            .expect("uri without id parses");
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("id"));
    }

    #[test]
    fn validate_run_missing_function_fails() {
        let cfg = SurrealDbEndpointConfig::from_uri("surrealdb:run?datasource=main")
            .expect("uri without function parses");
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("function"));
    }

    #[test]
    fn invalid_top_k_value_is_error() {
        let result = SurrealDbEndpointConfig::from_uri(
            "surrealdb:search?datasource=main&table=emb&top_k=abc",
        );
        assert!(result.is_err());
        let err_msg = match result {
            Err(e) => format!("{e:?}"),
            Ok(_) => panic!("top_k=abc must be rejected"),
        };
        assert!(err_msg.contains("top_k"), "got: {err_msg}");
    }

    #[test]
    fn validate_select_invalid_table_fails() {
        let cfg = SurrealDbEndpointConfig::from_uri(
            "surrealdb:select?datasource=main&table=users%3BDROP",
        )
        .expect("uri parses (semicolon url-encoded)");
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("identifier") || err.to_string().contains("invalid"));
    }

    #[test]
    fn validate_relate_invalid_edge_fails() {
        let cfg = SurrealDbEndpointConfig::from_uri(
            "surrealdb:relate?datasource=main&table=user&edge=knows%20admin&from=u:1&to=u:2",
        )
        .expect("uri parses (space url-encoded)");
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("identifier") || err.to_string().contains("invalid"));
    }

    #[test]
    fn validate_vector_invalid_vector_field_fails() {
        let cfg = SurrealDbEndpointConfig::from_uri(
            "surrealdb:vector?datasource=main&table=emb&vector_field=emb%27%3B",
        )
        .expect("uri parses");
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("identifier") || err.to_string().contains("invalid"));
    }

    #[test]
    fn validate_search_invalid_vector_field_fails() {
        let cfg = SurrealDbEndpointConfig::from_uri(
            "surrealdb:search?datasource=main&table=emb&top_k=5&vector_field=bad%20field",
        )
        .expect("uri parses");
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("identifier") || err.to_string().contains("invalid"));
    }
}
