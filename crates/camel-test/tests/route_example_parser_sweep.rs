//! Task 2.6 — route-example sweep classifier and regression fixtures.
//!
//! The sweep collects route-producing YAML/JSON documents from `examples/`,
//! `benchmarks/scenarios/` and `docs/` (whole files and Markdown fences),
//! normalizes their envelope shape, parses them with the real declarative
//! parser, and classifies every `script:` step by whether its language
//! supports mutating expressions. Task 2.7 consumes the collector, the
//! envelope selection and `classify_text` to build the per-site baseline
//! ledger; this task supplies them plus the `sweep_flags_*` fixtures.
//!
//! Envelope policy (design: `openspec/changes/364-simpleset/tasks.md`):
//! - a mapping containing `routes`, `rest` or `mcp` is already complete;
//! - a root sequence (`- id:` at column 0) is re-rooted under `routes:`;
//! - a single route object (`from` + `steps`) becomes a one-element `routes`.
//!
//! Anything else is a partial fragment and is excluded by the caller.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, OnceLock};

use camel_api::CamelError;
use camel_dsl::model::{DeclarativeRoute, DeclarativeStep};
use camel_language_api::{Language, LanguageError, LanguagesConfig};
use noyalib::compat::serde_yaml as serde_yml;

/// Source document kind of a swept site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum DocKind {
    Yaml,
    Json,
}

/// Location of a swept document: a whole file or a Markdown fence ordinal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum FenceIndex {
    File,
    /// 0-based ordinal over the `yaml`/`yml`/`json` fences in a `.md` file.
    Fence(usize),
}

/// A route-producing candidate collected from the swept roots.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Site {
    path: std::path::PathBuf,
    fence_index: FenceIndex,
    kind: DocKind,
}

/// Registry of expression languages used to resolve `script:` capabilities.
type LangMap = HashMap<String, Arc<dyn Language>>;

/// Outcome of resolving one `script:` step against the language registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Capability {
    /// The language accepted the source as a mutating expression.
    Mutating,
    /// The language does not support mutating expressions (`NotSupported`).
    NonMutating,
    /// The language supports mutating expressions but rejected the source.
    CreationError,
    /// The language name is not registered.
    Unresolved,
}

/// Classification of a normalized route document.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Classified {
    /// The document parsed; each `script:` step and its capability.
    Parsed { scripts: Vec<(String, Capability)> },
    /// A bare value-source mapping (`<verb>: {constant: ok}`).
    BareMap,
    /// A value-source key with the wrong value type.
    WrongType,
    /// Any other parse failure, message preserved.
    ParseError(String),
}

/// The normalized route document produced by envelope selection.
#[derive(Debug, PartialEq, Eq)]
enum Envelope {
    /// The document is route-producing; carries the normalized text.
    Ready(String),
    /// The document is not route-producing (partial fragment / step list).
    Partial,
    /// The document could not even be inspected (for example invalid JSON).
    Invalid(String),
}

// ---------------------------------------------------------------------------
// Envelope selection and normalization
// ---------------------------------------------------------------------------

/// Select and normalize the route envelope of `text`.
fn select_envelope(text: &str, kind: DocKind) -> Envelope {
    match kind {
        DocKind::Yaml => select_envelope_yaml(text),
        DocKind::Json => select_envelope_json(text),
    }
}

fn select_envelope_yaml(text: &str) -> Envelope {
    let trimmed = text.trim();

    // The parser is the authority on the block-mapping spelling: a document
    // that parses to at least one route is route-producing, whatever key
    // quoting or style it uses. A `RouteError` from the value-source gate also
    // proves the top level deserialized as `RouteDslRoutes` (the gate runs
    // after document shaping), so it stays Ready for the classifier to label.
    match camel_dsl::parse_yaml_to_declarative(trimmed) {
        Ok(routes) if !routes.is_empty() => return Envelope::Ready(trimmed.to_string()),
        Err(ref err) if is_value_source_mapping_error(err) => {
            return Envelope::Ready(trimmed.to_string());
        }
        _ => {}
    }

    // A root sequence is complete only when every element is a route object
    // (`from` present). A step-only sequence is a partial fragment.
    if is_yaml_root_sequence(trimmed) {
        return if root_sequence_is_route_objects(trimmed) {
            Envelope::Ready(indent_block(trimmed, 2, "routes:"))
        } else {
            Envelope::Partial
        };
    }
    // One key-set recognizer for both styles. Block mappings use the column-0
    // scanner; a flow mapping (`{k: v, ...}`) uses the bounded depth-aware
    // extractor, because its `from`/`steps` are hidden after the first key.
    // Recognition never depends on parse success: a malformed complete flow
    // route must surface as a ParseError, never be skipped as Partial.
    let body: String = trimmed
        .lines()
        .skip_while(|line| {
            let text = line.trim();
            text.is_empty() || text.starts_with('#') || text == "---" || text == "..."
        })
        .collect::<Vec<_>>()
        .join("\n");
    let keys = if body.starts_with('{') {
        flow_top_level_keys(&body)
    } else {
        yaml_top_level_keys(trimmed)
    };
    let has = |wanted: &str| keys.iter().any(|key| key == wanted);
    if has("routes") || has("rest") || has("mcp") {
        return Envelope::Ready(trimmed.to_string());
    }
    if has("from") && has("steps") {
        return Envelope::Ready(wrap_single_route(trimmed));
    }
    Envelope::Partial
}

fn select_envelope_json(text: &str) -> Envelope {
    let value: serde_json::Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(err) => return Envelope::Invalid(format!("JSON parse error: {err}")),
    };

    match value {
        serde_json::Value::Object(map) => select_json_object(map),
        serde_json::Value::Array(items) => {
            // A root array is complete only when every element is a route
            // object (`from` present). A step-only array is partial.
            if items.is_empty() || !items.iter().all(is_json_route_object) {
                return Envelope::Partial;
            }
            envelope_to_text(serde_json::json!({ "routes": items }))
        }
        _ => Envelope::Partial,
    }
}

fn select_json_object(map: serde_json::Map<String, serde_json::Value>) -> Envelope {
    if map.contains_key("routes") || map.contains_key("rest") || map.contains_key("mcp") {
        envelope_to_text(serde_json::Value::Object(map))
    } else if map.contains_key("from") && map.contains_key("steps") {
        envelope_to_text(serde_json::json!({
            "routes": [serde_json::Value::Object(map)]
        }))
    } else {
        Envelope::Partial
    }
}

fn is_json_route_object(value: &serde_json::Value) -> bool {
    value
        .as_object()
        .is_some_and(|map| map.contains_key("from"))
}

/// True when the error came from the value-source mapping gate (bare map or
/// wrong-typed field) — proof the document was a route envelope.
fn is_value_source_mapping_error(err: &CamelError) -> bool {
    match err {
        CamelError::RouteError(message) => {
            message.contains("in value-source mapping") || message.contains("wrong type for key")
        }
        _ => false,
    }
}

fn envelope_to_text(value: serde_json::Value) -> Envelope {
    match serde_json::to_string(&value) {
        Ok(text) => Envelope::Ready(text),
        Err(err) => Envelope::Invalid(format!("JSON serialize error: {err}")),
    }
}

/// First line that is neither blank nor a comment/document marker.
fn first_meaningful_line(text: &str) -> Option<&str> {
    text.lines().find(|line| {
        let trimmed = line.trim();
        !trimmed.is_empty() && !trimmed.starts_with('#') && trimmed != "---" && trimmed != "..."
    })
}

/// Top-level mapping keys (lines starting at column 0).
fn yaml_top_level_keys(text: &str) -> Vec<String> {
    let mut keys = Vec::new();
    for raw in text.lines() {
        let trimmed = raw.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if trimmed.len() != raw.len() {
            continue; // indented → nested key
        }
        let line = raw.trim_end();
        if line == "---" || line == "..." || line.starts_with('%') {
            continue;
        }
        if let Some(colon) = line.find(':') {
            let key = normalize_yaml_key(&line[..colon]);
            if !key.is_empty() {
                keys.push(key);
            }
        }
    }
    keys
}

/// Normalize a YAML key spelling: drop a leading flow-mapping `{` and any
/// surrounding quotes, so `"routes"` and `{routes` both read as `routes`.
fn normalize_yaml_key(raw: &str) -> String {
    let mut key = raw.trim_start();
    if let Some(rest) = key.strip_prefix('{') {
        key = rest.trim_start();
    }
    key.trim().trim_matches(['"', '\'']).to_string()
}

