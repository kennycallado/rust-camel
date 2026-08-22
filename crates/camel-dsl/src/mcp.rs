//! MCP DSL block types (`mcp:` in YAML/JSON).
//!
//! The MCP block is the transport- and version-agnostic declaration of a named
//! MCP server (Consumer role) and the tools and resources it exposes. It is
//! structurally analogous to the `rest:` block — a plain AST that a later
//! lowering pass turns into `mcp:<server>/tool/<name>` and
//! `mcp:<server>/resource/<name>` consumer routes. The block owns its
//! listener configuration (spec: MCP listener ownership): `bind` always,
//! and `tls`/`max_tools`/`max_resources` when declared, flow to the runtime
//! as `mcp.declared.*` endpoint parameters on every lowered route, and ARE
//! the runtime values for the shared listener. Caps and TLS are
//! presence-based: the lowering emits a `mcp.declared.*` parameter ONLY
//! when the block declares the value, so "declared" stays distinguishable
//! from "defaulted" at the consumer merge (a defaulted cap must never
//! conflict with — or overwrite — a TOML-declared one). The DSL carries no
//! session or protocol-version keys to lower, so any such key in the block
//! is a parse error via `deny_unknown_fields`.

use camel_api::CamelError;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde::Deserialize;
use std::collections::BTreeMap;

use crate::route_ast::{RouteDslRoute, RouteDslSecurityPolicy};

fn non_empty_path<'de, D>(deserializer: D, field: &'static str) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let path = String::deserialize(deserializer)?;
    let path = path.trim();
    if path.is_empty() {
        return Err(serde::de::Error::custom(format!(
            "{field} must not be empty"
        )));
    }
    Ok(path.to_owned())
}

fn deserialize_cert_path<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    non_empty_path(deserializer, "cert_path")
}

fn deserialize_key_path<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    non_empty_path(deserializer, "key_path")
}

/// TLS certificate and private-key paths declared by an MCP DSL server.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct RouteDslMcpTlsConfig {
    /// PEM-encoded server certificate chain.
    #[serde(deserialize_with = "deserialize_cert_path")]
    pub cert_path: String,
    /// PEM-encoded server private key.
    #[serde(deserialize_with = "deserialize_key_path")]
    pub key_path: String,
}

/// A top-level MCP block (`mcp:` in YAML/JSON).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct RouteDslMcp {
    /// Named server (Consumer) declaration.
    pub server: RouteDslMcpServer,
    /// Tools this server exposes.
    #[serde(default)]
    pub tools: Vec<RouteDslMcpTool>,
    /// Resources this server exposes.
    #[serde(default)]
    pub resources: Vec<RouteDslMcpResource>,
}

/// Server-role (Consumer) declaration for one named MCP server inside a
/// `mcp:` block.
///
/// The block owns its listener configuration the way `rest:` does (spec: MCP
/// listener ownership): `bind` always, and `tls`/`max_tools`/`max_resources`
/// when declared, are lowered onto every route of the block as
/// `mcp.declared.*` endpoint parameters and ARE the runtime values for
/// listener construction/lookup. TOML `mcp.servers.<name>` must still carry
/// the entry (the `name` MUST match a `mcp.servers.<name>` key or consumer
/// start fails) and remains the source for keys with no DSL counterpart
/// (`allowed_hosts`, `security_policy`); a key declared by BOTH sides with
/// different values fails consumer start with an error naming both sources —
/// no DSL field is silently ignored, and silence is never a disagreement
/// (a cap declared by one side only is that side's value).
/// The `security_policy` field is a real `RouteDslSecurityPolicy` (parse-time
/// validated, mirroring the route field) and it DOES flow to the lowered
/// consumer routes — enforcement is route-level (camel-api `SecurityPolicy`),
/// the same mechanism camel-http routes use.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct RouteDslMcpServer {
    /// Name of the server (referenced by lowered `mcp:<name>/...` routes).
    pub name: String,
    /// Streamable-HTTP listen address (IP:port literal).
    pub bind: String,
    /// Optional TLS configuration.
    #[serde(default)]
    pub tls: Option<RouteDslMcpTlsConfig>,
    /// Route-level authorization policy, propagated to every lowered route
    /// (tools AND resources). Evaluated per request against the carried
    /// HTTP headers; absent means no route-level policy (the TOML
    /// `security_policy` presence gate still applies fail-closed).
    #[serde(default)]
    pub security_policy: Option<RouteDslSecurityPolicy>,
    /// Maximum number of tools this server may register (`None` when the
    /// block declares no cap — the runtime default then applies, and the
    /// lowering emits no `max_tools` parameter so a TOML-declared cap is
    /// kept).
    #[serde(default)]
    pub max_tools: Option<usize>,
    /// Maximum number of resources this server may register (`None` when
    /// the block declares no cap).
    #[serde(default)]
    pub max_resources: Option<usize>,
}

