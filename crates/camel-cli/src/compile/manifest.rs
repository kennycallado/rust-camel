//! Operational manifest embedded next to the payload in compiled artifacts.
//!
//! The manifest is canonical UTF-8 JSON with lexicographically ordered keys
//! and source-ordered arrays. It records the logical source name, runtime
//! version, artifact kind, embedded components (derived from endpoint URI
//! schemes), required environment names (`${env:NAME}` tokens WITHOUT
//! defaults — defaulted tokens resolve at runtime and name nothing), and
//! listener declarations from the canonical REST/MCP port fields as literal
//! addresses or unresolved expressions.
//!
//! Derivation runs on the normalized, pre-interpolation document text: the
//! artifact captures authoring text before `${env:}` resolution, so every
//! field here must tolerate unresolved placeholders.

use std::sync::OnceLock;

use noyalib::compat::serde_yaml as serde_yml;
use regex::Regex;

use super::CompileError;
use super::trailer::TrailerKind;

/// Runtime version recorded in every manifest built by this crate.
pub const RUNTIME_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Operational manifest of a compiled artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    /// Logical source name: input path relative to the compile working
    /// directory. Runtime source identity becomes `compiled://<source_name>`.
    pub source_name: String,
    /// Runtime version of the compiling CLI.
    pub runtime_version: String,
    /// Embedded artifact kind.
    pub kind: TrailerKind,
    /// Endpoint URI schemes referenced by the document, in source order.
    pub components: Vec<String>,
    /// `${env:NAME}` names WITHOUT defaults, in source order. A defaulted
    /// token never requires its variable, so it is never listed.
    pub env_names: Vec<String>,
    /// Listener declarations from the canonical REST/MCP port fields, in
    /// source order: `host:port` for REST blocks, the `bind` value verbatim
    /// for MCP blocks. Literal ports and unresolved expressions pass through
    /// as written.
    pub listeners: Vec<String>,
}

impl Manifest {
    /// Serialize to canonical UTF-8 JSON: compact, lexicographically ordered
    /// keys (serde_json maps are BTreeMaps), source-ordered arrays.
    pub fn to_canonical_json(&self) -> String {
        let value = serde_json::json!({
            "components": self.components,
            "env_names": self.env_names,
            "kind": self.kind.as_str(),
            "listeners": self.listeners,
            "runtime_version": self.runtime_version,
            "source_name": self.source_name,
        });
        serde_json::to_string(&value).expect("json! of strings and arrays cannot fail") // allow-unwrap
    }
}

/// Derive the manifest from a normalized, pre-interpolation document.
pub fn derive(
    source_name: &str,
    kind: TrailerKind,
    document: &str,
) -> Result<Manifest, CompileError> {
    let root = parse_document(document)
        .map_err(|e| CompileError::InvalidDocument(format!("not a YAML/JSON document: {e}")))?;
    let mut components = Vec::new();
    let mut listeners = Vec::new();
    walk_document(&root, &mut components, &mut listeners);
    let env_names = scan_env_names(document);
    Ok(Manifest {
        source_name: source_name.to_string(),
        runtime_version: RUNTIME_VERSION.to_string(),
        kind,
        components,
        env_names,
        listeners,
    })
}

/// Parser used for the manifest Value walk: the same YAML shim camel-dsl
/// uses, which also accepts JSON documents (YAML superset).
fn parse_document(document: &str) -> Result<serde_yml::Value, String> {
    serde_yml::from_str(document).map_err(|e| e.to_string())
}

