//! End-to-end bundle-seam tests for the env-typed component probe
//! (openspec change `envtyped-components`, task 2.2).
//!
//! Observation goes through test-local adapter bundles that mirror the real
//! bundles' `from_toml` bodies byte-for-byte over the real config types.
//! Bundles keep their parsed configs private, so the adapter is the seam's
//! observation path: it keeps the production `config_key`, parses with the
//! same typed `try_into` oracle, and exposes the parsed config to the test.

#![cfg(all(feature = "redis", feature = "ws"))]

use camel_bundles::bundle_from_config;
use camel_component_api::{CamelError, ComponentBundle, ComponentRegistrar};
use camel_config::config::CamelConfig;

/// Build a `CamelConfig` through the public embedded seam: the TOML source
/// is parsed, placeholders are resolved through `lookup`, provenance is
/// attached, and the config is validated.
fn config_from(toml_src: &str, lookup: impl Fn(&str) -> Option<String>) -> CamelConfig {
    let value: toml::Value = toml::from_str(toml_src).expect("test fixture must be valid TOML");
    CamelConfig::from_toml_value_with_env(value, &lookup).expect("fixture config must load")
}

/// The resolved `[components.<key>]` block as the bundle seam sees it.
fn raw_block(config: &CamelConfig, key: &str) -> toml::Value {
    config
        .components
        .raw
        .get(key)
        .cloned()
        .expect("fixture declares the block")
}

/// Error from `bundle_from_config` without requiring `Debug` on the bundle.
fn seam_err<B: ComponentBundle>(config: &CamelConfig) -> CamelError {
    match bundle_from_config::<B>(config) {
        Ok(_) => panic!("bundle parse must fail for {}", B::config_key()),
        Err(e) => e,
    }
}

/// The seam error must equal the bundle's direct `from_toml` error: same
/// variant and same payload, byte-for-byte.
fn assert_same_error(seam: &CamelError, direct: &CamelError) {
    match (seam, direct) {
        (CamelError::Config(seam), CamelError::Config(direct)) => assert_eq!(seam, direct),
        _ => panic!("expected CamelError::Config on both sides: seam={seam:?}, direct={direct:?}"),
    }
}

#[derive(Debug)]
struct RedisProbeBundle {
    config: camel_component_redis::RedisConfig,
}

impl ComponentBundle for RedisProbeBundle {
    fn config_key() -> &'static str {
        "redis"
    }

    fn from_toml(value: toml::Value) -> Result<Self, CamelError> {
        let config: camel_component_redis::RedisConfig = value
            .try_into()
            .map_err(|e: toml::de::Error| CamelError::Config(e.to_string()))?;
        Ok(Self { config })
    }

    fn register_all(self, _ctx: &mut dyn ComponentRegistrar) {}
}

#[derive(Debug)]
struct WsProbeBundle {
    config: camel_component_ws::WsConfig,
}

impl ComponentBundle for WsProbeBundle {
    fn config_key() -> &'static str {
        "ws"
    }

    fn from_toml(value: toml::Value) -> Result<Self, CamelError> {
        let config: camel_component_ws::WsConfig = value
            .try_into()
            .map_err(|e: toml::de::Error| CamelError::Config(e.to_string()))?;
        Ok(Self { config })
    }

    fn register_all(self, _ctx: &mut dyn ComponentRegistrar) {}
}

#[derive(Debug)]
struct HttpProbeBundle {
    config: camel_component_http::HttpConfig,
}

impl ComponentBundle for HttpProbeBundle {
    fn config_key() -> &'static str {
        "http"
    }

    fn from_toml(value: toml::Value) -> Result<Self, CamelError> {
        let config: camel_component_http::HttpConfig = value
            .try_into()
            .map_err(|e: toml::de::Error| CamelError::Config(e.to_string()))?;
        Ok(Self { config })
    }

    fn register_all(self, _ctx: &mut dyn ComponentRegistrar) {}
}

#[test]
fn redis_port_via_env_value_coerces() {
    let cfg = config_from(
        "[components.redis]\nport = \"${env:EP_REDIS_PORT}\"\n",
        |name| (name == "EP_REDIS_PORT").then(|| "7100".to_string()),
    );
    let bundle = bundle_from_config::<RedisProbeBundle>(&cfg).expect("env-typed port must coerce");
    assert_eq!(bundle.config.port, 7100);
}

#[test]
fn redis_port_default_coerces() {
    let cfg = config_from(
        "[components.redis]\nport = \"${env:EP_REDIS_PORT:-6379}\"\n",
        |_| None,
    );
    let bundle =
        bundle_from_config::<RedisProbeBundle>(&cfg).expect("placeholder default must coerce");
    assert_eq!(bundle.config.port, 6379);
}

#[test]
fn ws_boolean_leaf_coerces() {
    let cfg = config_from(
        "[components.ws]\nbinary_payload = \"${env:EP_WS_BIN:-true}\"\n",
        |_| None,
    );
    let bundle = bundle_from_config::<WsProbeBundle>(&cfg).expect("env-typed boolean must coerce");
    assert_eq!(bundle.config.binary_payload, Some(true));
}

#[test]
fn numeric_string_leaf_stays_string() {
    let cfg = config_from(
        "[components.redis]\nhost = \"${env:EP_REDIS_HOST:-1234}\"\n",
        |_| None,
    );
    let bundle = bundle_from_config::<RedisProbeBundle>(&cfg)
        .expect("a String leaf parses on the first pass");
    assert_eq!(bundle.config.host, "1234");
}

