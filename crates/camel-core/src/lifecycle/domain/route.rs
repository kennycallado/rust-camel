/// Minimal domain value object representing a route's identity.
/// No framework dependencies — pure domain type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteSpec {
    pub route_id: String,
    pub from_uri: String,
}

impl RouteSpec {
    pub fn new(route_id: impl Into<String>, from_uri: impl Into<String>) -> Self {
        Self {
            route_id: route_id.into(),
            from_uri: from_uri.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_spec_new_sets_fields() {
        let spec = RouteSpec::new("r1", "direct:start");
        assert_eq!(spec.route_id, "r1");
        assert_eq!(spec.from_uri, "direct:start");
    }
}