/// Top-level keys of a single YAML flow mapping (`{k: v, ...}`).
///
/// Bounded scanner, not a YAML parser: it tracks `{}`/`[]` depth and quoted
/// scalars, and records the token before the first `:` of each depth-1 entry.
/// Anchors, tags, `?` complex keys and comments are out of scope.
fn flow_top_level_keys(text: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut expecting_key = false;
    let mut token = String::new();

    for ch in text.chars() {
        let in_key = depth == 1 && expecting_key;
        if let Some(open) = quote {
            if in_key {
                token.push(ch);
            }
            if open == '"' && ch == '\\' && !escaped {
                escaped = true;
                continue;
            }
            if ch == open && !escaped {
                quote = None;
            }
            escaped = false;
            continue;
        }
        match ch {
            '"' | '\'' => {
                quote = Some(ch);
                if in_key {
                    token.push(ch);
                }
            }
            '{' | '[' => {
                depth += 1;
                if depth == 1 && ch == '{' {
                    expecting_key = true;
                    token.clear();
                }
            }
            '}' | ']' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    break;
                }
            }
            ',' if depth == 1 => {
                expecting_key = true;
                token.clear();
            }
            ':' if in_key => {
                let key = normalize_yaml_key(&token);
                if !key.is_empty() {
                    keys.push(key);
                }
                expecting_key = false;
                token.clear();
            }
            _ if in_key => token.push(ch),
            _ => {}
        }
    }
    keys
}

/// A root sequence starts with a `-` marker at column 0.
fn is_yaml_root_sequence(text: &str) -> bool {
    first_meaningful_line(text).is_some_and(|line| line.starts_with('-'))
}

/// A root sequence is a complete route envelope only when every element is a
/// route object (has a `from` key). Step-only sequences are partial.
fn root_sequence_is_route_objects(text: &str) -> bool {
    let mut saw_element = false;
    let mut element_has_from = false;
    let mut complete = true;

    for raw in text.lines() {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed == "---" || trimmed == "..." {
            continue;
        }
        let indented = raw.len() != raw.trim_start().len();
        if !indented {
            let Some(rest) = trimmed.strip_prefix('-') else {
                return false; // top-level line that is not a sequence marker
            };
            if saw_element && !element_has_from {
                complete = false;
            }
            saw_element = true;
            element_has_from = key_present(rest.trim_start(), "from");
            continue;
        }
        let indent = raw.len() - raw.trim_start().len();
        if indent == 2 && key_present(trimmed, "from") {
            element_has_from = true;
        }
    }
    if saw_element && !element_has_from {
        complete = false;
    }
    saw_element && complete
}

/// True when `line` is the mapping key `key` (quotes tolerated).
fn key_present(line: &str, key: &str) -> bool {
    let Some(colon) = line.find(':') else {
        return false;
    };
    normalize_yaml_key(&line[..colon]) == key
}