/// A single MCP tool declaration, carrying its input JSON Schema.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct RouteDslMcpTool {
    /// Tool name (referenced by the lowered `mcp:<server>/tool/<name>` route).
    pub name: String,
    /// Input JSON Schema for the tool's arguments.
    pub input_schema: serde_json::Value,
}

/// A single MCP resource declaration, carrying its MCP resource URI.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct RouteDslMcpResource {
    /// Resource name (referenced by the lowered `mcp:<server>/resource/<name>` route).
    pub name: String,
    /// The MCP resource URI (operator config, e.g. `crm://customers`).
    pub uri: String,
}

/// Validate an MCP server/tool/resource name against the closed charset
/// `[A-Za-z0-9._-]+` (bd rc-ap58).
///
/// Names travel verbatim into the lowered `mcp:<server>/tool/<name>` and
/// `mcp:<server>/resource/<name>` URI path segments — they are NOT
/// percent-encoded (only the schema and resource-URI query values are). A `?`
/// would silently truncate the URI and can shadow the `schema` param; a `/`
/// breaks the `<server>/<kind>/<name>` segment shape and fails consumer start
/// late. Rejecting at lowering names the offending key.
fn validate_mcp_name(kind: &str, name: &str) -> Result<(), CamelError> {
    let valid = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if valid {
        Ok(())
    } else {
        Err(CamelError::RouteError(format!(
            "mcp {kind} name '{name}' is invalid: names must match [A-Za-z0-9._-]+ — \
             a '?' truncates the lowered URI and can shadow the schema param; a '/' \
             breaks the <server>/<kind>/<name> segment shape"
        )))
    }
}

/// Build the `mcp.declared.*` endpoint-parameter suffix carrying the block's
/// listener declaration (`bind`/`tls`/caps) — the DSL lowering channel the
/// consumer start merges into the TOML server config (spec: MCP listener
/// ownership). `bind` is always emitted; caps and TLS are presence-based (a
/// parameter is emitted ONLY when the block declares the value, so
/// "declared" stays distinguishable from "defaulted" at the consumer merge).
/// Values are percent-encoded like every other lowered parameter.
fn declared_params(server: &RouteDslMcpServer) -> String {
    let mut out = format!(
        "&mcp.declared.bind={}",
        utf8_percent_encode(&server.bind, NON_ALPHANUMERIC),
    );
    if let Some(max_tools) = server.max_tools {
        out.push_str(&format!("&mcp.declared.max_tools={max_tools}"));
    }
    if let Some(max_resources) = server.max_resources {
        out.push_str(&format!("&mcp.declared.max_resources={max_resources}"));
    }
    if let Some(tls) = &server.tls {
        out.push_str(&format!(
            "&mcp.declared.tls.cert_path={}&mcp.declared.tls.key_path={}",
            utf8_percent_encode(&tls.cert_path, NON_ALPHANUMERIC),
            utf8_percent_encode(&tls.key_path, NON_ALPHANUMERIC),
        ));
    }
    out
}

