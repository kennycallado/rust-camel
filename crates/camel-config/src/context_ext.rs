use crate::config::CamelConfig;
use crate::discovery::discover_routes;
use camel_api::CamelError;
use camel_core::route::RouteDefinition;

impl CamelConfig {
    /// Load routes from config file and return them (without adding to context yet)
    /// This allows components to be registered before routes are resolved
    pub fn load_routes(path: &str) -> Result<Vec<RouteDefinition>, CamelError> {
        let config = Self::from_file_with_profile_and_env(path, None)
            .map_err(|e| CamelError::Config(e.to_string()))?;

        if config.routes.is_empty() {
            return Ok(Vec::new());
        }

        discover_routes(&config.routes).map_err(|e| CamelError::Config(e.to_string()))
    }
}