/// Re-root a root sequence under `routes:` by indenting every line.
fn indent_block(text: &str, spaces: usize, prefix: &str) -> String {
    let pad = " ".repeat(spaces);
    let mut out = String::from(prefix);
    out.push('\n');
    for line in text.lines() {
        if line.is_empty() {
            out.push('\n');
        } else {
            out.push_str(&pad);
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Wrap a single route object as a one-element `routes:` sequence.
fn wrap_single_route(text: &str) -> String {
    let mut out = String::from("routes:\n");
    let mut first = true;
    for line in text.lines() {
        if line.trim().is_empty() {
            out.push('\n');
            continue;
        }
        if first {
            out.push_str("  - ");
            out.push_str(line);
            first = false;
        } else {
            out.push_str("    ");
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

fn parse_normalized(text: &str, kind: DocKind) -> Result<Vec<DeclarativeRoute>, CamelError> {
    match kind {
        DocKind::Yaml => camel_dsl::parse_yaml_to_declarative(text),
        DocKind::Json => camel_dsl::parse_json_to_declarative(text),
    }
}

// ---------------------------------------------------------------------------
// Language registry and capability policy
// ---------------------------------------------------------------------------

/// Shared language registry: built once, cloned by reference for every test.
fn languages() -> &'static LangMap {
    static LANGUAGES: OnceLock<LangMap> = OnceLock::new();
    LANGUAGES.get_or_init(build_languages)
}

fn build_languages() -> LangMap {
    use camel_language_js::JsLanguage;
    use camel_language_jsonpath::JsonPathLanguage;
    use camel_language_rhai::RhaiLanguage;
    use camel_language_xpath::XPathLanguage;

    let registry = camel_core::languages_from_config(&LanguagesConfig::default());
    let mut map = registry.lock().expect("language registry lock").clone();

    map.insert("jsonpath".to_string(), Arc::new(JsonPathLanguage::new()));
    map.insert("xpath".to_string(), Arc::new(XPathLanguage::new()));
    map.insert("rhai".to_string(), Arc::new(RhaiLanguage::new()));
    let js: Arc<dyn Language> = Arc::new(JsLanguage::new());
    map.insert("js".to_string(), js.clone());
    map.insert("javascript".to_string(), js);
    map
}

/// The capability policy is the registry itself: a language's mutating support
/// is probed dynamically, never inferred from a hardcoded name list.
fn capability(languages: &LangMap, name: &str, source: &str) -> Capability {
    let Some(language) = languages.get(name) else {
        return Capability::Unresolved;
    };
    match language.create_mutating_expression(source) {
        Ok(_) => Capability::Mutating,
        Err(LanguageError::NotSupported { .. }) => Capability::NonMutating,
        Err(_) => Capability::CreationError,
    }
}

/// Recursively collect every `script:` step as `(language, source)`.
fn script_languages(steps: &[DeclarativeStep], out: &mut Vec<(String, String)>) {
    for step in steps {
        match step {
            DeclarativeStep::Script(script) => out.push((
                script.expression.language.clone(),
                script.expression.source.clone(),
            )),
            DeclarativeStep::Filter(filter) => script_languages(&filter.steps, out),
            DeclarativeStep::Choice(choice) => {
                for when in &choice.whens {
                    script_languages(&when.steps, out);
                }
                if let Some(otherwise) = &choice.otherwise {
                    script_languages(otherwise, out);
                }
            }
            DeclarativeStep::Split(split) => script_languages(&split.steps, out),
            DeclarativeStep::Throttle(throttle) => script_languages(&throttle.steps, out),
            DeclarativeStep::LoadBalance(balance) => script_languages(&balance.steps, out),
            DeclarativeStep::Multicast(multicast) => script_languages(&multicast.steps, out),
            DeclarativeStep::Loop(loop_step) => script_languages(&loop_step.steps, out),
            DeclarativeStep::IdempotentConsumer(consumer) => {
                script_languages(&consumer.steps, out);
            }
            DeclarativeStep::Cache(cache) => script_languages(&cache.on_miss, out),
            DeclarativeStep::DoTry {
                steps,
                catch,
                finally,
            } => {
                script_languages(steps, out);
                for clause in catch {
                    script_languages(&clause.steps, out);
                }
                if let Some(finally) = finally {
                    script_languages(&finally.steps, out);
                }
            }
            _ => {}
        }
    }
}

/// Collect scripts from a route's pipeline and its route-level children.
fn collect_route_scripts(route: &DeclarativeRoute, out: &mut Vec<(String, String)>) {
    if let Some(breaker) = &route.circuit_breaker {
        script_languages(&breaker.fallback, out);
    }
    let clauses = route
        .error_handler
        .as_ref()
        .and_then(|handler| handler.on_exceptions.as_ref());
    if let Some(clauses) = clauses {
        for clause in clauses {
            script_languages(&clause.steps, out);
        }
    }
    script_languages(&route.steps, out);
}

/// Classify a route document and every `script:` step it contains.
fn classify_text(text: &str, kind: DocKind, languages: &LangMap) -> Classified {
    let normalized = match select_envelope(text, kind) {
        Envelope::Ready(text) => text,
        Envelope::Partial => {
            return Classified::ParseError("not a route-producing document".to_string());
        }
        Envelope::Invalid(message) => return Classified::ParseError(message),
    };

    match parse_normalized(&normalized, kind) {
        Ok(routes) => {
            let mut collected = Vec::new();
            for route in &routes {
                collect_route_scripts(route, &mut collected);
            }
            let scripts = collected
                .into_iter()
                .map(|(language, source)| {
                    let cap = capability(languages, &language, &source);
                    (language, cap)
                })
                .collect();
            Classified::Parsed { scripts }
        }
        Err(ref err) => classify_parse_error(err),
    }
}

/// Map a parse failure to its sweep category.
fn classify_parse_error(err: &CamelError) -> Classified {
    match err {
        CamelError::RouteError(message) if message.contains("in value-source mapping") => {
            Classified::BareMap
        }
        CamelError::RouteError(message) if message.contains("wrong type for key") => {
            Classified::WrongType
        }
        other => Classified::ParseError(other.to_string()),
    }
}

// ---------------------------------------------------------------------------
// File-system collector (wired by Task 2.7)
// ---------------------------------------------------------------------------

/// Collector helpers used by the Task 2.7 repo sweep. They are exercised by
/// the fence fixture below; the whole-file walk is wired by the ledger task.
mod sweep_io {
    use std::path::PathBuf;

    use super::{DocKind, FenceIndex, Site};

    /// Repository root, anchored to this crate's manifest.
    pub(crate) fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    /// A Markdown fence block with its stable ordinal.
    #[derive(Debug, Clone)]
    pub(crate) struct Fence {
        pub(crate) ordinal: usize,
        pub(crate) kind: DocKind,
        pub(crate) text: String,
    }

    /// Extract every `yaml`/`yml`/`json` fence in document order. Ordinals are
    /// 0-based over those fences, so an excluded fence keeps the others stable.
    pub(crate) fn fences_from_markdown(md: &str) -> Vec<Fence> {
        let lines: Vec<&str> = md.lines().collect();
        let mut fences = Vec::new();
        let mut index = 0;
        while index < lines.len() {
            let trimmed = lines[index].trim_start();
            let marker = if trimmed.starts_with("```") {
                "```"
            } else if trimmed.starts_with("~~~") {
                "~~~"
            } else {
                index += 1;
                continue;
            };
            let info = trimmed[marker.len()..].trim();

            let mut cursor = index + 1;
            let mut body = String::new();
            let mut closed = false;
            while cursor < lines.len() {
                if lines[cursor].trim().starts_with(marker) {
                    closed = true;
                    break;
                }
                body.push_str(lines[cursor]);
                body.push('\n');
                cursor += 1;
            }
            if !closed {
                break; // unterminated fence: stop scanning
            }
            if let Some(kind) = doc_kind_from_info(info) {
                let ordinal = fences.len();
                fences.push(Fence {
                    ordinal,
                    kind,
                    text: normalize_fence_text(&body),
                });
            }
            index = cursor + 1;
        }
        fences
    }

    fn doc_kind_from_info(info: &str) -> Option<DocKind> {
        let token = info.split_whitespace().next()?.to_ascii_lowercase();
        match token.as_str() {
            "yaml" | "yml" => Some(DocKind::Yaml),
            "json" => Some(DocKind::Json),
            _ => None,
        }
    }

    /// Drop leading/trailing blank lines and the minimum common indentation.
    fn normalize_fence_text(body: &str) -> String {
        let trimmed_lines: Vec<&str> = body.lines().collect();
        let start = trimmed_lines
            .iter()
            .position(|line| !line.trim().is_empty())
            .unwrap_or(trimmed_lines.len());
        let end = trimmed_lines
            .iter()
            .rposition(|line| !line.trim().is_empty())
            .map_or(start, |position| position + 1);
        let selected = &trimmed_lines[start..end];

        let indent = selected
            .iter()
            .filter(|line| !line.trim().is_empty())
            .map(|line| line.len() - line.trim_start().len())
            .min()
            .unwrap_or(0);

        let mut out = String::new();
        for line in selected {
            if line.len() >= indent {
                out.push_str(&line[indent..]);
            } else {
                out.push_str(line.trim_start());
            }
            out.push('\n');
        }
        out
    }

    /// Recursively collect swept sites under the fixed roots, sorted.
    pub(crate) fn collect_sites() -> Vec<Site> {
        let root = repo_root();
        let mut sites = Vec::new();
        for relative in ["examples", "benchmarks/scenarios", "docs"] {
            walk_dir(&root.join(relative), &mut sites);
        }
        sites.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then(left.fence_index.cmp(&right.fence_index))
                .then(left.kind.cmp(&right.kind))
        });
        sites
    }

    /// Walk `dir` recursively, collecting sites. Every I/O failure is fatal
    /// and named with its path: a missing/unreadable directory, a failing
    /// directory entry, a stat failure, or an unreadable `.md` never silently
    /// truncate the sweep. `collect_sites` keeps its `Vec<Site>` signature;
    /// the panic carries the path.
    pub(crate) fn walk_dir(dir: &PathBuf, out: &mut Vec<Site>) {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(err) => panic!(
                "route sweep: cannot read directory {}: {err}",
                dir.display()
            ),
        };

        let mut paths: Vec<PathBuf> = Vec::new();
        for entry in entries {
            match entry {
                Ok(entry) => paths.push(entry.path()),
                Err(err) => panic!("route sweep: cannot read entry in {}: {err}", dir.display()),
            }
        }
        paths.sort();

        for path in paths {
            let metadata = match std::fs::metadata(&path) {
                Ok(metadata) => metadata,
                Err(err) => panic!("route sweep: cannot stat {}: {err}", path.display()),
            };
            if metadata.is_dir() {
                walk_dir(&path, out);
                continue;
            }
            match path.extension().and_then(|ext| ext.to_str()) {
                Some("yaml") | Some("yml") => out.push(Site {
                    path,
                    fence_index: FenceIndex::File,
                    kind: DocKind::Yaml,
                }),
                Some("json") => out.push(Site {
                    path,
                    fence_index: FenceIndex::File,
                    kind: DocKind::Json,
                }),
                Some("md") => {
                    let text = match std::fs::read_to_string(&path) {
                        Ok(text) => text,
                        Err(err) => panic!("route sweep: cannot read {}: {err}", path.display()),
                    };
                    for fence in fences_from_markdown(&text) {
                        out.push(Site {
                            path: path.clone(),
                            fence_index: FenceIndex::Fence(fence.ordinal),
                            kind: fence.kind,
                        });
                    }
                }
                _ => {}
            }
        }
    }

    /// Foreign Apache Camel syntax, untouched per the language-value-boundary spec.
    pub(crate) fn is_foreign_excluded(path: &std::path::Path) -> bool {
        path.components().any(|component| {
            matches!(
                component.as_os_str().to_str(),
                Some("camel-quarkus") | Some("camel-standalone")
            )
        })
    }

    /// Read the raw text of a site (whole file or a specific fence).
    pub(crate) fn read_site_text(site: &Site) -> Option<String> {
        let raw = std::fs::read_to_string(&site.path).ok()?;
        match site.fence_index {
            FenceIndex::File => Some(raw),
            FenceIndex::Fence(ordinal) => fences_from_markdown(&raw)
                .into_iter()
                .find(|fence| fence.ordinal == ordinal)
                .map(|fence| fence.text),
        }
    }
}

// ---------------------------------------------------------------------------
// Regression fixtures (Task 2.6): `sweep_flags_*` tests
// ---------------------------------------------------------------------------

fn parsed_scripts(text: &str, kind: DocKind) -> Vec<(String, Capability)> {
    match classify_text(text, kind, languages()) {
        Classified::Parsed { scripts } => scripts,
        other => panic!("expected Parsed, got {other:?}"),
    }
}

#[test]
fn sweep_flags_yaml_bare_map() {
    let yaml = r#"
routes:
  - id: r
    from: direct:start
    steps:
      - set_body: {constant: ok}
"#;
    assert_eq!(
        classify_text(yaml, DocKind::Yaml, languages()),
        Classified::BareMap
    );
}

#[test]
fn sweep_flags_json_bare_map_fence() {
    let json =
        r#"{"routes":[{"id":"r","from":"direct:x","steps":[{"set_body":{"constant":"ok"}}]}]}"#;
    assert_eq!(
        classify_text(json, DocKind::Json, languages()),
        Classified::BareMap
    );
}

#[test]
fn sweep_flags_markdown_fenced_bare_map() {
    let markdown = r#"
Some prose before the fence.

```yaml
routes:
  - id: r
    from: direct:start
    steps:
      - set_body: {constant: ok}
```

Some prose after.
"#;
    let fences = sweep_io::fences_from_markdown(markdown);
    let fence = &fences[0];
    assert_eq!(fence.ordinal, 0);
    assert_eq!(fence.kind, DocKind::Yaml);
    assert_eq!(
        classify_text(&fence.text, fence.kind, languages()),
        Classified::BareMap
    );
}

#[test]
fn sweep_flags_root_sequence_script() {
    let yaml = r#"
- id: r
  from: direct:start
  steps:
    - script:
        language: rhai
        source: |
          headers["tenant"] = "acme";
          body = body + "_processed"
"#;
    assert_eq!(
        parsed_scripts(yaml, DocKind::Yaml),
        vec![("rhai".to_string(), Capability::Mutating)]
    );
}

#[test]
fn sweep_flags_nested_script_non_mutating() {
    let yaml = r#"
routes:
  - id: r
    from: direct:start
    steps:
      - filter:
          simple: "${body} != null"
          steps:
            - script:
                language: simple
                source: "${body}"
"#;
    assert_eq!(
        parsed_scripts(yaml, DocKind::Yaml),
        vec![("simple".to_string(), Capability::NonMutating)]
    );
}

#[test]
fn sweep_flags_circuit_breaker_fallback_non_mutating() {
    let yaml = r#"
routes:
  - id: r
    from: direct:start
    steps:
      - to: direct:out
    circuit_breaker:
      failure_threshold: 1
      open_duration_ms: 1000
      fallback:
        - script:
            language: simple
            source: "${body}"
"#;
    assert_eq!(
        parsed_scripts(yaml, DocKind::Yaml),
        vec![("simple".to_string(), Capability::NonMutating)]
    );
}

#[test]
fn sweep_flags_rest_block_steps_inspected() {
    let yaml = r#"
rest:
  - host: 127.0.0.1
    port: 8080
    path: /sweep
    operations:
      - method: GET
        operation_id: sweepGet
        steps:
          - script:
              language: simple
              source: "${body}"
"#;
    assert_eq!(
        parsed_scripts(yaml, DocKind::Yaml),
        vec![("simple".to_string(), Capability::NonMutating)]
    );
}

#[test]
fn sweep_flags_mcp_block_steps_inspected() {
    let yaml = r#"
mcp:
  - server:
      name: sweep
      bind: 127.0.0.1:19100
    tools:
      - name: sweepTool
        input_schema: {type: object}
        steps:
          - script:
              language: simple
              source: "${body}"
"#;
    assert_eq!(
        parsed_scripts(yaml, DocKind::Yaml),
        vec![("simple".to_string(), Capability::NonMutating)]
    );
}

#[test]
fn sweep_flags_non_mutating_script() {
    let yaml = r#"
routes:
  - id: r
    from: direct:start
    steps:
      - script:
          language: simple
          source: "${body}"
"#;
    assert_eq!(
        parsed_scripts(yaml, DocKind::Yaml),
        vec![("simple".to_string(), Capability::NonMutating)]
    );
}

#[test]
fn sweep_flags_accepts_mutating_script() {
    let yaml = r#"
routes:
  - id: r
    from: direct:start
    steps:
      - script:
          language: rhai
          source: |
            body = "ok";
"#;
    assert_eq!(
        parsed_scripts(yaml, DocKind::Yaml),
        vec![("rhai".to_string(), Capability::Mutating)]
    );
}

#[test]
fn sweep_flags_step_only_sequences_are_partial() {
    let yaml = "- to: \"exec:build\"\n";
    let json = r#"[{"to":"exec:build"}]"#;
    assert_eq!(select_envelope(yaml, DocKind::Yaml), Envelope::Partial);
    assert_eq!(select_envelope(json, DocKind::Json), Envelope::Partial);
}

#[test]
fn sweep_flags_quoted_yaml_envelope_bare_map() {
    let yaml = "\"routes\":\n  - id: r\n    from: direct:start\n    steps:\n      - set_body: {constant: ok}\n";
    assert_eq!(
        classify_text(yaml, DocKind::Yaml, languages()),
        Classified::BareMap
    );
}

#[test]
fn sweep_flags_flow_yaml_envelope_non_mutating() {
    let yaml = "{routes: [{id: r, from: \"direct:start\", steps: [{script: {language: simple, source: \"${body}\"}}]}]}";
    assert_eq!(
        parsed_scripts(yaml, DocKind::Yaml),
        vec![("simple".to_string(), Capability::NonMutating)]
    );
}

#[test]
fn sweep_flags_missing_directory_collection_fails() {
    let base = std::env::var("TMPDIR").unwrap_or_else(|_| "/home/shared/tmp".to_string());
    let missing = std::path::PathBuf::from(base)
        .join("364-simpleset")
        .join(format!("sweep-missing-{}", std::process::id()))
        .join("does-not-exist");
    assert!(
        !missing.exists(),
        "fixture path must not exist: {}",
        missing.display()
    );

    let mut out = Vec::new();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        sweep_io::walk_dir(&missing, &mut out);
    }));
    let payload = result.expect_err("missing directory must surface a failure, not an empty Vec");
    let message = panic_message(payload.as_ref());
    assert!(
        message.contains("does-not-exist"),
        "panic must name the offending path: {message}"
    );
    assert!(
        out.is_empty(),
        "a failed walk must not return partially collected sites"
    );
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else {
        "<non-string panic payload>".to_string()
    }
}