#[test]
fn literal_quoted_numeric_stays_rejected() {
    let cfg = config_from("[components.ws]\nmax_connections = \"8080\"\n", |_| None);
    let seam = seam_err::<WsProbeBundle>(&cfg);
    let direct = <WsProbeBundle as ComponentBundle>::from_toml(raw_block(&cfg, "ws"))
        .expect_err("a literal quoted numeric must stay rejected");
    assert_same_error(&seam, &direct);
}

#[test]
fn unrelated_error_surfaces_verbatim() {
    let cfg = config_from(
        "[components.redis]\nport = \"${env:EP_REDIS_PORT2:-6379}\"\nconnection_timeout_secs = \"not-a-number\"\n",
        |_| None,
    );
    let seam = seam_err::<RedisProbeBundle>(&cfg);
    let direct = <RedisProbeBundle as ComponentBundle>::from_toml(raw_block(&cfg, "redis"))
        .expect_err("the unrelated defect must reject the direct parse");
    assert_same_error(&seam, &direct);
}

#[test]
fn two_blocks_each_coerce() {
    let cfg = config_from(
        "[components.redis]\nport = \"${env:EP_P1}\"\n[components.http]\nconnect_timeout_ms = \"${env:EP_P2:-5000}\"\n",
        |name| match name {
            "EP_P1" => Some("7100".to_string()),
            "EP_P2" => Some("8080".to_string()),
            _ => None,
        },
    );
    let redis =
        bundle_from_config::<RedisProbeBundle>(&cfg).expect("redis block must coerce in isolation");
    assert_eq!(redis.config.port, 7100);
    let http =
        bundle_from_config::<HttpProbeBundle>(&cfg).expect("http block must coerce in isolation");
    assert_eq!(http.config.connect_timeout_ms, 8080);
}

#[test]
fn nine_split_across_blocks_both_parse() {
    let cfg = config_from(
        "[components.redis]\nport = \"${env:EP_S1:-7100}\"\nconnection_timeout_secs = \"${env:EP_S2:-17}\"\ntls = \"${env:EP_S3:-true}\"\ntls_mode = \"${env:EP_S4:-false}\"\n[components.http]\nconnect_timeout_ms = \"${env:EP_S5:-201}\"\npool_idle_timeout_ms = \"${env:EP_S6:-202}\"\nresponse_timeout_ms = \"${env:EP_S7:-203}\"\nfollow_redirects = \"${env:EP_S8:-true}\"\nmax_body_size = \"${env:EP_S9:-204}\"\n",
        |_| None,
    );
    let redis =
        bundle_from_config::<RedisProbeBundle>(&cfg).expect("four candidates stay under the cap");
    assert_eq!(redis.config.port, 7100);
    assert_eq!(redis.config.connection_timeout_secs, 17);
    assert!(redis.config.tls);
    assert_eq!(redis.config.tls_mode, Some(false));

    let http =
        bundle_from_config::<HttpProbeBundle>(&cfg).expect("five candidates stay under the cap");
    assert_eq!(http.config.connect_timeout_ms, 201);
    assert_eq!(http.config.pool_idle_timeout_ms, 202);
    assert_eq!(http.config.response_timeout_ms, 203);
    assert!(http.config.follow_redirects);
    assert_eq!(http.config.max_body_size, 204);
}

#[test]
fn nine_candidates_one_block_keeps_first_error() {
    let cfg = config_from(
        "[components.http]\nconnect_timeout_ms = \"${env:EP_N1:-101}\"\npool_max_idle_per_host = \"${env:EP_N2:-102}\"\npool_idle_timeout_ms = \"${env:EP_N3:-103}\"\nresponse_timeout_ms = \"${env:EP_N4:-104}\"\nread_timeout_ms = \"${env:EP_N5:-105}\"\nmax_body_size = \"${env:EP_N6:-106}\"\nmax_response_bytes = \"${env:EP_N7:-107}\"\nmax_request_body = \"${env:EP_N8:-108}\"\nfollow_redirects = \"${env:EP_N9:-true}\"\n",
        |_| None,
    );
    let seam = seam_err::<HttpProbeBundle>(&cfg);
    let direct = <HttpProbeBundle as ComponentBundle>::from_toml(raw_block(&cfg, "http"))
        .expect_err("the uncoerced block rejects the direct parse");
    assert_same_error(&seam, &direct);
}

#[test]
fn eight_candidates_one_block_still_searches() {
    let cfg = config_from(
        "[components.http]\nconnect_timeout_ms = \"${env:EP_N1:-101}\"\npool_max_idle_per_host = \"${env:EP_N2:-102}\"\npool_idle_timeout_ms = \"${env:EP_N3:-103}\"\nresponse_timeout_ms = \"${env:EP_N4:-104}\"\nread_timeout_ms = \"${env:EP_N5:-105}\"\nmax_body_size = \"${env:EP_N6:-106}\"\nmax_response_bytes = \"${env:EP_N7:-107}\"\nmax_request_body = \"${env:EP_N8:-108}\"\n",
        |_| None,
    );
    let http = bundle_from_config::<HttpProbeBundle>(&cfg)
        .expect("eight candidates sit on the cap boundary");
    assert_eq!(http.config.connect_timeout_ms, 101);
    assert_eq!(http.config.pool_max_idle_per_host, 102);
    assert_eq!(http.config.pool_idle_timeout_ms, 103);
    assert_eq!(http.config.response_timeout_ms, 104);
    assert_eq!(http.config.read_timeout_ms, 105);
    assert_eq!(http.config.max_body_size, 106);
    assert_eq!(http.config.max_response_bytes, 107);
    assert_eq!(http.config.max_request_body, 108);
}