/// Collect components and listeners from the document tree.
///
/// Generic recursive walk (not typed-AST): the raw document still carries
/// unresolved `${env:}` tokens, so typed fields like `rest.port: u16` cannot
/// deserialize. Endpoint strings are found under `from`/`to` keys at any
/// depth; listeners under the canonical `rest[]` and `mcp[].server` blocks.
fn walk_document(
    value: &serde_yml::Value,
    components: &mut Vec<String>,
    listeners: &mut Vec<String>,
) {
    match value {
        serde_yml::Value::Mapping(map) => {
            for (key, val) in map {
                let key = key.as_str();
                if (key == "from" || key == "to")
                    && let Some(uri) = val.as_str()
                {
                    push_scheme(components, uri);
                }
                if key == "rest"
                    && let Some(entries) = val.as_sequence()
                {
                    for entry in entries {
                        push_rest_listener(listeners, entry);
                    }
                }
                if key == "mcp"
                    && let Some(entries) = val.as_sequence()
                {
                    for entry in entries {
                        push_mcp_listener(listeners, entry);
                    }
                }
                walk_document(val, components, listeners);
            }
        }
        serde_yml::Value::Sequence(seq) => {
            for item in seq {
                walk_document(item, components, listeners);
            }
        }
        _ => {}
    }
}

/// Record the scheme of one endpoint URI (text before the first `:`).
/// Only RFC 3986 scheme shapes are recorded: a prefix that is not
/// `ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )` carries no derivable scheme
/// pre-interpolation (e.g. a whole `${env:NAME}` token) and is skipped.
fn push_scheme(components: &mut Vec<String>, uri: &str) {
    let Some((scheme, _rest)) = uri.split_once(':') else {
        return;
    };
    if !is_uri_scheme(scheme) {
        return;
    }
    if !components.iter().any(|s| s == scheme) {
        components.push(scheme.to_string());
    }
}

/// RFC 3986 scheme grammar: leading ALPHA, then ALPHA / DIGIT / "+" / "-" / ".".
/// Shared with `compile::policy` for endpoint scheme classification.
pub(crate) fn is_uri_scheme(scheme: &str) -> bool {
    let mut chars = scheme.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// REST block listener entry -> `host:port`, honoring the canonical host/port
/// defaults when the raw block omits them.
///
/// SYNC: defaults mirror camel-dsl `default_rest_host`/`default_rest_port`
/// (`0.0.0.0` / `8080`); update together.
fn push_rest_listener(listeners: &mut Vec<String>, entry: &serde_yml::Value) {
    let Some(map) = entry.as_mapping() else {
        return;
    };
    let host = map
        .get("host")
        .and_then(serde_yml::Value::as_str)
        .unwrap_or("0.0.0.0");
    let port = match map.get("port") {
        Some(v) => match v {
            serde_yml::Value::Number(n) => n.to_string(),
            serde_yml::Value::String(s) => s.clone(),
            _ => "8080".to_string(),
        },
        None => "8080".to_string(),
    };
    listeners.push(format!("{host}:{port}"));
}

/// MCP block listener entry -> the `server.bind` value verbatim (an IP:port
/// literal or an unresolved `${env:}` expression, as written).
fn push_mcp_listener(listeners: &mut Vec<String>, entry: &serde_yml::Value) {
    let Some(map) = entry.as_mapping() else {
        return;
    };
    let Some(server) = map.get("server").and_then(serde_yml::Value::as_mapping) else {
        return;
    };
    let Some(bind) = server.get("bind").and_then(serde_yml::Value::as_str) else {
        return;
    };
    listeners.push(bind.to_string());
}

/// Token scanner for `${env:NAME}` / `${env:NAME:-default}` placeholders.
///
/// SYNC: grammar mirrors camel-dsl `env_interpolation` `env_regex()` (escape
/// forms `$${env:...}` and `$$` are literal text, never substitutions);
/// crate purity forbids the dependency. Update both together.
fn env_token_re() -> &'static Regex {
    static ENV_RE: OnceLock<Regex> = OnceLock::new();
    ENV_RE.get_or_init(|| {
        Regex::new(r"(\$\$\{env:[^}]*\})|(\$\$)|(\$\{env:([A-Za-z_][A-Za-z0-9_]*)(?::-([^}]*))?\})")
            .unwrap() // allow-unwrap
    })
}

/// Collect `${env:NAME}` names WITHOUT defaults, in source order, deduped.
/// Escaped forms and defaulted tokens are excluded: neither requires the
/// variable at runtime.
fn scan_env_names(document: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for caps in env_token_re().captures_iter(document) {
        if caps.get(1).is_some() || caps.get(2).is_some() {
            continue; // escaped literal text, not a substitution
        }
        if caps.get(5).is_some() {
            continue; // has a default (even empty): not required
        }
        let name = &caps[4];
        if !names.iter().any(|n| n == name) {
            names.push(name.to_string());
        }
    }
    names
}