/// Lower ALL MCP blocks in a document into consumer route entries.
///
/// Each tool becomes a `mcp:<server>/tool/<name>?schema=<URL-encoded schema>`
/// consumer route and each resource becomes a
/// `mcp:<server>/resource/<name>?uri=<URL-encoded resource URI>` consumer
/// route. The schema and resource URI travel on the URI's query params — the
/// same structural choice `rest.rs` makes putting `httpMethod` in the `from`
/// query — never as Exchange headers or body content (the schema is operator
/// config, not wire content). The block's listener declaration
/// (`bind`/`tls`/caps) rides the same channel as `mcp.declared.*`
/// parameters so consumer start can enforce DSL listener ownership.
pub fn lower_all_mcp_to_routes(blocks: &[RouteDslMcp]) -> Result<Vec<RouteDslRoute>, CamelError> {
    let mut routes = Vec::new();

    for block in blocks {
        validate_mcp_name("server", &block.server.name)?;
        let security_policy = block.server.security_policy.clone();
        let declared = declared_params(&block.server);
        for tool in &block.tools {
            validate_mcp_name("tool", &tool.name)?;
            let schema =
                utf8_percent_encode(&tool.input_schema.to_string(), NON_ALPHANUMERIC).to_string();
            let from = format!(
                "mcp:{}/tool/{}?schema={schema}{declared}",
                block.server.name, tool.name
            );
            routes.push(consumer_route(
                &format!("mcp-{}-tool-{}", block.server.name, tool.name),
                from,
                security_policy.clone(),
            ));
        }
        for resource in &block.resources {
            validate_mcp_name("resource", &resource.name)?;
            let uri = utf8_percent_encode(&resource.uri, NON_ALPHANUMERIC).to_string();
            let from = format!(
                "mcp:{}/resource/{}?uri={uri}{declared}",
                block.server.name, resource.name
            );
            routes.push(consumer_route(
                &format!("mcp-{}-resource-{}", block.server.name, resource.name),
                from,
                security_policy.clone(),
            ));
        }
    }

    Ok(routes)
}

/// Expand MCP blocks into consumer route entries and append them to `routes`.
/// Shared by the YAML and JSON parsers so both run the MCP lowering on every
/// parse path. No-op when there are no MCP blocks.
pub fn expand_mcp_into(
    routes: &mut Vec<RouteDslRoute>,
    blocks: &[RouteDslMcp],
) -> Result<(), CamelError> {
    if blocks.is_empty() {
        return Ok(());
    }
    let lowered = lower_all_mcp_to_routes(blocks)?;
    routes.extend(lowered);
    Ok(())
}

/// Build a consumer-shaped `RouteDslRoute` with no processing steps — the MCP
/// consumer registers the tool/resource on the shared listener and submits the
/// tool call/read into the route pipeline as the route's input. The block's
/// server `security_policy` (when present) rides on every lowered route so
/// enforcement is route-level, not block-level.
fn consumer_route(
    id: &str,
    from: String,
    security_policy: Option<RouteDslSecurityPolicy>,
) -> RouteDslRoute {
    RouteDslRoute {
        id: id.to_string(),
        from,
        parameters: BTreeMap::new(),
        steps: Vec::new(),
        auto_startup: true,
        startup_order: 0,
        sequential: false,
        concurrent: None,
        error_handler: None,
        circuit_breaker: None,
        security_policy,
        on_complete: None,
        on_failure: None,
    }
}

#[cfg(test)]
mod tests {
    // serde_yml migrated to noyalib (compat-serde-yaml shim) — closes
    // RUSTSEC-2025-0068. Module alias preserves call-site paths byte-for-byte.
    use noyalib::compat::serde_yaml as serde_yml;

    use super::*;
    use crate::route_ast::{RouteDslRoute, RouteDslRoutes};

    #[test]
    fn parse_mcp_block_from_yaml() {
        let yaml = r#"
mcp:
  - server:
      name: crm
      bind: 127.0.0.1:9100
      security_policy:
        roles: [admin]
    tools:
      - name: lookup
        input_schema:
          type: object
          properties:
            id:
              type: string
          required: [id]
    resources:
      - name: customers
        uri: crm://customers
"#;
        let parsed: RouteDslRoutes = serde_yml::from_str(yaml).unwrap();
        assert_eq!(parsed.mcp.len(), 1);
        let mcp = &parsed.mcp[0];

        assert_eq!(mcp.server.name, "crm");
        assert_eq!(mcp.server.bind, "127.0.0.1:9100");
        assert_eq!(
            mcp.server
                .security_policy
                .as_ref()
                .and_then(|sp| sp.roles.clone()),
            Some(vec!["admin".to_string()])
        );
        assert!(mcp.server.tls.is_none());

        assert_eq!(mcp.tools.len(), 1);
        let tool = &mcp.tools[0];
        assert_eq!(tool.name, "lookup");
        assert_eq!(
            tool.input_schema,
            serde_json::json!({
                "type": "object",
                "properties": { "id": { "type": "string" } },
                "required": ["id"]
            })
        );

        assert_eq!(mcp.resources.len(), 1);
        let resource = &mcp.resources[0];
        assert_eq!(resource.name, "customers");
        assert_eq!(resource.uri, "crm://customers");
    }

