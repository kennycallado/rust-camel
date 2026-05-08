use std::collections::HashMap;
use std::sync::Arc;

use camel_ai::{QdrantConfig, QdrantStore};
use camel_component_api::{CamelError, Component, ComponentContext, Endpoint};

use crate::endpoint::VectorEndpoint;

pub fn parse_query(query: &str) -> HashMap<String, String> {
    query
        .split('&')
        .filter_map(|pair| {
            let mut parts = pair.splitn(2, '=');
            let k = parts.next()?.to_string();
            let v = parts.next().unwrap_or("").to_string();
            if k.is_empty() {
                None
            } else {
                Some((k, v))
            }
        })
        .collect()
}

#[derive(Default)]
pub struct VectorComponent;

impl Component for VectorComponent {
    fn scheme(&self) -> &str {
        "vector"
    }

    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        // uri: "vector:upsert?url=...&collection=..." or "vector:search?..."
        let without_scheme = uri.strip_prefix("vector:").unwrap_or(uri);
        let (operation, query) = without_scheme
            .split_once('?')
            .unwrap_or((without_scheme, ""));
        let params = parse_query(query);

        let url = params
            .get("url")
            .cloned()
            .unwrap_or_else(|| "http://localhost:6333".into());
        let collection = params
            .get("collection")
            .cloned()
            .unwrap_or_else(|| "default".into());
        let top_k = params
            .get("top_k")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(5);

        let store = Arc::new(QdrantStore::new(QdrantConfig { url, collection })?);

        Ok(Box::new(VectorEndpoint {
            uri: uri.to_string(),
            operation: operation.to_string(),
            store,
            top_k,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vector_component_scheme() {
        assert_eq!(VectorComponent::default().scheme(), "vector");
    }

    #[test]
    fn parse_query_extracts_top_k() {
        let p = parse_query("collection=docs&top_k=10");
        assert_eq!(p["collection"], "docs");
        assert_eq!(p["top_k"], "10");
    }

    #[test]
    fn parse_query_empty_value() {
        let p = parse_query("key");
        assert_eq!(p["key"], "");
    }

    #[test]
    fn parse_query_empty_key_skipped() {
        let p = parse_query("=value");
        assert!(p.is_empty());
    }
}
