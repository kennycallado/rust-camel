//! Fail-closed v1 compile asset policy.
//!
//! [`reject_unsupported_assets`] inspects the normalized, pre-interpolation
//! document text and rejects every asset class the compiled-artifact v1
//! format cannot embed:
//!
//! - route-source fields (`routeFiles`, `routeFilesFromRoot`, glob patterns)
//! - configuration fields (`profiles`, `includes`; `Camel.toml` and
//!   `CAMEL_*` compile overrides are rejected separately by `run_compile`)
//! - asset-bearing fields (`cert`, `key`, `client_ca`, `wasm`, `plugin`,
//!   `xslt`, `xsd`, `sql`, `static_dir`, file-valued secret fields)
//! - asset-bearing endpoint URI schemes (`wasm:`, `xslt:`, `validator:`)
//!   in every URI-bearing field (`from`, `to`, `wire_tap`, `poll_enrich`,
//!   `enrich`, `dead_letter_channel`, `scatter_gather.endpoints`)
//!
//! The certificate/key/CA fields are scoped to TLS and listener contexts
//! (`tls`, `ssl`, `rest`, `mcp` ancestors): a bare `key:` under ordinary
//! message steps (`set_header`, `remove_header`, claim checks, …) is a
//! message key, not a private-key file, and must compile.
//!
//! Runtime endpoint URI paths (e.g. `file:`, `kafka:`, `log:`), runtime
//! `${env:}` expressions outside forbidden fields, and deploy-side I/O stay
//! permitted. Top-level job `args:` declarations (`required`, `default`,
//! `description`) are ordinary document data, not assets, so declared job
//! documents compile (jobargs Task 3.2). Field names match the blessed v1
//! matrix exactly; the
//! camelCase/path spellings the rest of the codebase uses for the same
//! fields (`route_files`, `certPath`, `clientCaPath`, …) are rejected too,
//! so a rename cannot smuggle an asset through. Every violation is named
//! (field, asset class, and value shape — file reference, glob pattern, or
//! dynamic `${env:}` placeholder); the walk is fail-closed, never
//! fail-open: an unparsable document is rejected as invalid.

use noyalib::compat::serde_yaml as serde_yml;

use super::CompileError;
use super::manifest::is_uri_scheme;
use super::trailer::TrailerKind;

/// Asset class for a forbidden document field key, or `None` if allowed.
///
/// The certificate/key/CA family is only an asset when the walk is inside a
/// TLS or listener context ([`is_asset_context`]); elsewhere (message
/// steps, claim checks, generic maps) a `key:`/`cert:` field is ordinary
/// data and compiles.
fn forbidden_field(key: &str, tls_context: bool) -> Option<&'static str> {
    match key {
        "routeFiles" | "routeFilesFromRoot" | "route_files" | "route_files_from_root" => {
            Some("route source")
        }
        "profiles" => Some("profile"),
        "includes" => Some("include"),
        "cert" | "certPath" | "cert_path" if tls_context => Some("certificate"),
        "key" | "keyPath" | "key_path" if tls_context => Some("private key"),
        "client_ca" | "clientCaPath" | "client_ca_path" if tls_context => Some("client CA"),
        "wasm" => Some("wasm module"),
        "plugin" => Some("plugin file"),
        "xslt" => Some("xslt stylesheet"),
        "xsd" => Some("xsd schema"),
        "sql" => Some("sql file"),
        "static_dir" | "staticDir" => Some("static directory"),
        _ => None,
    }
}

/// Whether descending into the value of `key` enters a TLS or listener
/// context where certificate/key/CA file fields are compile-time assets
/// (REST and MCP listeners, explicit `tls:`/`ssl:` blocks). Descending is
/// sticky: the whole listener subtree stays asset-bearing.
fn is_asset_context(key: &str) -> bool {
    matches!(
        key,
        "tls" | "ssl" | "sslContext" | "ssl_context" | "rest" | "mcp"
    )
}

/// Endpoint-URI strings carried by a URI-bearing field, or empty if the
/// field carries none. `from`/`to`/`wire_tap`/`dead_letter_channel` hold
/// the URI directly; `enrich`/`poll_enrich` hold either the shorthand URI
/// string or the full `{uri: ...}` mapping; `scatter_gather` holds a
/// sequence of endpoint URI strings under `endpoints`.
fn uri_strings<'a>(key: &str, val: &'a serde_yml::Value) -> Vec<&'a str> {
    let enrich = matches!(key, "enrich" | "poll_enrich" | "pollEnrich");
    if (key == "from"
        || key == "to"
        || key == "wire_tap"
        || key == "wireTap"
        || key == "dead_letter_channel"
        || key == "deadLetterChannel"
        || enrich)
        && let Some(uri) = val.as_str()
    {
        return vec![uri];
    }
    if enrich && let Some(uri) = val.get("uri").and_then(|uri| uri.as_str()) {
        return vec![uri];
    }
    if (key == "scatter_gather" || key == "scatterGather")
        && let Some(endpoints) = val.get("endpoints").and_then(|e| e.as_sequence())
    {
        return endpoints.iter().filter_map(|e| e.as_str()).collect();
    }
    Vec::new()
}

/// Asset class for a forbidden asset-bearing endpoint URI scheme (the URI
/// path of these schemes is an external asset file), or `None` if allowed.
///
/// `sql:` endpoints stay permitted: they carry inline queries against
/// runtime datasources, and the blessed matrix forbids SQL *files* (the
/// `sql` document field), not runtime datasource endpoints.
fn forbidden_scheme(scheme: &str) -> Option<&'static str> {
    Some(match scheme {
        "wasm" => "wasm module",
        "xslt" => "xslt stylesheet",
        "validator" => "xsd schema",
        _ => return None,
    })
}