// ---------------------------------------------------------------------------
// Tests. The blessed manifest test lives at MODULE level (not in a nested
// `mod tests`) so the mandated filter command
// `cargo test -p camel-cli --lib compile::manifest::<name>` matches exactly.
// Extra coverage lives in `mod tests` below.
// ---------------------------------------------------------------------------

#[test]
fn compile_manifest_lists_components_env_and_listeners() {
    let document = "\
routes:
  - id: tick
    from: timer:tick
    steps:
      - to: kafka:events
  - id: ingest
    from: ${env:INPUT}
    steps:
      - to: file:out?dir=${env:OUT_DIR:-/tmp/spool}
rest:
  - port: 8080
    host: 0.0.0.0
    operations:
      - get: /health
        to: direct:health
mcp:
  - server:
      name: tools
      bind: \"127.0.0.1:${env:MCP_PORT}\"
    tools: []
";

    let manifest = derive("routes/app.yaml", TrailerKind::Route, document)
        .expect("supported document must derive a manifest");

    assert_eq!(manifest.source_name, "routes/app.yaml");
    assert_eq!(manifest.kind, TrailerKind::Route);
    assert_eq!(manifest.runtime_version, RUNTIME_VERSION);

    // Endpoint URI schemes in source order (timer, kafka, file; the
    // `${env:INPUT}` from-URI has no derivable scheme before interpolation
    // and the direct: operation target adds the last one).
    assert_eq!(
        manifest.components,
        vec!["timer", "kafka", "file", "direct"]
    );

    // Required env names, source order; OUT_DIR has a default and is
    // never required.
    assert_eq!(manifest.env_names, vec!["INPUT", "MCP_PORT"]);

    // Listeners: literal REST port and unresolved MCP bind expression.
    assert_eq!(
        manifest.listeners,
        vec!["0.0.0.0:8080", "127.0.0.1:${env:MCP_PORT}"]
    );

    // Canonical JSON: lexicographic keys, compact, deterministic.
    let json = manifest.to_canonical_json();
    assert_eq!(
        json,
        format!(
            "{{\"components\":[\"timer\",\"kafka\",\"file\",\"direct\"],\
\"env_names\":[\"INPUT\",\"MCP_PORT\"],\"kind\":\"route\",\
\"listeners\":[\"0.0.0.0:8080\",\"127.0.0.1:${{env:MCP_PORT}}\"],\
\"runtime_version\":\"{RUNTIME_VERSION}\",\
\"source_name\":\"routes/app.yaml\"}}"
        )
    );
    // The string is itself valid JSON round-tripping to the same values.
    let reparsed: serde_json::Value =
        serde_json::from_str(&json).expect("canonical manifest JSON must parse");
    assert_eq!(
        reparsed.get("kind").and_then(serde_json::Value::as_str),
        Some("route")
    );
    assert!(
        !json.contains("OUT_DIR"),
        "defaulted env tokens must not appear in the manifest"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_derive_rejects_unparsable_document() {
        // Unclosed flow sequence: neither valid YAML nor valid JSON.
        let err = derive("x.yaml", TrailerKind::Route, "key: [unclosed")
            .expect_err("unparsable document must be named");
        assert!(matches!(err, CompileError::InvalidDocument(_)));
    }

    #[test]
    fn manifest_rest_defaults_applied_when_fields_absent() {
        let document = "rest:\n  - operations:\n      - get: /x\n        to: direct:a\n";
        let manifest = derive("a.yaml", TrailerKind::Route, document).expect("derives");
        assert_eq!(manifest.listeners, vec!["0.0.0.0:8080"]);
    }

    #[test]
    fn manifest_env_scan_skips_escapes_and_defaults() {
        let names = scan_env_names(
            "$${env:LIT} $$ ${env:REQUIRED} ${env:WITH_DEFAULT:-x} ${env:EMPTY_DEFAULT:-} \
             ${env:REQUIRED}",
        );
        assert_eq!(names, vec!["REQUIRED"]);
    }
}