#[test]
fn sweep_flags_flow_single_route_bare_map() {
    // Arrange
    let yaml = r#"{id: r, from: "direct:start", steps: [{set_body: {constant: ok}}]}"#;
    // Act
    let envelope = select_envelope(yaml, DocKind::Yaml);
    let classified = classify_text(yaml, DocKind::Yaml, languages());
    // Assert
    assert_eq!(envelope, Envelope::Ready(wrap_single_route(yaml)));
    assert_eq!(classified, Classified::BareMap);
}

#[test]
fn sweep_flags_flow_single_route_key_order_independent() {
    let orders = [
        r#"{from: "direct:start", id: r, steps: [{set_body: {constant: ok}}]}"#,
        r#"{steps: [{set_body: {constant: ok}}], id: r, from: "direct:start"}"#,
        r#"{"steps": [{set_body: {constant: ok}}], "from": "direct:start", "id": r}"#,
    ];
    for yaml in orders {
        assert_eq!(
            classify_text(yaml, DocKind::Yaml, languages()),
            Classified::BareMap,
            "order: {yaml}"
        );
    }
    let json = r#"{"steps":[{"set_body":{"constant":"ok"}}],"id":"r","from":"direct:start"}"#;
    assert_eq!(
        classify_text(json, DocKind::Json, languages()),
        Classified::BareMap
    );
}

#[test]
fn sweep_flags_flow_single_route_malformed_is_not_partial() {
    // Complete route with an unknown route-level key: must surface, not skip.
    let yaml = r#"{id: r, from: "direct:start", bogus: 1, steps: [{to: "direct:out"}]}"#;
    assert_eq!(
        select_envelope(yaml, DocKind::Yaml),
        Envelope::Ready(wrap_single_route(yaml))
    );
    let classified = classify_text(yaml, DocKind::Yaml, languages());
    assert!(
        matches!(&classified, Classified::ParseError(m) if m.contains("bogus")),
        "got {classified:?}"
    );
}

#[test]
fn sweep_flags_flow_nested_or_incomplete_keys_are_partial() {
    // from/steps only nested -> not a route object.
    let nested = r#"{split: {from: "x", steps: [{to: "direct:out"}]}}"#;
    // top-level steps without from -> spec says partial.
    let no_from = r#"{id: r, steps: [{to: "direct:out"}]}"#;
    assert_eq!(select_envelope(nested, DocKind::Yaml), Envelope::Partial);
    assert_eq!(select_envelope(no_from, DocKind::Yaml), Envelope::Partial);
}

#[test]
fn sweep_flags_flow_top_level_keys_extractor() {
    let text = r#"{a: 1, "b": {c: 2}, 'd': [e, {f: 3}], g: "x:y,z", h: direct:start}"#;
    assert_eq!(flow_top_level_keys(text), vec!["a", "b", "d", "g", "h"]);
}

#[test]
fn sweep_flags_malformed_source_is_creation_error() {
    let yaml = r#"
routes:
  - id: r
    from: direct:start
    steps:
      - script:
          language: rhai
          source: |
            headers["tenant"] = "acme"
            body = body + "_processed"
"#;
    let scripts = parsed_scripts(yaml, DocKind::Yaml);
    assert_eq!(
        scripts,
        vec![("rhai".to_string(), Capability::CreationError)]
    );
    assert!(
        scripts.iter().all(|(_, capability)| !matches!(
            capability,
            Capability::NonMutating | Capability::Mutating
        )),
        "malformed source must never be counted as non-mutating or mutating"
    );
}

// ---------------------------------------------------------------------------
// Task 2.7: repository sweep, fixed per-site baseline ledger and A16 acceptance
// ---------------------------------------------------------------------------

/// One row of the per-site sweep ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
struct LedgerEntry {
    /// Repo-relative path (forward slashes).
    path: String,
    /// `file` for a whole file, otherwise the 0-based fence ordinal.
    fence_index: String,
    outcome: LedgerOutcome,
}

