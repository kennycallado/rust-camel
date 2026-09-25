//! Compile-time asset policy for the revised r2embed matrix (Task 1.2).
//!
//! The policy has two halves, both fail-closed and both walking the
//! normalized, pre-interpolation document text:
//!
//! **Rejection** ([`reject_entry_document_assets`] for the entry document
//! and [`reject_unsupported_assets`] for every additionally embedded
//! route/job document) keeps rejecting the classes the artifact format
//! still cannot embed:
//!
//! - configuration-selection fields (`profiles`, `includes`: profiles
//!   and includes are selected through `--config`/`--profile` and
//!   resolved by [`super::sources`], never declared inside a route/job
//!   document)
//! - nested-document route sources (`routeFiles`/`routeFilesFromRoot`
//!   stay permitted in the entry document, where [`super::sources`]
//!   resolves and embeds them, and rejected in every nested document —
//!   recursive resolution is not supported)
//! - file-valued secret fields
//! - `wasm:` URI operands (R2-only deferral recorded in bd rc-2ygmc)
//!
//! **Collection** ([`collect_document_assets`]) gathers every class of
//! the revised embed matrix so [`super::sources`] can confine and embed
//! it: TLS-context `cert`/`key`/`client_ca` document fields, the TLS
//! endpoint URI parameters (`tlsCert`/`tlsKey` on `http:`/`https:`/`ws:`/
//! `wss:` endpoints — camel-http reads `tlsCert` regardless of scheme,
//! `camel-component-http/src/lib.rs:805`; the gRPC family
//! `serverCertPath`/`serverKeyPath`/`clientCaPath` server-side and
//! `caCertPath`/`clientCertPath`/`clientKeyPath` client-side),
//! `xslt`/`xsd` fields, `xslt:`/`validator:` URI operands, `sql:file:`
//! URI operands, and `static_dir`/`staticDir` trees. Each reference
//! records its class, declared path, originating document, and the site
//! context for the substitution table: `literal` for document fields,
//! `uri` for URI parameters and URI operands. Dynamic `${env:}`
//! placeholders and absolute paths inside asset fields and TLS URI
//! parameters are named rejections — an embeddable asset needs a
//! root-relative, compile-known path.
//!
//! The certificate/key/CA fields are scoped to TLS and listener contexts
//! (`tls`, `ssl`, `rest`, `mcp` ancestors): a bare `key:` under ordinary
//! message steps (`set_header`, `remove_header`, claim checks, …) is a
//! message key, not a private-key file, and must compile.
//!
//! The document `wasm` field is a security-policy REGISTRY NAME
//! (`DeclarativeSecurityPolicy::Wasm` resolves it through
//! `SecurityPolicyRegistry::get`, `camel-dsl/src/compile.rs:164-179`),
//! not an embedded asset — it compiles as ordinary document data. There
//! is no `sql` or `plugin` document field. Runtime endpoint URI paths
//! (e.g. `file:`, `kafka:`, `log:`), runtime `${env:}` expressions
//! outside asset fields, and deploy-side I/O stay permitted. Top-level
//! job `args:` declarations are ordinary document data, not assets.
//! Every violation is named (field, asset class, and value shape); the
//! walk is fail-closed, never fail-open: an unparsable document is
//! rejected as invalid.

use noyalib::compat::serde_yaml as serde_yml;

use super::CompileError;
use super::manifest::is_uri_scheme;
use super::store::SubstitutionContext;
use super::trailer::TrailerKind;

/// One collected compile-time asset reference (r2embed Task 1.2):
/// class, declared path, originating document, and the substitution
/// site context. Byte spans are computed by [`super::sources`] against
/// the site document's normalized bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetRef {
    /// Asset class name (`certificate`, `private key`, `client CA`,
    /// `xslt stylesheet`, `xsd schema`, `sql file`, `static directory`).
    pub class: &'static str,
    /// The declared path exactly as written in the site document.
    pub declared: String,
    /// Document key or URI parameter the reference was declared in,
    /// for named diagnostics.
    pub field: String,
    /// Site entry logical path (the originating document).
    pub site: String,
    /// Substitution site context: document fields are `literal`, URI
    /// parameters and URI operands are `uri`.
    pub context: SubstitutionContext,
}