    #[test]
    fn unknown_server_key_rejected() {
        let yaml = r#"
mcp:
  - server:
      name: crm
      bind: 127.0.0.1:9100
      session: true
"#;
        let result = serde_yml::from_str::<RouteDslRoutes>(yaml);
        assert!(result.is_err(), "session key must be rejected");
    }

    #[test]
    fn caps_default_to_none() {
        // A block with no cap keys parses to `None` caps — "declared" must
        // stay distinguishable from "defaulted" (the 128 default is applied
        // by the runtime after the TOML/DSL merge, never fabricated here).
        let yaml = r#"
mcp:
  - server:
      name: crm
      bind: 127.0.0.1:9100
"#;
        let parsed: RouteDslRoutes = serde_yml::from_str(yaml).unwrap();
        let mcp = &parsed.mcp[0];
        assert_eq!(mcp.server.max_tools, None);
        assert_eq!(mcp.server.max_resources, None);
        assert!(mcp.tools.is_empty());
        assert!(mcp.resources.is_empty());
    }

    #[test]
    fn dsl_tls_block_parses_typed() {
        // A `tls:` DSL block parses to the typed shape (cert/key paths,
        // trimmed non-empty at deserialize) — Task 2.3 review follow-up.
        let yaml = r#"
mcp:
  - server:
      name: crm
      bind: 127.0.0.1:9100
      tls:
        cert_path: /etc/certs/crm.pem
        key_path: /etc/certs/crm-key.pem
"#;
        let parsed: RouteDslRoutes = serde_yml::from_str(yaml).unwrap();
        let tls = parsed.mcp[0]
            .server
            .tls
            .as_ref()
            .expect("tls block must parse to Some");
        assert_eq!(tls.cert_path, "/etc/certs/crm.pem");
        assert_eq!(tls.key_path, "/etc/certs/crm-key.pem");
    }

    // ── Carried mandatory minors from 3.1 review (deny_unknown_fields pins) ──

    #[test]
    fn initialize_at_block_level_rejected() {
        // `initialize:` beside `server:` at block top level must be a parse
        // error — the DSL carries no session/protocol-version keys (block
        // docstring). Pins deny_unknown_fields beyond the server level.
        let yaml = r#"
mcp:
  - server:
      name: crm
      bind: 127.0.0.1:9100
    initialize: true
"#;
        let result = serde_yml::from_str::<RouteDslRoutes>(yaml);
        assert!(
            result.is_err(),
            "initialize key at block level must be rejected"
        );
    }

    #[test]
    fn unknown_tool_key_rejected() {
        // An unknown key inside a tool entry must be a parse error — pins
        // deny_unknown_fields on RouteDslMcpTool.
        let yaml = r#"
mcp:
  - server:
      name: crm
      bind: 127.0.0.1:9100
    tools:
      - name: lookup
        input_schema:
          type: object
        session: true
"#;
        let result = serde_yml::from_str::<RouteDslRoutes>(yaml);
        assert!(
            result.is_err(),
            "unknown key inside a tool must be rejected"
        );
    }

    // ── Lowering tests (Task 3.2) ──

    fn make_block() -> RouteDslMcp {
        RouteDslMcp {
            server: RouteDslMcpServer {
                name: "crm".to_string(),
                bind: "127.0.0.1:9100".to_string(),
                tls: None,
                security_policy: None,
                max_tools: None,
                max_resources: None,
            },
            tools: vec![],
            resources: vec![],
        }
    }

    fn make_existing_route() -> RouteDslRoute {
        RouteDslRoute {
            id: "existing".to_string(),
            from: "direct:existing".to_string(),
            parameters: BTreeMap::new(),
            steps: vec![],
            auto_startup: true,
            startup_order: 0,
            sequential: false,
            concurrent: None,
            error_handler: None,
            circuit_breaker: None,
            security_policy: None,
            on_complete: None,
            on_failure: None,
        }
    }

