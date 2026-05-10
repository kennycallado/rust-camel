use camel_api::{CamelError, CanonicalRouteSpec};
use camel_core::route::RouteDefinition;

use crate::compile::compile_canonical_route;

pub fn parse_canonical_json(json: &str) -> Result<Vec<RouteDefinition>, CamelError> {
    #[derive(serde::Deserialize)]
    struct CanonicalRoutes {
        routes: Vec<CanonicalRouteSpec>,
    }

    let specs: CanonicalRoutes = serde_json::from_str(json)
        .map_err(|e| CamelError::RouteError(format!("canonical JSON parse error: {e}")))?;

    specs
        .routes
        .into_iter()
        .map(|spec| {
            spec.validate_contract()?;
            compile_canonical_route(
                spec,
                camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
            )
        })
        .collect()
}

pub fn parse_canonical_route(spec: CanonicalRouteSpec) -> Result<RouteDefinition, CamelError> {
    spec.validate_contract()?;
    compile_canonical_route(
        spec,
        camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_canonical_json_minimal_route() {
        let json = r#"{
            "routes": [{
                "route_id": "test-route",
                "from": "timer:tick?period=1000",
                "steps": [
                    { "step": "log", "config": { "message": "Hello" } },
                    { "step": "to", "config": { "uri": "log:info" } }
                ],
                "version": 1
            }]
        }"#;
        let routes = parse_canonical_json(json).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].route_id(), "test-route");
    }

    #[test]
    fn parse_canonical_json_rejects_wrong_version() {
        let json = r#"{
            "routes": [{
                "route_id": "bad",
                "from": "timer:tick",
                "steps": [],
                "version": 99
            }]
        }"#;
        let err = parse_canonical_json(json).err().unwrap();
        assert!(err.to_string().contains("version"));
    }

    #[test]
    fn parse_canonical_json_rejects_empty_route_id() {
        let json = r#"{
            "routes": [{
                "route_id": "  ",
                "from": "timer:tick",
                "steps": [],
                "version": 1
            }]
        }"#;
        let err = parse_canonical_json(json).err().unwrap();
        assert!(err.to_string().contains("route_id"));
    }
}