/// Asset class for a forbidden document field key, or `None` if allowed.
///
/// The revised matrix classes (`cert`/`key`/`client_ca` in TLS contexts,
/// `xslt`, `xsd`, `static_dir`) are COLLECTED by
/// [`collect_document_assets`], not forbidden; the document `wasm` field
/// is a registry name and ordinary data.
///
/// `allow_route_sources` permits the `routeFiles`/`routeFilesFromRoot`
/// fields: the entry document's route sources are resolved and embedded
/// by [`super::sources`]; every nested document keeps them forbidden.
fn forbidden_field(key: &str, allow_route_sources: bool) -> Option<&'static str> {
    if allow_route_sources && matches!(key, "routeFiles" | "routeFilesFromRoot") {
        return None;
    }
    match key {
        "routeFiles" | "routeFilesFromRoot" | "route_files" | "route_files_from_root" => {
            Some("route source")
        }
        "profiles" => Some("profile"),
        "includes" => Some("include"),
        _ => None,
    }
}

/// Asset class marking the secret family (private keys). Secret-class
/// entries drive the `--embed-secrets` opt-in gate, the 0700 artifact
/// mode, and manifest `class: "secret"` classification — all sites MUST
/// compare against this constant, never a literal.
pub(crate) const SECRET_ASSET_CLASS: &str = "private key";

/// Asset class for a collected document field key, or `None`. The
/// certificate/key/CA family only counts inside a TLS or listener
/// context ([`is_asset_context`]); elsewhere a `key:`/`cert:` field is
/// ordinary data and compiles.
fn collected_field(key: &str, tls_context: bool) -> Option<&'static str> {
    match key {
        "cert" | "certPath" | "cert_path" if tls_context => Some("certificate"),
        "key" | "keyPath" | "key_path" if tls_context => Some(SECRET_ASSET_CLASS),
        "client_ca" | "clientCaPath" | "client_ca_path" if tls_context => Some("client CA"),
        "xslt" => Some("xslt stylesheet"),
        "xsd" => Some("xsd schema"),
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

/// The still-forbidden endpoint URI schemes: `wasm:` module operands are
/// an R2-only deferral (bd rc-2ygmc). `xslt:`, `validator:`, and
/// `sql:file:` operands moved to [`collect_uri`] (collected assets);
/// inline-query `sql:` endpoints stay permitted — they carry runtime
/// datasource queries, not assets.
fn forbidden_scheme(scheme: &str) -> Option<&'static str> {
    match scheme {
        "wasm" => Some("wasm module; recorded R2 deferral"),
        _ => None,
    }
}

/// Asset class for a collected URI operand scheme, plus whether the
/// operand must carry the `file:` prefix (`sql:file:<path>`). The URI
/// path after the scheme (query stripped) is the declared asset path.
fn uri_operand_class(scheme: &str) -> Option<(&'static str, bool)> {
    match scheme {
        "xslt" => Some(("xslt stylesheet", false)),
        "validator" => Some(("xsd schema", false)),
        "sql" => Some(("sql file", true)),
        _ => None,
    }
}

/// Asset class for a TLS endpoint URI parameter, or `None`.
/// `tlsCert`/`tlsKey` ride HTTP and WS endpoint URIs (camel-http reads
/// `tlsCert` regardless of scheme, `camel-component-http/src/lib.rs:805`,
/// `camel-component-ws/src/config.rs:96`); the gRPC family splits into
/// server-side (`serverCertPath`/`serverKeyPath`/`clientCaPath`) and
/// client-side (`caCertPath`/`clientCertPath`/`clientKeyPath`) spellings
/// (`camel-component-grpc/src/config.rs:484-490`).
fn tls_uri_param_class(scheme: &str, name: &str) -> Option<&'static str> {
    let httpish = matches!(scheme, "http" | "https" | "ws" | "wss");
    let grpc = scheme == "grpc";
    if !httpish && !grpc {
        return None;
    }
    match name {
        "tlsCert" if httpish => Some("certificate"),
        "tlsKey" if httpish => Some(SECRET_ASSET_CLASS),
        "serverCertPath" if grpc => Some("certificate"),
        "serverKeyPath" if grpc => Some(SECRET_ASSET_CLASS),
        "clientCaPath" if grpc => Some("client CA"),
        "caCertPath" if grpc => Some("client CA"),
        "clientCertPath" if grpc => Some("certificate"),
        "clientKeyPath" if grpc => Some(SECRET_ASSET_CLASS),
        _ => None,
    }
}