    #[test]
    fn dsl_block_lowers_to_consumer_routes() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "id": { "type": "string" } },
            "required": ["id"]
        });
        let mut block = make_block();
        block.tools = vec![RouteDslMcpTool {
            name: "lookup".to_string(),
            input_schema: schema.clone(),
        }];

        let routes = lower_all_mcp_to_routes(&[block]).unwrap();
        let tool_route = routes
            .iter()
            .find(|r| r.from.starts_with("mcp:crm/tool/lookup?schema="))
            .expect("tool consumer route must be present");

        // The schema query param URL-decodes back to the declared schema
        // (the declared-params suffix behind the first `&` is not part of
        // the schema value).
        let encoded = tool_route
            .from
            .strip_prefix("mcp:crm/tool/lookup?schema=")
            .unwrap()
            .split('&')
            .next()
            .unwrap();
        let decoded = percent_encoding::percent_decode_str(encoded)
            .decode_utf8()
            .unwrap()
            .to_string();
        assert_eq!(decoded, schema.to_string());
    }

    #[test]
    fn lowering_carries_declared_params() {
        // The listener declaration rides the lowered from-URI as
        // `mcp.declared.*` parameters (percent-encoded values) — the channel
        // consumer start consumes for DSL listener ownership. Without this
        // pin a lowering regression could silently drop the declaration and
        // the block would fall back to TOML-only behavior.
        let mut block = make_block();
        block.server.bind = "127.0.0.1:9100".to_string();
        block.server.tls = Some(RouteDslMcpTlsConfig {
            cert_path: "/etc/certs/crm.pem".to_string(),
            key_path: "/etc/certs/crm-key.pem".to_string(),
        });
        block.server.max_tools = Some(200);
        block.server.max_resources = Some(64);
        block.tools = vec![RouteDslMcpTool {
            name: "lookup".to_string(),
            input_schema: serde_json::json!({ "type": "object" }),
        }];

        let routes = lower_all_mcp_to_routes(&[block]).unwrap();
        let from = &routes[0].from;
        // Decode round-trip instead of pinning the exact escape set: every
        // declared value must survive the URI encoding intact.
        let declared_value = |name: &str| {
            from.split('&')
                .find(|pair| pair.starts_with(name))
                .and_then(|pair| pair.split_once('='))
                .map(|(_, value)| {
                    percent_encoding::percent_decode_str(value)
                        .decode_utf8()
                        .unwrap()
                        .to_string()
                })
        };
        assert_eq!(
            declared_value("mcp.declared.bind").as_deref(),
            Some("127.0.0.1:9100"),
            "declared bind must ride the from-URI, got: {from}"
        );
        assert_eq!(
            declared_value("mcp.declared.max_tools").as_deref(),
            Some("200"),
            "declared max_tools must ride the from-URI, got: {from}"
        );
        assert_eq!(
            declared_value("mcp.declared.max_resources").as_deref(),
            Some("64"),
            "declared max_resources must ride the from-URI, got: {from}"
        );
        assert_eq!(
            declared_value("mcp.declared.tls.cert_path").as_deref(),
            Some("/etc/certs/crm.pem"),
            "declared tls cert path must ride the from-URI, got: {from}"
        );
        assert_eq!(
            declared_value("mcp.declared.tls.key_path").as_deref(),
            Some("/etc/certs/crm-key.pem"),
            "declared tls key path must ride the from-URI, got: {from}"
        );
    }

    #[test]
    fn resource_lowers_with_uri() {
        let mut block = make_block();
        block.resources = vec![RouteDslMcpResource {
            name: "customers".to_string(),
            uri: "crm://customers".to_string(),
        }];

        let routes = lower_all_mcp_to_routes(&[block]).unwrap();
        let resource_route = routes
            .iter()
            .find(|r| r.from.starts_with("mcp:crm/resource/customers?uri="))
            .expect("resource consumer route must be present");

        let encoded = resource_route
            .from
            .strip_prefix("mcp:crm/resource/customers?uri=")
            .unwrap()
            .split('&')
            .next()
            .unwrap();
        // The resource URI must be percent-encoded (non-alphanumerics escaped),
        // then decode back to the declared operator-config URI.
        assert!(
            encoded.contains('%'),
            "uri must be percent-encoded, got: {encoded}"
        );
        let decoded = percent_encoding::percent_decode_str(encoded)
            .decode_utf8()
            .unwrap()
            .to_string();
        assert_eq!(decoded, "crm://customers");
    }

    #[test]
    fn name_with_invalid_charset_rejected() {
        // A tool name carrying '?' would truncate the lowered URI (and can
        // shadow the `schema` param); '/' breaks the segment shape. Lowering
        // must reject it, naming the offending key (bd rc-ap58).
        let mut block = make_block();
        block.tools = vec![RouteDslMcpTool {
            name: "bad?name".to_string(),
            input_schema: serde_json::json!({ "type": "object" }),
        }];
        let err = lower_all_mcp_to_routes(&[block])
            .err()
            .expect("tool name with '?' must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("bad?name"),
            "error must name the offending key, got: {msg}"
        );
    }

    #[test]
    fn schema_not_in_headers_or_body() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "id": { "type": "string" } },
            "required": ["id"]
        });
        let mut block = make_block();
        block.tools = vec![RouteDslMcpTool {
            name: "lookup".to_string(),
            input_schema: schema.clone(),
        }];

        let routes = lower_all_mcp_to_routes(&[block]).unwrap();
        let route = &routes[0];
        // The schema lives in operator config (the from-URI query), never on
        // the wire as an Exchange header or body template step.
        let schema_str = schema.to_string();
        assert!(
            route
                .steps
                .iter()
                .all(|s| !format!("{s:?}").contains(&schema_str)),
            "schema value must not leak into any header or body step"
        );
    }

    #[test]
    fn expand_mcp_into_appends_to_routes() {
        let mut routes = vec![make_existing_route()];
        let mut block = make_block();
        block.tools = vec![RouteDslMcpTool {
            name: "lookup".to_string(),
            input_schema: serde_json::json!({ "type": "object" }),
        }];
        block.resources = vec![RouteDslMcpResource {
            name: "customers".to_string(),
            uri: "crm://customers".to_string(),
        }];

        expand_mcp_into(&mut routes, &[block]).unwrap();

        // Existing route + one tool route + one resource route.
        assert_eq!(routes.len(), 3);
        assert_eq!(routes[0].id, "existing");
        assert!(
            routes
                .iter()
                .any(|r| r.from.starts_with("mcp:crm/tool/lookup?"))
        );
        assert!(
            routes
                .iter()
                .any(|r| r.from.starts_with("mcp:crm/resource/customers?"))
        );
    }

    #[test]
    fn expand_mcp_into_is_noop_for_empty() {
        let mut routes = vec![make_existing_route()];
        let len = routes.len();
        expand_mcp_into(&mut routes, &[]).unwrap();
        assert_eq!(routes.len(), len, "existing routes must be untouched");
        assert!(lower_all_mcp_to_routes(&[]).unwrap().is_empty());
    }

    #[test]
    fn dsl_policy_propagates_to_lowered_routes() {
        let policy = RouteDslSecurityPolicy {
            roles: Some(vec!["mcp-client".to_string()]),
            scopes: None,
            all_required: None,
            r#ref: None,
            wasm: None,
            config: None,
            permission: None,
            credential_sources: None,
            provider: None,
            audiences: None,
        };
        let mut block = make_block();
        block.server.security_policy = Some(policy);
        block.tools = vec![RouteDslMcpTool {
            name: "lookup".to_string(),
            input_schema: serde_json::json!({ "type": "object" }),
        }];
        block.resources = vec![RouteDslMcpResource {
            name: "customers".to_string(),
            uri: "crm://customers".to_string(),
        }];

        let routes = lower_all_mcp_to_routes(&[block]).unwrap();
        // One tool route + one resource route, each carrying the block policy.
        assert_eq!(routes.len(), 2);
        for route in &routes {
            let sp = route
                .security_policy
                .as_ref()
                .expect("every lowered route must carry the block policy");
            assert_eq!(
                sp.roles,
                Some(vec!["mcp-client".to_string()]),
                "lowered route '{}' must carry the roles policy",
                route.id
            );
        }
        assert!(
            routes
                .iter()
                .any(|r| r.from.starts_with("mcp:crm/tool/lookup?")),
            "the tool route must be present"
        );
        assert!(
            routes
                .iter()
                .any(|r| r.from.starts_with("mcp:crm/resource/customers?")),
            "the resource route must be present"
        );
    }
}
