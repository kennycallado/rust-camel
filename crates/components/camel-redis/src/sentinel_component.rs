//! Component factories for the sentinel URI schemes.
//!
//! The runtime registry resolves a route URI to a component by scheme, so
//! `redis-sentinel://` and `rediss-sentinel://` endpoints need components
//! registered under those scheme names. These factories share
//! [`RedisComponent`](crate::RedisComponent)'s endpoint-creation logic: the
//! config parser derives the Sentinel topology from the URI scheme, and the
//! fail-closed feature check (ADR-0033) rejects sentinel URIs when the
//! `sentinel` cargo feature is disabled.

use camel_component_api::{CamelError, Component, ComponentMetadata, Endpoint};

use crate::RedisConfig;
use crate::create_redis_endpoint;
use crate::metadata::RedisMetadataDescriptor;

/// Sentinel-scheme metadata derived from the redis descriptor.
///
/// Keeps the redis URI options (command, key, channels, ...) — they apply to
/// sentinel endpoints identically — but carries the sentinel scheme so
/// registry harvesting does not log a scheme-mismatch normalization.
fn sentinel_metadata(scheme: &str) -> ComponentMetadata {
    let mut meta = RedisMetadataDescriptor::metadata();
    meta.scheme = scheme.to_string();
    meta.description = "Redis Sentinel failover (commands / pub-sub)".to_string();
    meta.uri_syntax =
        format!("{scheme}://<node:26379>[,<node:26379>]/<master-name>/<db>?command=<cmd>");
    meta
}

macro_rules! sentinel_component {
    ($(#[$meta:meta])* $name:ident, $scheme:expr) => {
        $(#[$meta])*
        pub struct $name {
            config: Option<RedisConfig>,
        }

        impl $name {
            /// Create a new component without global config defaults.
            ///
            /// Endpoint configs fall back to hardcoded defaults via
            /// `resolve_defaults()`.
            pub fn new() -> Self {
                Self { config: None }
            }

            /// Create a component with global config defaults from the
            /// `[components.redis]` TOML block (including
            /// `[components.redis.sentinel]` topology defaults).
            pub fn with_config(config: RedisConfig) -> Self {
                Self {
                    config: Some(config),
                }
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl Component for $name {
            fn scheme(&self) -> &str {
                $scheme
            }

            fn metadata(&self) -> ComponentMetadata {
                sentinel_metadata($scheme)
            }

            fn create_endpoint(
                &self,
                uri: &str,
                ctx: &dyn camel_component_api::ComponentContext,
            ) -> Result<Box<dyn Endpoint>, CamelError> {
                create_redis_endpoint(uri, self.config.as_ref(), ctx)
            }
        }
    };
}

sentinel_component!(
    /// Component factory for the `redis-sentinel://` scheme.
    ///
    /// Creates endpoints for URIs like
    /// `redis-sentinel://sentinel-a:26379/mymaster/0?command=SET`. Register
    /// it in addition to [`RedisComponent`](crate::RedisComponent) to use
    /// sentinel URIs from routes.
    RedisSentinelComponent,
    "redis-sentinel"
);

sentinel_component!(
    /// Component factory for the `rediss-sentinel://` scheme (TLS to both
    /// sentinels and Redis nodes).
    RedissSentinelComponent,
    "rediss-sentinel"
);

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::NoOpComponentContext;

    #[test]
    fn sentinel_component_schemes() {
        assert_eq!(RedisSentinelComponent::new().scheme(), "redis-sentinel");
        assert_eq!(RedissSentinelComponent::new().scheme(), "rediss-sentinel");
    }

    #[test]
    fn sentinel_metadata_carries_scheme() {
        let meta = RedisSentinelComponent::new().metadata();
        assert_eq!(meta.scheme, "redis-sentinel");
        assert!(meta.uri_syntax.contains("redis-sentinel://"));
    }

    #[cfg(feature = "sentinel")]
    #[test]
    fn sentinel_component_creates_sentinel_endpoint() {
        let ctx = NoOpComponentContext;
        let endpoint = RedisSentinelComponent::new()
            .create_endpoint(
                "redis-sentinel://127.0.0.1:26379/mymaster/0?command=GET",
                &ctx,
            )
            .expect("sentinel endpoint creation should succeed");
        assert_eq!(
            endpoint.uri(),
            "redis-sentinel://127.0.0.1:26379/mymaster/0?command=GET"
        );
    }

    #[cfg(not(feature = "sentinel"))]
    #[test]
    fn sentinel_component_fails_closed_without_feature() {
        let ctx = NoOpComponentContext;
        let err = match RedisSentinelComponent::new().create_endpoint(
            "redis-sentinel://127.0.0.1:26379/mymaster/0?command=GET",
            &ctx,
        ) {
            Ok(_) => panic!("sentinel URI must fail closed without the sentinel feature"),
            Err(e) => e,
        };
        assert!(
            err.to_string().contains("'sentinel'"),
            "unexpected error: {err:?}"
        );
    }
}