/// Class label for a violation, phrased as a job dependency for job
/// documents (their route source and configuration must live in the
/// document itself).
fn label(kind: TrailerKind, class: &str) -> String {
    match (kind, class) {
        (TrailerKind::Job, "route source" | "profile" | "include") => {
            format!("job dependency: {class}")
        }
        _ => class.to_string(),
    }
}

/// Reject every unsupported compile-time asset named by the document.
///
/// Walks the parsed document tree (YAML shim accepts JSON too) and collects
/// ALL violations, so one diagnostic names every rejected asset class.
pub fn reject_unsupported_assets(
    document_text: &str,
    kind: TrailerKind,
) -> Result<(), CompileError> {
    let root: serde_yml::Value = serde_yml::from_str(document_text)
        .map_err(|e| CompileError::InvalidDocument(format!("not a YAML/JSON document: {e}")))?;
    let mut violations: Vec<String> = Vec::new();
    walk(&root, kind, &mut violations, false);
    if violations.is_empty() {
        Ok(())
    } else {
        Err(CompileError::UnsupportedAsset(violations.join("; ")))
    }
}

/// Recursive document walk collecting named violations. `tls_context`
/// tracks whether the walk is inside a TLS/listener subtree.
fn walk(
    value: &serde_yml::Value,
    kind: TrailerKind,
    violations: &mut Vec<String>,
    tls_context: bool,
) {
    match value {
        serde_yml::Value::Mapping(map) => {
            for (key, val) in map {
                let key = key.as_str();
                if let Some(class) = forbidden_field(key, tls_context) {
                    violations.push(format!(
                        "field '{key}' ({}, {})",
                        label(kind, class),
                        value_shape(val)
                    ));
                } else if let Some(file) = secret_file(key, val) {
                    // The class wording lives outside the `format!` span:
                    // `lint-secrets` flags sensitive field names inside
                    // format macros, and this diagnostic NAMES the class
                    // without printing the file's contents.
                    let class = label(kind, "secret file");
                    violations.push(format!(
                        "field '{key}' ({class} '{file}', {})",
                        value_shape(val)
                    ));
                } else {
                    for uri in uri_strings(key, val) {
                        if let Some((scheme, _)) = uri.split_once(':')
                            && is_uri_scheme(scheme)
                            && let Some(class) = forbidden_scheme(scheme)
                        {
                            violations.push(format!("endpoint '{uri}' ({class})"));
                        }
                    }
                }
                walk(val, kind, violations, tls_context || is_asset_context(key));
            }
        }
        serde_yml::Value::Sequence(seq) => {
            for item in seq {
                walk(item, kind, violations, tls_context);
            }
        }
        _ => {}
    }
}

/// Describe the shape of a forbidden field's value: a dynamic `${env:}`
/// placeholder (dynamic asset path), a glob pattern, or a plain file
/// reference.
fn value_shape(value: &serde_yml::Value) -> &'static str {
    if any_string(value, |s| s.contains("${env:")) {
        "dynamic ${env:} placeholder"
    } else if any_string(value, |s| {
        s.contains('*') || s.contains('?') || (s.contains('[') && s.contains(']'))
    }) {
        "glob pattern"
    } else {
        "file reference"
    }
}

/// First `file:` entry of a `secrets`/`secret` field (a literal secret
/// file the artifact cannot embed), if any.
fn secret_file(key: &str, value: &serde_yml::Value) -> Option<String> {
    if key != "secrets" && key != "secret" {
        return None;
    }
    let mut stack = vec![value];
    while let Some(value) = stack.pop() {
        match value {
            serde_yml::Value::Mapping(map) => {
                for (k, v) in map {
                    if k.as_str() == "file"
                        && let Some(file) = v.as_str()
                    {
                        return Some(file.to_string());
                    }
                    stack.push(v);
                }
            }
            serde_yml::Value::Sequence(seq) => stack.extend(seq.iter()),
            _ => {}
        }
    }
    None
}

/// Whether any string in the tree satisfies `pred`.
fn any_string(value: &serde_yml::Value, pred: fn(&str) -> bool) -> bool {
    match value {
        serde_yml::Value::String(s) => pred(s),
        serde_yml::Value::Mapping(map) => map.values().any(|v| any_string(v, pred)),
        serde_yml::Value::Sequence(seq) => seq.iter().any(|v| any_string(v, pred)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Job `args:` declarations are ordinary document data and compile,
    /// while unsupported route assets stay rejected (jobargs Task 3.2).
    #[test]
    fn job_args_declarations_permitted_and_assets_rejected() {
        let declared = "\
args:
  value:
    required: true
    default: hello
    description: the value to send
execute:
  mode: one-shot
  timeout: 60s
  send:
    to: direct:transform
    body: \"${arg:value}\"
routes:
  - id: job-arg
    from: direct:transform
";
        reject_unsupported_assets(declared, TrailerKind::Job)
            .expect("args declarations are not compile-time assets");

        let err = reject_unsupported_assets(
            "args:\n  value:\n    default: hi\nrouteFiles:\n  - routes/*.yaml\n",
            TrailerKind::Job,
        )
        .expect_err("route-file assets must stay rejected alongside declarations");
        let CompileError::UnsupportedAsset(text) = err else {
            panic!("unexpected error variant");
        };
        assert!(text.contains("routeFiles"), "err: {text}");
    }
}
