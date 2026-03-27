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
