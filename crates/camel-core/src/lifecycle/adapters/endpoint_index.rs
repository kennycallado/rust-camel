use std::collections::HashMap;

/// Index mapping source endpoint URIs to the set of route_ids consuming from them.
/// One-to-many: multiple routes may share the same source URI (e.g. `direct:`, `seda:`).
pub(crate) struct EndpointIndex {
    uri_to_routes: HashMap<String, Vec<String>>,
}

impl EndpointIndex {
    pub fn new() -> Self {
        Self {
            uri_to_routes: HashMap::new(),
        }
    }

    /// Insert a URI → route_id mapping. Idempotent: if route_id already
    /// exists for this URI, no-op.
    pub fn insert(&mut self, uri: &str, route_id: &str) {
        let routes = self.uri_to_routes.entry(uri.to_string()).or_default();
        if !routes.iter().any(|r| r == route_id) {
            routes.push(route_id.to_string());
        }
    }

    /// Remove a route_id from all URI entries. Drops URI keys that become empty.
    pub fn remove(&mut self, route_id: &str) {
        self.uri_to_routes.retain(|_, routes| {
            routes.retain(|r| r != route_id);
            !routes.is_empty()
        });
    }

    /// Return all route_ids for the given URI. Empty vec if not found.
    pub fn routes_for(&self, uri: &str) -> Vec<String> {
        self.uri_to_routes.get(uri).cloned().unwrap_or_default()
    }

    /// Return all registered URI keys.
    pub fn list_uris(&self) -> Vec<String> {
        self.uri_to_routes.keys().cloned().collect()
    }
}

impl Default for EndpointIndex {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_index_insert_and_lookup() {
        let mut idx = EndpointIndex::new();
        idx.insert("timer:tick", "r1");
        assert_eq!(idx.routes_for("timer:tick"), vec!["r1"]);
    }

    #[test]
    fn endpoint_index_multiple_routes_same_uri() {
        let mut idx = EndpointIndex::new();
        idx.insert("direct:x", "a");
        idx.insert("direct:x", "b");
        assert_eq!(idx.routes_for("direct:x"), vec!["a", "b"]);
    }

    #[test]
    fn endpoint_index_insert_idempotent() {
        let mut idx = EndpointIndex::new();
        idx.insert("timer:tick", "r1");
        idx.insert("timer:tick", "r1");
        assert_eq!(idx.routes_for("timer:tick"), vec!["r1"]);
    }

    #[test]
    fn endpoint_index_remove_route() {
        let mut idx = EndpointIndex::new();
        idx.insert("timer:tick", "r1");
        idx.remove("r1");
        assert!(idx.routes_for("timer:tick").is_empty());
    }

    #[test]
    fn endpoint_index_remove_drops_empty_uri() {
        let mut idx = EndpointIndex::new();
        idx.insert("timer:tick", "r1");
        idx.remove("r1");
        assert!(!idx.list_uris().contains(&"timer:tick".to_string()));
    }

    #[test]
    fn endpoint_index_remove_preserves_other_routes() {
        let mut idx = EndpointIndex::new();
        idx.insert("direct:x", "a");
        idx.insert("direct:x", "b");
        idx.remove("a");
        assert_eq!(idx.routes_for("direct:x"), vec!["b"]);
    }
}