/// Ledger classification, including the two exclusion outcomes.
#[derive(Debug, Clone, PartialEq, Eq)]
enum LedgerOutcome {
    /// The document parsed; each `script:` step and its capability.
    Parsed {
        scripts: Vec<(String, Capability)>,
    },
    BareMap,
    WrongType,
    ParseError(String),
    /// A partial fragment that is not a route-producing document.
    PartialExcluded,
    /// Foreign Apache Camel syntax, untouched per the spec.
    ForeignExcluded,
}

impl LedgerOutcome {
    fn category(&self) -> &'static str {
        match self {
            LedgerOutcome::Parsed { .. } => "Parsed",
            LedgerOutcome::BareMap => "BareMap",
            LedgerOutcome::WrongType => "WrongType",
            LedgerOutcome::ParseError(_) => "ParseError",
            LedgerOutcome::PartialExcluded => "PartialExcluded",
            LedgerOutcome::ForeignExcluded => "ForeignExcluded",
        }
    }

    /// Free-text detail for the ledger: script capabilities or the error text.
    fn detail(&self) -> String {
        match self {
            LedgerOutcome::Parsed { scripts } => scripts
                .iter()
                .map(|(language, capability)| format!("{language}:{capability:?}"))
                .collect::<Vec<_>>()
                .join(","),
            LedgerOutcome::ParseError(message) => sanitize_cell(message),
            _ => String::new(),
        }
    }
}

/// A parsed `route_sweep_baseline.tsv` row.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BaselineEntry {
    path: String,
    fence_index: String,
    reason: String,
}

/// TSV-safe text: no tabs or line breaks in a single ledger cell.
fn sanitize_cell(text: &str) -> String {
    text.replace(['\t', '\n', '\r'], " ")
}

/// The `fence_index` column spelling for a site.
fn fence_label(fence: FenceIndex) -> String {
    match fence {
        FenceIndex::File => "file".to_string(),
        FenceIndex::Fence(ordinal) => ordinal.to_string(),
    }
}

/// Repo-relative path with forward slashes.
fn relative_path(root: &std::path::Path, path: &std::path::Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Derive the document kind of a whole file from its extension.
fn kind_for_path(path: &std::path::Path) -> DocKind {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("json") => DocKind::Json,
        _ => DocKind::Yaml,
    }
}

/// Classify one collected site into a ledger outcome, keeping the two
/// exclusion outcomes distinct from a change-related parse failure.
fn ledger_outcome(site: &Site, languages: &LangMap) -> LedgerOutcome {
    if sweep_io::is_foreign_excluded(&site.path) {
        return LedgerOutcome::ForeignExcluded;
    }
    let raw = sweep_io::read_site_text(site)
        .unwrap_or_else(|| panic!("route sweep: cannot read site {}", site.path.display()));

    match select_envelope(&raw, site.kind) {
        Envelope::Ready(text) => match parse_normalized(&text, site.kind) {
            Ok(routes) => {
                let mut collected = Vec::new();
                for route in &routes {
                    collect_route_scripts(route, &mut collected);
                }
                let scripts = collected
                    .into_iter()
                    .map(|(language, source)| {
                        let capability = capability(languages, &language, &source);
                        (language, capability)
                    })
                    .collect();
                LedgerOutcome::Parsed { scripts }
            }
            Err(ref err) => match classify_parse_error(err) {
                Classified::BareMap => LedgerOutcome::BareMap,
                Classified::WrongType => LedgerOutcome::WrongType,
                Classified::ParseError(message) => LedgerOutcome::ParseError(message),
                Classified::Parsed { .. } => {
                    unreachable!("classify_parse_error never yields Parsed")
                }
            },
        },
        Envelope::Partial => LedgerOutcome::PartialExcluded,
        Envelope::Invalid(message) => LedgerOutcome::ParseError(message),
    }
}

/// Render one ledger row as four tab-separated columns.
fn ledger_line(entry: &LedgerEntry) -> String {
    format!(
        "{}\t{}\t{}\t{}",
        entry.path,
        entry.fence_index,
        entry.outcome.category(),
        entry.outcome.detail()
    )
}

/// Absolute path of the committed baseline fixture.
fn baseline_path() -> std::path::PathBuf {
    sweep_io::repo_root().join("crates/camel-test/tests/fixtures/route_sweep_baseline.tsv")
}

/// Read the committed baseline: `#` comments ignored, three TSV columns.
fn read_baseline() -> Vec<BaselineEntry> {
    let path = baseline_path();
    let text = std::fs::read_to_string(&path).unwrap_or_else(|err| {
        panic!(
            "route sweep: cannot read baseline {}: {err}",
            path.display()
        )
    });

    let mut entries = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim_end_matches('\r');
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let columns: Vec<&str> = line.split('\t').collect();
        assert_eq!(
            columns.len(),
            3,
            "route sweep: baseline line {} must have exactly 3 tab-separated columns",
            index + 1
        );
        entries.push(BaselineEntry {
            path: columns[0].to_string(),
            fence_index: columns[1].to_string(),
            reason: columns[2].to_string(),
        });
    }
    entries
}

/// Read a document as committed at `HEAD` through a read-only git object read.
fn git_show(root: &std::path::Path, repo_relative: &str) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("show")
        .arg(format!("HEAD:{repo_relative}"))
        .output()
        .unwrap_or_else(|err| {
            panic!("route sweep: cannot run git show for {repo_relative}: {err}")
        });
    assert!(
        output.status.success(),
        "route sweep: git show HEAD:{repo_relative} failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    String::from_utf8(output.stdout).unwrap_or_else(|err| {
        panic!("route sweep: git show HEAD:{repo_relative} emitted non-UTF8: {err}")
    })
}

/// Classify the committed (`HEAD`) form of a baseline entry with the same
/// `classify_text` used on the working tree.
///
/// This is a HEAD-consistency check only. It does NOT prove the failure predates
/// this change: once the change is committed, `HEAD` is post-change. The
/// pre-change identity is recorded separately (base `2b02ebde`, byte-identical;
/// bd rc-b0kv3) instead of pinning the historical base commit here, because
/// shallow CI checkouts cannot resolve it.
fn classify_head_entry(
    entry: &BaselineEntry,
    root: &std::path::Path,
    languages: &LangMap,
) -> Classified {
    let committed = git_show(root, &entry.path);
    if entry.fence_index == "file" {
        let kind = kind_for_path(std::path::Path::new(&entry.path));
        return classify_text(&committed, kind, languages);
    }
    let ordinal: usize = entry.fence_index.parse().unwrap_or_else(|_| {
        panic!(
            "route sweep: baseline {}#{} has a non-numeric, non-`file` fence index",
            entry.path, entry.fence_index
        )
    });
    let fences = sweep_io::fences_from_markdown(&committed);
    let fence = fences
        .iter()
        .find(|fence| fence.ordinal == ordinal)
        .unwrap_or_else(|| {
            panic!(
                "route sweep: baseline {}#{} fence missing at HEAD",
                entry.path, entry.fence_index
            )
        });
    classify_text(&fence.text, fence.kind, languages)
}

