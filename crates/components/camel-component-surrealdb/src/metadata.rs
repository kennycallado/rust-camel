//! SurrealDB metadata descriptor — URI options catalog for the `surrealdb` scheme.
//!
//! This module is metadata-only. The generated `parse_uri_components` is never
//! called. `#[allow(dead_code)]` suppresses the associated-function warning.

use camel_component_api::UriConfig;

#[allow(dead_code)]
#[derive(UriConfig)]
#[uri_scheme = "surrealdb"]
#[uri_config(
    skip_impl,
    metadata(
        scheme = "surrealdb",
        description = "SurrealDB CRUD / query / vector",
        producer,
        consumer
    ),
    crate = "camel_component_api"
)]
pub(super) struct SurrealDbMetadataDescriptor {
    // NOTE: `output` is deliberately excluded — the parser reads it only to REJECT
    // `output=stream` (config.rs:188, CONTEXT.md "Rejected"). Advertising it as a
    // uri_option would mislead users into thinking it is accepted.
    #[uri_param(name = "datasource", required)]
    pub _datasource: String,

    #[uri_param(name = "table")]
    pub _table: Option<String>,

    #[uri_param(name = "id")]
    pub _id: Option<String>,

    #[uri_param(name = "from")]
    pub _from: Option<String>,

    #[uri_param(name = "edge")]
    pub _edge: Option<String>,

    #[uri_param(name = "to")]
    pub _to: Option<String>,

    #[uri_param(name = "to_table")]
    pub _to_table: Option<String>,

    #[uri_param(name = "top_k")]
    pub _top_k: Option<u64>,

    #[uri_param(name = "metric")]
    pub _metric: Option<String>,

    #[uri_param(name = "vector_field", default = "embedding")]
    pub _vector_field: String,

    #[uri_param(name = "limit")]
    pub _limit: Option<u64>,

    #[uri_param(name = "query")]
    pub _query: Option<String>,

    #[uri_param(name = "allow_dynamic_query", default = "false")]
    pub _allow_dynamic_query: bool,

    #[uri_param(name = "function")]
    pub _function: Option<String>,

    #[uri_param(name = "retryEnabled", default = "true")]
    pub _retry_enabled: bool,

    #[uri_param(name = "retryMaxAttempts", default = "10")]
    pub _retry_max_attempts: u32,

    #[uri_param(name = "retryInitialDelayMs", default = "100")]
    pub _retry_initial_delay_ms: u64,

    #[uri_param(name = "retryMultiplier", default = "2.0")]
    pub _retry_multiplier: f64,

    #[uri_param(name = "retryMaxDelayMs", default = "30000")]
    pub _retry_max_delay_ms: u64,

    #[uri_param(name = "retryJitter", default = "0.2")]
    pub _retry_jitter: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that every URI query key the `from_uri` parser reads has a
    /// corresponding `#[uri_param]` in the descriptor, and no extra keys.
    #[test]
    fn surrealdb_metadata_uri_options_parity() {
        let meta = SurrealDbMetadataDescriptor::metadata();
        let mut names: Vec<String> = meta.uri_options.iter().map(|o| o.name.clone()).collect();
        names.sort();

        let expected = vec![
            "allow_dynamic_query",
            "datasource",
            "edge",
            "from",
            "function",
            "id",
            "limit",
            "metric",
            "query",
            "retryEnabled",
            "retryInitialDelayMs",
            "retryJitter",
            "retryMaxAttempts",
            "retryMaxDelayMs",
            "retryMultiplier",
            "table",
            "to",
            "to_table",
            "top_k",
            "vector_field",
        ];

        assert_eq!(names, expected, "URI option names must match parser keys");

        // Verify datasource carries required
        let ds = meta
            .uri_options
            .iter()
            .find(|o| o.name == "datasource")
            .expect("datasource must exist");
        assert!(ds.required, "datasource must be required");

        // Verify retry defaults match NetworkRetryPolicy defaults
        let re = meta
            .uri_options
            .iter()
            .find(|o| o.name == "retryEnabled")
            .expect("retryEnabled must exist");
        assert_eq!(re.default_value.as_deref(), Some("true"));

        let rma = meta
            .uri_options
            .iter()
            .find(|o| o.name == "retryMaxAttempts")
            .expect("retryMaxAttempts must exist");
        assert_eq!(rma.default_value.as_deref(), Some("10"));

        let rid = meta
            .uri_options
            .iter()
            .find(|o| o.name == "retryInitialDelayMs")
            .expect("retryInitialDelayMs must exist");
        assert_eq!(rid.default_value.as_deref(), Some("100"));

        let rm = meta
            .uri_options
            .iter()
            .find(|o| o.name == "retryMultiplier")
            .expect("retryMultiplier must exist");
        assert_eq!(rm.default_value.as_deref(), Some("2.0"));

        let rmd = meta
            .uri_options
            .iter()
            .find(|o| o.name == "retryMaxDelayMs")
            .expect("retryMaxDelayMs must exist");
        assert_eq!(rmd.default_value.as_deref(), Some("30000"));

        let rj = meta
            .uri_options
            .iter()
            .find(|o| o.name == "retryJitter")
            .expect("retryJitter must exist");
        assert_eq!(rj.default_value.as_deref(), Some("0.2"));

        // Verify vector_field default
        let vf = meta
            .uri_options
            .iter()
            .find(|o| o.name == "vector_field")
            .expect("vector_field must exist");
        assert_eq!(vf.default_value.as_deref(), Some("embedding"));

        // Verify allow_dynamic_query default
        let adq = meta
            .uri_options
            .iter()
            .find(|o| o.name == "allow_dynamic_query")
            .expect("allow_dynamic_query must exist");
        assert_eq!(adq.default_value.as_deref(), Some("false"));
    }
}