/// RFC 3986 scheme + optional query of an endpoint URI, or `None` when
/// the prefix is not a scheme at all.
fn scheme_and_query(uri: &str) -> Option<(&str, Option<&str>)> {
    let (scheme, rest) = uri.split_once(':')?;
    if !is_uri_scheme(scheme) {
        return None;
    }
    let query = rest.split_once('?').map(|(_, query)| query);
    Some((scheme, query))
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

/// Reject every unsupported compile-time asset named by an additionally
/// embedded route/job document (the NESTED rule: route sources are not
/// recursively resolvable, so they stay forbidden outside the entry
/// document).
///
/// Walks the parsed document tree (YAML shim accepts JSON too) and collects
/// ALL violations, so one diagnostic names every rejected asset class.
pub fn reject_unsupported_assets(
    document_text: &str,
    kind: TrailerKind,
) -> Result<(), CompileError> {
    reject_with(document_text, kind, false)
}

/// Reject every unsupported compile-time asset named by the ENTRY
/// document: identical to [`reject_unsupported_assets`] except that the
/// document's own `routeFiles`/`routeFilesFromRoot` declarations are
/// permitted — [`super::sources`] resolves, confines, and embeds them.
pub fn reject_entry_document_assets(
    document_text: &str,
    kind: TrailerKind,
) -> Result<(), CompileError> {
    reject_with(document_text, kind, true)
}

/// Shared fail-closed walk entry point.
fn reject_with(
    document_text: &str,
    kind: TrailerKind,
    allow_route_sources: bool,
) -> Result<(), CompileError> {
    let root: serde_yml::Value = serde_yml::from_str(document_text)
        .map_err(|e| CompileError::InvalidDocument(format!("not a YAML/JSON document: {e}")))?;
    let mut violations: Vec<String> = Vec::new();
    walk(&root, kind, &mut violations, allow_route_sources);
    if violations.is_empty() {
        Ok(())
    } else {
        Err(CompileError::UnsupportedAsset(violations.join("; ")))
    }
}

/// Recursive rejection walk collecting named violations.
fn walk(
    value: &serde_yml::Value,
    kind: TrailerKind,
    violations: &mut Vec<String>,
    allow_route_sources: bool,
) {
    match value {
        serde_yml::Value::Mapping(map) => {
            for (key, val) in map {
                let key = key.as_str();
                if let Some(class) = forbidden_field(key, allow_route_sources) {
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
                walk(val, kind, violations, allow_route_sources);
            }
        }
        serde_yml::Value::Sequence(seq) => {
            for item in seq {
                walk(item, kind, violations, allow_route_sources);
            }
        }
        _ => {}
    }
}

/// Collect every revised-matrix asset reference declared in one
/// normalized document (r2embed Task 1.2). The returned references carry
/// class, declared path, originating document, and site context; byte
/// spans and confinement belong to [`super::sources`].
pub fn collect_document_assets(
    site: &str,
    document_text: &str,
) -> Result<Vec<AssetRef>, CompileError> {
    let root: serde_yml::Value = serde_yml::from_str(document_text)
        .map_err(|e| CompileError::InvalidDocument(format!("not a YAML/JSON document: {e}")))?;
    let mut refs = Vec::new();
    collect_walk(&root, site, &mut refs, false)?;
    Ok(refs)
}

/// Recursive collection walk. `tls_context` tracks whether the walk is
/// inside a TLS/listener subtree.
fn collect_walk(
    value: &serde_yml::Value,
    site: &str,
    refs: &mut Vec<AssetRef>,
    tls_context: bool,
) -> Result<(), CompileError> {
    match value {
        serde_yml::Value::Mapping(map) => {
            for (key, val) in map {
                let key = key.as_str();
                if let Some(class) = collected_field(key, tls_context) {
                    for declared in string_values(key, class, val)? {
                        refs.push(checked(
                            key,
                            class,
                            &declared,
                            site,
                            SubstitutionContext::Literal,
                        )?);
                    }
                } else {
                    for uri in uri_strings(key, val) {
                        collect_uri(uri, key, site, refs)?;
                    }
                }
                collect_walk(val, site, refs, tls_context || is_asset_context(key))?;
            }
        }
        serde_yml::Value::Sequence(seq) => {
            for item in seq {
                collect_walk(item, site, refs, tls_context)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Path strings of a collected field's value: one string or a sequence
/// of strings. Any other shape is a named rejection (fail-closed — an
/// asset field must hold compile-known paths).
fn string_values(
    key: &str,
    class: &str,
    value: &serde_yml::Value,
) -> Result<Vec<String>, CompileError> {
    match value {
        serde_yml::Value::String(s) => Ok(vec![s.clone()]),
        serde_yml::Value::Sequence(seq) => seq
            .iter()
            .map(|item| {
                item.as_str().map(str::to_string).ok_or_else(|| {
                    CompileError::UnsupportedAsset(format!(
                        "field '{key}' ({class}): value must be a path string or a list of \
                         path strings"
                    ))
                })
            })
            .collect(),
        _ => Err(CompileError::UnsupportedAsset(format!(
            "field '{key}' ({class}): value must be a path string or a list of path strings"
        ))),
    }
}

/// Classify one endpoint URI: still-forbidden schemes are skipped (the
/// rejection walk owns them), collected operand schemes and TLS URI
/// parameters yield asset references with `uri` site context.
fn collect_uri(
    uri: &str,
    field: &str,
    site: &str,
    refs: &mut Vec<AssetRef>,
) -> Result<(), CompileError> {
    let Some((scheme, query)) = scheme_and_query(uri) else {
        return Ok(());
    };
    if forbidden_scheme(scheme).is_some() {
        return Ok(());
    }
    if let Some((class, requires_file)) = uri_operand_class(scheme) {
        let rest = &uri[scheme.len() + 1..];
        let operand = rest.split('?').next().unwrap_or(rest);
        let path = if requires_file {
            operand.strip_prefix("file:")
        } else {
            Some(operand)
        };
        // An empty operand names no asset; inline-query `sql:` endpoints
        // (no `file:` prefix) carry datasource queries, not assets.
        if let Some(path) = path
            && !path.is_empty()
        {
            refs.push(checked(field, class, path, site, SubstitutionContext::Uri)?);
        }
        return Ok(());
    }
    if let Some(query) = query {
        for pair in query.split('&') {
            if let Some((name, value)) = pair.split_once('=')
                && let Some(class) = tls_uri_param_class(scheme, name)
            {
                refs.push(checked(name, class, value, site, SubstitutionContext::Uri)?);
            }
        }
    }
    Ok(())
}

/// Named rejections every collected reference must pass: dynamic
/// `${env:}` placeholders and absolute paths cannot be embedded (an
/// embeddable asset needs a root-relative, compile-known path).
fn checked(
    field: &str,
    class: &'static str,
    declared: &str,
    site: &str,
    context: SubstitutionContext,
) -> Result<AssetRef, CompileError> {
    if declared.contains("${") {
        return Err(CompileError::UnsupportedAsset(format!(
            "field '{field}' ({class}): dynamic placeholder '{declared}' — an unknowable \
             asset path cannot be embedded"
        )));
    }
    if declared.starts_with('/') {
        return Err(CompileError::UnsupportedAsset(format!(
            "field '{field}' ({class}): absolute path '{declared}' — an embeddable asset \
             requires a root-relative, compile-known path"
        )));
    }
    Ok(AssetRef {
        class,
        declared: declared.to_string(),
        field: field.to_string(),
        site: site.to_string(),
        context,
    })
}

/// Reject bean plugin declarations and WASM security modules in the
/// selected `Camel.toml` chain (config document and includes) —
/// fail-closed R2-only deferrals (bd rc-fjutr): bean plugins load
/// `<cwd>/plugins/<name>.wasm` from the deployment working directory and
/// `[security.permissions.<name>]` WASM providers consume a
/// path-taking constructor, so neither can be confined or substituted
/// in-lease.
///
/// The walk covers every level that merges into the effective root at
/// runtime (review F2): the document root, `[default]`, and each
/// selected profile section (`section_walk` order) — a
/// `[prod.beans.<name>]` plugin escapes nothing when `prod` is
/// selected, while an unselected section never merges and stays
/// permitted. Any `[security.policies.wasm.<name>]` module is rejected
/// by name at every walked level: a security-policy WASM module reads a
/// host file-system path at boot, and security-policy WASM never
/// reaches an artifact in R2.
pub fn reject_config_assets(
    source: &str,
    config: &toml::Value,
    profiles: &[String],
) -> Result<(), CompileError> {
    reject_config_level(source, config)?;
    for section in camel_dsl::config_semantics::section_walk(profiles) {
        if let Some(table) = config
            .get(section.as_str())
            .filter(|value| value.is_table())
        {
            let owner = format!("{source} [{section}]");
            reject_config_level(&owner, table)?;
        }
    }
    Ok(())
}

/// Reject the bean-plugin, WASM-permission, and WASM-security-policy
/// shapes in one configuration table (a document root or one profile
/// section).
fn reject_config_level(source: &str, config: &toml::Value) -> Result<(), CompileError> {
    if let Some(beans) = config.get("beans").and_then(|beans| beans.as_table()) {
        for (name, entry) in beans {
            if entry.get("plugin").is_some() {
                return Err(CompileError::UnsupportedAsset(format!(
                    "bean '{name}' ({source}): plugin declarations cannot be embedded — bean \
                     plugins load '<cwd>/plugins/<name>.wasm' from the deployment working \
                     directory, which cannot be confined or substituted (recorded R2 \
                     deferral)"
                )));
            }
        }
    }
    if let Some(permissions) = config
        .get("security")
        .and_then(|security| security.get("permissions"))
        .and_then(|permissions| permissions.as_table())
    {
        for (name, entry) in permissions {
            if entry.get("provider").and_then(|provider| provider.as_str()) == Some("wasm") {
                return Err(CompileError::UnsupportedAsset(format!(
                    "security permission '{name}' ({source}): WASM providers use a \
                     path-taking policy constructor that cannot consume embedded bytes in \
                     R2 (recorded deferral)"
                )));
            }
        }
    }
    if let Some(policies) = config
        .get("security")
        .and_then(|security| security.get("policies"))
        .and_then(|policies| policies.get("wasm"))
        .and_then(|wasm| wasm.as_table())
        && let Some(name) = policies.keys().next()
    {
        return Err(CompileError::UnsupportedAsset(format!(
            "security policy '{name}' ({source}): WASM security-policy modules read a \
             host file-system path at boot and security-policy WASM never reaches an \
             artifact in R2 (recorded deferral)"
        )));
    }
    Ok(())
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

    /// The revised matrix collects TLS document fields, `wasm:` operand
    /// URIs stay rejected, and the `wasm` REGISTRY-NAME field is
    /// ordinary data (r2embed Task 1.2).
    #[test]
    fn collection_and_rejection_split_the_revised_matrix() {
        let doc = "\
routes:
  - id: r
    from: timer:t
    steps:
      - to: 'wasm:module.wasm'
    tls:
      cert: certs/tls.crt
security_policy:
  wasm: registry-policy
";
        let err = reject_entry_document_assets(doc, TrailerKind::Route)
            .expect_err("wasm: operands stay rejected");
        let CompileError::UnsupportedAsset(text) = err else {
            panic!("unexpected error variant");
        };
        assert!(text.contains("wasm:module.wasm"), "err: {text}");
        assert!(!text.contains("registry-policy"), "err: {text}");

        let refs = collect_document_assets("app.yaml", doc).expect("collection succeeds");
        assert_eq!(refs.len(), 1, "only the TLS cert is collected: {refs:?}");
        assert_eq!(refs[0].class, "certificate");
        assert_eq!(refs[0].site, "app.yaml");
        assert_eq!(refs[0].context, SubstitutionContext::Literal);
    }

    /// Config-asset rejection covers every runtime-merged level
    /// (review F2): the root table, `[default]`, and each selected
    /// profile section — a `[prod.beans]` plugin escapes nothing, while
    /// an unselected section never merges and stays permitted — and any
    /// `[security.policies.wasm.<name>]` module is rejected by name:
    /// security-policy WASM never reaches an artifact in R2.
    #[test]
    fn config_asset_rejection_covers_profile_sections_and_wasm_policies() {
        let policies: toml::Value =
            toml::from_str("[security.policies.wasm.corp-auth]\npath = \"plugins/authz.wasm\"\n")
                .expect("parse policies config");
        let err = reject_config_assets("Camel.toml", &policies, &[])
            .expect_err("security-policy WASM never reaches an artifact");
        let CompileError::UnsupportedAsset(text) = err else {
            panic!("unexpected error variant");
        };
        assert!(text.contains("corp-auth"), "err: {text}");
        assert!(text.contains("security policy"), "err: {text}");

        let prod_beans: toml::Value =
            toml::from_str("[prod.beans.loader]\nplugin = \"file-loader\"\n").expect("parse");
        assert!(
            reject_config_assets("Camel.toml", &prod_beans, &[]).is_ok(),
            "an unselected profile section never merges at runtime"
        );
        let err = reject_config_assets("Camel.toml", &prod_beans, &["prod".to_string()])
            .expect_err("a selected profile section's bean plugin must fail closed");
        let CompileError::UnsupportedAsset(text) = err else {
            panic!("unexpected error variant");
        };
        assert!(
            text.contains("loader") && text.contains("prod"),
            "err: {text}"
        );

        let default_beans: toml::Value =
            toml::from_str("[default.beans.loader]\nplugin = \"file-loader\"\n").expect("parse");
        let err = reject_config_assets("Camel.toml", &default_beans, &[])
            .expect_err("[default] merges with no selection at all");
        let CompileError::UnsupportedAsset(text) = err else {
            panic!("unexpected error variant");
        };
        assert!(text.contains("loader"), "err: {text}");
    }
}