#[test]
fn route_examples_parse_without_new_rejection() {
    let languages = languages();
    let root = sweep_io::repo_root();
    let sites = sweep_io::collect_sites();

    let mut ledger = Vec::with_capacity(sites.len());
    for site in &sites {
        ledger.push(LedgerEntry {
            path: relative_path(&root, &site.path),
            fence_index: fence_label(site.fence_index),
            outcome: ledger_outcome(site, languages),
        });
    }

    // Persist and print the ledger before asserting: a failing test still leaves
    // the full per-site evidence on disk and in `--nocapture` output.
    let ledger_text = format!(
        "{}\n",
        ledger
            .iter()
            .map(ledger_line)
            .collect::<Vec<_>>()
            .join("\n")
    );
    let ledger_path =
        std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("route_sweep_ledger.tsv");
    if let Some(parent) = ledger_path.parent() {
        std::fs::create_dir_all(parent)
            .unwrap_or_else(|err| panic!("route sweep: cannot create {}: {err}", parent.display()));
    }
    std::fs::write(&ledger_path, &ledger_text)
        .unwrap_or_else(|err| panic!("route sweep: cannot write {}: {err}", ledger_path.display()));
    println!(
        "route sweep ledger: {} sites -> {}",
        ledger.len(),
        ledger_path.display()
    );
    print!("{ledger_text}");

    assert!(!ledger.is_empty(), "route sweep: no sites examined");
    assert_eq!(
        ledger.len(),
        sites.len(),
        "route sweep: every collected site must be recorded in the ledger"
    );

    // Change-related outcomes can never appear anywhere. A `script:` capability
    // other than `Mutating` (non-mutating, creation error, unresolved) fails too.
    for entry in &ledger {
        match &entry.outcome {
            LedgerOutcome::BareMap | LedgerOutcome::WrongType => panic!(
                "route sweep: change-related rejection at {}#{} ({:?})",
                entry.path, entry.fence_index, entry.outcome
            ),
            LedgerOutcome::Parsed { scripts } => {
                for (language, capability) in scripts {
                    assert_eq!(
                        *capability,
                        Capability::Mutating,
                        "route sweep: script language '{language}' at {}#{} is {capability:?}, not Mutating",
                        entry.path,
                        entry.fence_index
                    );
                }
            }
            _ => {}
        }
    }

    // Exclusions are explicit, recorded rows, never silently dropped.
    let partial_excluded = ledger
        .iter()
        .filter(|entry| matches!(entry.outcome, LedgerOutcome::PartialExcluded))
        .count();
    let foreign_excluded = ledger
        .iter()
        .filter(|entry| matches!(entry.outcome, LedgerOutcome::ForeignExcluded))
        .count();
    assert!(
        partial_excluded > 0,
        "route sweep: fragments must be recorded as PartialExcluded"
    );
    assert!(
        foreign_excluded > 0,
        "route sweep: foreign Camel syntax must be recorded as ForeignExcluded"
    );

    // The complete root-sequence route in `docs/src/eip/script.md` is selected,
    // parses and yields `Mutating`; it is never PartialExcluded, never baselined.
    let script_md = "docs/src/eip/script.md";
    let script_md_entries: Vec<&LedgerEntry> = ledger
        .iter()
        .filter(|entry| entry.path == script_md)
        .collect();
    assert!(
        !script_md_entries.is_empty(),
        "route sweep: {script_md} must contribute selected sites"
    );
    assert!(
        script_md_entries.iter().any(|entry| matches!(
            &entry.outcome,
            LedgerOutcome::Parsed { scripts }
                if scripts.iter().any(|(language, capability)| language == "rhai"
                    && *capability == Capability::Mutating)
        )),
        "route sweep: {script_md} root-sequence route must parse and yield a Mutating rhai script; got {script_md_entries:?}"
    );
    assert!(
        !script_md_entries
            .iter()
            .any(|entry| matches!(entry.outcome, LedgerOutcome::PartialExcluded)),
        "route sweep: {script_md} must not be PartialExcluded"
    );

    // ParseError keys must equal the baseline key set exactly, both directions.
    let actual: BTreeSet<(String, String)> = ledger
        .iter()
        .filter_map(|entry| match &entry.outcome {
            LedgerOutcome::ParseError(_) => Some((entry.path.clone(), entry.fence_index.clone())),
            _ => None,
        })
        .collect();
    let baseline = read_baseline();
    let expected: BTreeSet<(String, String)> = baseline
        .iter()
        .map(|entry| (entry.path.clone(), entry.fence_index.clone()))
        .collect();
    assert_eq!(
        actual, expected,
        "route sweep: ParseError key set must equal the baseline key set exactly"
    );

    // The complete `docs/src/eip/script.md` route is never baselined.
    assert!(
        !expected.iter().any(|(path, _)| path == script_md),
        "route sweep: {script_md} must never be a baseline entry"
    );

    // The migrated B2 site `docs/src/components/opensearch.md` fence 0 must be
    // Parsed (its pre-existing `set_header` map was repaired) and never baselined.
    let opensearch_key = (
        "docs/src/components/opensearch.md".to_string(),
        "0".to_string(),
    );
    let opensearch = ledger
        .iter()
        .find(|entry| (entry.path.clone(), entry.fence_index.clone()) == opensearch_key)
        .unwrap_or_else(|| panic!("route sweep: opensearch.md fence 0 must be selected"));
    assert!(
        matches!(opensearch.outcome, LedgerOutcome::Parsed { .. }),
        "route sweep: opensearch.md fence 0 must be Parsed; got {:?}",
        opensearch.outcome
    );
    assert!(
        !expected.contains(&opensearch_key),
        "route sweep: opensearch.md fence 0 must never be a baseline entry"
    );

    // Baseline reasons are mandatory error text, never a raw variable value.
    let actual_messages: HashMap<(String, String), String> = ledger
        .iter()
        .filter_map(|entry| match &entry.outcome {
            LedgerOutcome::ParseError(message) => Some((
                (entry.path.clone(), entry.fence_index.clone()),
                sanitize_cell(message),
            )),
            _ => None,
        })
        .collect();
    for entry in &baseline {
        let reason = entry.reason.trim();
        assert!(
            !reason.is_empty(),
            "route sweep: baseline {}#{} has an empty reason",
            entry.path,
            entry.fence_index
        );
        assert!(
            !entry.reason.contains('\t') && !entry.reason.contains('\n'),
            "route sweep: baseline reason must be one TSV-safe cell: {}#{}",
            entry.path,
            entry.fence_index
        );
        assert!(
            !entry.reason.contains('"'),
            "route sweep: baseline reason must not embed a raw string value: {}#{}",
            entry.path,
            entry.fence_index
        );
        let key = (entry.path.clone(), entry.fence_index.clone());
        let message = actual_messages.get(&key).unwrap_or_else(|| {
            panic!(
                "route sweep: baseline {}#{} has no matching ParseError",
                entry.path, entry.fence_index
            )
        });
        assert!(
            message.contains(reason),
            "route sweep: baseline reason for {}#{} must be error text from the actual failure",
            entry.path,
            entry.fence_index
        );
    }

    // HEAD-consistency check (git objects only): every baselined document is
    // still an unrelated parse failure at `HEAD`. This does not claim
    // pre-change-ness after the change is committed; the base identity
    // (`2b02ebde`, byte-identical; bd rc-b0kv3) is recorded in the task report.
    for entry in &baseline {
        let classified = classify_head_entry(entry, &root, languages);
        assert!(
            matches!(classified, Classified::ParseError(_)),
            "route sweep: baseline {}#{} must be an unrelated parse failure at HEAD, got {classified:?}",
            entry.path,
            entry.fence_index
        );
    }

    println!(
        "route sweep summary: {} sites, {} parsed, {} partial-excluded, {} foreign-excluded, {} baselined parse failures",
        ledger.len(),
        ledger
            .iter()
            .filter(|entry| matches!(entry.outcome, LedgerOutcome::Parsed { .. }))
            .count(),
        partial_excluded,
        foreign_excluded,
        baseline.len()
    );
}

#[test]
fn sweep_baseline_fence_identity_is_exact() {
    let markdown = r#"
Intro prose.

```yaml
routes:
  - id: first
    from: direct:first
    steps:
      - to: direct:out
```

Between fences.

```yaml
routes:
  - id: second
    from: direct:second
    steps:
      - to: direct:out
```
"#;

    let fences = sweep_io::fences_from_markdown(markdown);
    assert_eq!(fences.len(), 2, "two yaml fences must be collected");
    assert_eq!(
        fences.iter().map(|fence| fence.ordinal).collect::<Vec<_>>(),
        vec![0, 1],
        "fence ordinals must be distinct 0-based identifiers"
    );

    // The collector keys fences by path + ordinal, so two fences in one document
    // never collapse into one another.
    let root = std::path::Path::new("/repo");
    let path = root.join("doc.md");
    let keys: BTreeSet<(String, String)> = fences
        .iter()
        .map(|fence| {
            (
                relative_path(root, &path),
                fence_label(FenceIndex::Fence(fence.ordinal)),
            )
        })
        .collect();
    assert_eq!(
        keys,
        BTreeSet::from([
            ("doc.md".to_string(), "0".to_string()),
            ("doc.md".to_string(), "1".to_string()),
        ])
    );
    assert_ne!(fences[0].text, fences[1].text);
}

// ---------------------------------------------------------------------------
// Task 2.7 supplemental guard: baseline entries must not mask change-owned steps
// ---------------------------------------------------------------------------

/// Baseline entries exempt from the "at least one failing isolated step"
/// assertion.
///
/// These documents cannot yield a locally failing step because their failure is
/// at the envelope level: `docs/src/cli/job.md` fence 0 is a job document,
/// valid per the `camel job` grammar but foreign to the route-DSL lens the
/// sweep applies. The list is explicit and fixed — there is no blanket "skip
/// on error" waiver. It is documented in
/// `crates/camel-test/tests/fixtures/route_sweep_baseline.tsv`.
fn key_only_baseline_keys() -> Vec<(String, String)> {
    [("docs/src/cli/job.md", "0")]
        .into_iter()
        .map(|(path, fence)| (path.to_string(), fence.to_string()))
        .collect()
}

/// Parse a document as a generic YAML/JSON value (`noyalib` is the workspace
/// YAML shim; JSON uses `serde_json`). No hand-rolled parser.
fn parse_generic_value(text: &str, kind: DocKind) -> Result<serde_json::Value, String> {
    match kind {
        DocKind::Json => serde_json::from_str(text).map_err(|err| err.to_string()),
        DocKind::Yaml => {
            let value: serde_json::Value =
                serde_yml::from_str(text).map_err(|err| err.to_string())?;
            Ok(value)
        }
    }
}

/// Collect every step to isolate from a generic route document tree.
///
/// Traversal is context-aware, driven by the real DSL AST field names
/// (`crates/camel-dsl/src/route_ast.rs`): the envelopes (`routes`, root
/// sequence, single route object, `rest` operations, `mcp` tools/resources,
/// route-level `circuit_breaker.fallback` and
/// `error_handler.on_exceptions[].steps`) and the known parent-step child
/// pipelines (`filter.steps`, `choice.when[*].steps` / `choice.otherwise`,
/// `split`/`throttle`/`load_balance`/`multicast`/`loop`/`idempotent_consumer`
/// `.steps`, `cache.on_miss`, `do_try` steps/catch/finally).
///
/// Literal user data (`set_body.value` objects, `input_schema`, arbitrary
/// mappings) is never treated as a pipeline.
fn collect_steps(value: &serde_json::Value, out: &mut Vec<serde_json::Value>) {
    match value {
        serde_json::Value::Array(items) => {
            // Root sequence: every element is a route object.
            for route in items {
                collect_route_steps(route, out);
            }
        }
        serde_json::Value::Object(map) => {
            if let Some(routes) = map.get("routes") {
                collect_route_array(routes, out);
            }
            if let Some(rest) = map.get("rest").and_then(|value| value.as_array()) {
                for block in rest {
                    if let Some(operations) = block.get("operations").and_then(|v| v.as_array()) {
                        for operation in operations {
                            collect_pipeline(operation.get("steps"), out);
                        }
                    }
                }
            }
            if let Some(mcp) = map.get("mcp") {
                for server in servers_of(mcp) {
                    for collection in ["tools", "resources"] {
                        if let Some(items) = server.get(collection).and_then(|v| v.as_array()) {
                            for item in items {
                                collect_pipeline(item.get("steps"), out);
                            }
                        }
                    }
                }
            }
            // A single route object.
            if map.contains_key("from") && map.contains_key("steps") {
                collect_route_steps(value, out);
            }
        }
        _ => {}
    }
}

/// The `mcp` envelope is a sequence of servers or a single server mapping.
fn servers_of(mcp: &serde_json::Value) -> Vec<&serde_json::Value> {
    match mcp {
        serde_json::Value::Array(items) => items.iter().collect(),
        serde_json::Value::Object(_) => vec![mcp],
        _ => Vec::new(),
    }
}

/// A `routes` value must be a sequence; collect each route object.
fn collect_route_array(routes: &serde_json::Value, out: &mut Vec<serde_json::Value>) {
    if let Some(routes) = routes.as_array() {
        for route in routes {
            collect_route_steps(route, out);
        }
    }
}

/// Collect a route object's pipeline and its route-level child pipelines.
fn collect_route_steps(route: &serde_json::Value, out: &mut Vec<serde_json::Value>) {
    collect_pipeline(route.get("steps"), out);
    if let Some(fallback) = route
        .get("circuit_breaker")
        .and_then(|breaker| breaker.get("fallback"))
    {
        collect_pipeline(Some(fallback), out);
    }
    if let Some(clauses) = route
        .get("error_handler")
        .and_then(|handler| handler.get("on_exceptions"))
        .and_then(|value| value.as_array())
    {
        for clause in clauses {
            collect_pipeline(clause.get("steps"), out);
        }
    }
}

/// Add every step of a pipeline, recursing into known parent-verb children.
fn collect_pipeline(pipeline: Option<&serde_json::Value>, out: &mut Vec<serde_json::Value>) {
    let Some(steps) = pipeline.and_then(|value| value.as_array()) else {
        return;
    };
    for step in steps {
        out.push(step.clone());
        collect_parent_children(step, out);
    }
}

/// Recurse into the child pipelines of a known parent step verb.
///
/// The verb key drives traversal, so a malformed parent (extra unknown field,
/// invalid metadata) still yields its nested pipelines.
fn collect_parent_children(step: &serde_json::Value, out: &mut Vec<serde_json::Value>) {
    let Some(map) = step.as_object() else {
        return;
    };
    if let Some(filter) = map.get("filter") {
        collect_pipeline(filter.get("steps"), out);
    }
    if let Some(choice) = map.get("choice") {
        if let Some(branches) = choice.get("when").and_then(|value| value.as_array()) {
            for branch in branches {
                collect_pipeline(branch.get("steps"), out);
            }
        }
        collect_pipeline(choice.get("otherwise"), out);
    }
    for verb in ["split", "throttle", "load_balance", "multicast"] {
        if let Some(config) = map.get(verb) {
            collect_pipeline(config.get("steps"), out);
        }
    }
    // `loop` is `usize` in the shorthand form, a mapping with `steps` in full form.
    if let Some(loop_config) = map.get("loop") {
        collect_pipeline(loop_config.get("steps"), out);
    }
    if let Some(consumer) = map.get("idempotent_consumer") {
        collect_pipeline(consumer.get("steps"), out);
    }
    // `cache` is a repository-name string in shorthand form, a mapping with
    // `on_miss` in full form.
    if let Some(cache) = map.get("cache") {
        collect_pipeline(cache.get("on_miss"), out);
    }
    if let Some(do_try) = map.get("do_try") {
        collect_pipeline(do_try.get("steps"), out);
        if let Some(catches) = do_try.get("catch").and_then(|value| value.as_array()) {
            for clause in catches {
                collect_pipeline(clause.get("steps"), out);
            }
        }
        if let Some(finally) = do_try.get("finally") {
            collect_pipeline(finally.get("steps"), out);
        }
    }
}

/// The change-owned verb key of a step, if any (`set_body`, `transform`, `script`).
fn owned_verb(step: &serde_json::Value) -> Option<&'static str> {
    let map = step.as_object()?;
    ["set_body", "transform", "script"]
        .into_iter()
        .find(|verb| map.contains_key(*verb))
}

/// Wrap one step in its own synthetic JSON route, so a bad sibling cannot mask it.
fn isolated_step_json(step: &serde_json::Value) -> String {
    serde_json::json!({
        "routes": [{
            "id": "sweep-iso",
            "from": "direct:sweep-iso",
            "steps": [step.clone()],
        }]
    })
    .to_string()
}

/// Classify a single step by wrapping it in its own synthetic JSON route.
fn classify_isolated_step(step: &serde_json::Value, languages: &LangMap) -> Classified {
    classify_text(&isolated_step_json(step), DocKind::Json, languages)
}

/// An isolated step "fails" when it does not parse (ParseError, BareMap,
/// WrongType). A parsed step is a pass.
fn isolated_step_fails(class: &Classified) -> bool {
    !matches!(class, Classified::Parsed { .. })
}

/// Assert one isolated step is allowed: never a change-related rejection, and
/// never a `script:` capability other than `Mutating`.
fn assert_isolated_allowed(class: &Classified, path: &str, fence: &str) {
    match class {
        Classified::BareMap | Classified::WrongType => panic!(
            "route sweep guard: baseline {path}#{fence} contains a change-owned rejection: {class:?}"
        ),
        Classified::ParseError(_) => {}
        Classified::Parsed { scripts } => {
            for (language, capability) in scripts {
                assert_eq!(
                    *capability,
                    Capability::Mutating,
                    "route sweep guard: baseline {path}#{fence} script language '{language}' is {capability:?}, not Mutating"
                );
            }
        }
    }
}

#[test]
fn baseline_entries_do_not_mask_change_owned_steps() {
    let languages = languages();
    let root = sweep_io::repo_root();
    let baseline = read_baseline();
    assert!(
        !baseline.is_empty(),
        "route sweep guard: baseline must not be empty"
    );

    let key_only = key_only_baseline_keys();
    let mut isolated_total = 0usize;

    for entry in &baseline {
        let key = (entry.path.clone(), entry.fence_index.clone());
        let is_key_only = key_only.contains(&key);

        let fence_index = if entry.fence_index == "file" {
            FenceIndex::File
        } else {
            FenceIndex::Fence(entry.fence_index.parse().unwrap_or_else(|_| {
                panic!(
                    "route sweep guard: {}#{} has a non-numeric, non-`file` fence index",
                    entry.path, entry.fence_index
                )
            }))
        };
        let site = Site {
            path: root.join(&entry.path),
            fence_index,
            kind: kind_for_path(std::path::Path::new(&entry.path)),
        };
        let raw = sweep_io::read_site_text(&site).unwrap_or_else(|| {
            panic!(
                "route sweep guard: cannot read baseline site {}#{}",
                entry.path, entry.fence_index
            )
        });

        // Every baseline entry must generic-parse; a failure here is a guard
        // error, never a silent skip.
        let value = match parse_generic_value(&raw, site.kind) {
            Ok(value) => value,
            Err(err) => panic!(
                "route sweep guard: generic parse failed for baseline {}#{}: {err}",
                entry.path, entry.fence_index
            ),
        };

        let mut steps = Vec::new();
        collect_steps(&value, &mut steps);
        if steps.is_empty() {
            assert!(
                is_key_only,
                "route sweep guard: no isolatable step in non-key-only baseline {}#{}",
                entry.path, entry.fence_index
            );
            continue;
        }

        let mut any_failure = false;
        for step in &steps {
            let class = classify_isolated_step(step, languages);
            assert_isolated_allowed(&class, &entry.path, &entry.fence_index);
            if let Some(verb) = owned_verb(step) {
                assert!(
                    matches!(class, Classified::Parsed { .. }),
                    "route sweep guard: owned `{verb}` step in baseline {}#{} must parse, got {class:?}",
                    entry.path,
                    entry.fence_index
                );
            }
            any_failure |= isolated_step_fails(&class);
            isolated_total += 1;
        }
        assert!(
            any_failure || is_key_only,
            "route sweep guard: baseline {}#{} must have at least one failing isolated step (a whole-entry failure with no local failing step would prove an envelope issue, not a step issue)",
            entry.path,
            entry.fence_index
        );
    }

    assert!(
        isolated_total > 0,
        "route sweep guard: expected to isolate at least one step from the baseline"
    );
}

#[test]
fn sweep_flags_key_only_baseline_is_job_doc_only() {
    // The fixed key-only list is exactly the job-document entry: every other
    // former key-only site was fixed and removed from the baseline, and the
    // former `exec.md` fence 0 generic-parse exemption is gone with the
    // TOML+YAML fence split.
    assert_eq!(
        key_only_baseline_keys(),
        vec![("docs/src/cli/job.md".to_string(), "0".to_string())]
    );
}

#[test]
fn sweep_flags_baseline_isolation_unmasks_bare_map() {
    let yaml = r#"
routes:
  - id: masked
    from: "direct:masked"
    steps:
      - set_header:
          title: "broken"
      - set_body:
          constant: ok
"#;

    // The whole document is a generic untagged-enum failure: the broken
    // `set_header` step masks the bare-map `set_body` behind one ParseError.
    assert!(
        matches!(
            classify_text(yaml, DocKind::Yaml, languages()),
            Classified::ParseError(_)
        ),
        "the broken header step must make the whole document a ParseError"
    );

    // Isolation must unmask the change-owned bare map.
    let value = parse_generic_value(yaml, DocKind::Yaml).expect("fixture is valid YAML");
    let mut steps = Vec::new();
    collect_steps(&value, &mut steps);
    let classes: Vec<Classified> = steps
        .iter()
        .map(|step| classify_isolated_step(step, languages()))
        .collect();
    assert!(
        classes.contains(&Classified::BareMap),
        "isolation must report BareMap; got {classes:?}"
    );
}

/// Isolate every step of `yaml` with the same collector the baseline guard uses.
fn isolated_classes(yaml: &str) -> Vec<Classified> {
    let value = parse_generic_value(yaml, DocKind::Yaml).expect("fixture is valid YAML");
    let mut steps = Vec::new();
    collect_steps(&value, &mut steps);
    steps
        .iter()
        .map(|step| classify_isolated_step(step, languages()))
        .collect()
}

#[test]
fn sweep_flags_baseline_isolation_fallback_unmasks_nonmutating_script() {
    let yaml = r#"
routes:
  - id: r
    from: "direct:x"
    steps:
      - set_header:
          title: "broken"
    circuit_breaker:
      failure_threshold: 1
      open_duration_ms: 1000
      fallback:
        - script:
            language: simple
            source: "${body}"
"#;

    // The broken header step makes the whole document a ParseError.
    assert!(
        matches!(
            classify_text(yaml, DocKind::Yaml, languages()),
            Classified::ParseError(_)
        ),
        "the broken header step must make the whole document a ParseError"
    );

    // The route-level `circuit_breaker.fallback` pipeline must be isolated and
    // the non-mutating script unmasked.
    let classes = isolated_classes(yaml);
    assert!(
        classes.iter().any(|class| matches!(
            class,
            Classified::Parsed { scripts }
                if scripts.iter().any(|(language, capability)| language == "simple"
                    && *capability == Capability::NonMutating)
        )),
        "fallback script must isolate as NonMutating; got {classes:?}"
    );
}

#[test]
fn sweep_flags_baseline_isolation_otherwise_unmasks_bare_map() {
    let yaml = r#"
routes:
  - id: r
    from: "direct:x"
    steps:
      - choice:
          when:
            - simple: "${body} == 'a'"
              steps:
                - to: "direct:a"
          otherwise:
            - set_body:
                constant: ok
          bogus: 1
"#;

    // The deliberate unknown `bogus` field makes the choice parent a ParseError.
    assert!(
        matches!(
            classify_text(yaml, DocKind::Yaml, languages()),
            Classified::ParseError(_)
        ),
        "the malformed choice parent must make the whole document a ParseError"
    );

    // The `choice.otherwise` child pipeline must be isolated and unmasked.
    let classes = isolated_classes(yaml);
    assert!(
        classes.contains(&Classified::BareMap),
        "otherwise child must isolate as BareMap; got {classes:?}"
    );
}

#[test]
fn sweep_flags_baseline_isolation_cache_unmasks_wrong_type() {
    let yaml = r#"
routes:
  - id: r
    from: "direct:x"
    steps:
      - cache:
          key: "${header.id}"
          on_miss:
            - transform:
                simple: 123
          bogus: 1
"#;

    // The deliberate unknown `bogus` field makes the cache parent a ParseError.
    assert!(
        matches!(
            classify_text(yaml, DocKind::Yaml, languages()),
            Classified::ParseError(_)
        ),
        "the malformed cache parent must make the whole document a ParseError"
    );

    // The `cache.on_miss` child pipeline must be isolated and unmasked.
    let value = parse_generic_value(yaml, DocKind::Yaml).expect("fixture is valid YAML");
    let mut steps = Vec::new();
    collect_steps(&value, &mut steps);
    let wrong = steps
        .iter()
        .find(|step| {
            matches!(
                classify_isolated_step(step, languages()),
                Classified::WrongType
            )
        })
        .unwrap_or_else(|| panic!("on_miss transform must isolate as WrongType"));
    let rendered = camel_dsl::parse_json_to_declarative(&isolated_step_json(wrong))
        .expect_err("wrong-type step must fail")
        .to_string();
    assert!(
        rendered.contains("wrong type for key `simple`"),
        "expected wrong-type text: {rendered}"
    );
    assert!(
        !rendered.contains("123"),
        "wrong-type error must not echo the raw value: {rendered}"
    );
}

#[test]
fn sweep_flags_baseline_isolation_literal_arrays_are_not_pipelines() {
    let yaml = r#"
routes:
  - id: r
    from: "direct:x"
    steps:
      - set_body:
          value:
            steps:
              - set_body:
                  constant: ok
            otherwise:
              - transform:
                  simple: 123
            on_miss:
              - script:
                  language: simple
                  source: "${body}"
            fallback:
              - set_body:
                  constant: leak
            input_schema:
              steps:
                - script:
                    language: simple
                    source: "${body}"
"#;

    // The wrapped object is a literal body, so the whole document parses.
    assert!(
        matches!(
            classify_text(yaml, DocKind::Yaml, languages()),
            Classified::Parsed { .. }
        ),
        "a wrapped object literal body must parse"
    );

    // Only the outer `set_body` step is a pipeline step. The `value:` object and
    // its `steps`/`otherwise`/`on_miss`/`fallback`/`input_schema` payloads are
    // user data, never DSL pipelines.
    let value = parse_generic_value(yaml, DocKind::Yaml).expect("fixture is valid YAML");
    let mut steps = Vec::new();
    collect_steps(&value, &mut steps);
    assert_eq!(
        steps.len(),
        1,
        "only the outer set_body step is a pipeline step; got {steps:?}"
    );
    assert!(steps[0].get("set_body").is_some());
    let classes: Vec<Classified> = steps
        .iter()
        .map(|step| classify_isolated_step(step, languages()))
        .collect();
    assert!(
        classes
            .iter()
            .all(|class| matches!(class, Classified::Parsed { .. })),
        "literal value data must never be parsed as DSL; got {classes:?}"
    );
}
