//! Lint CONTEXT.md and CONTEXT-MAP.md citation hygiene.
//!
//! Walks every `CONTEXT.md` under `crates/`, `examples/`, `benchmarks/`,
//! `platforms/` plus the root `CONTEXT-MAP.md`, masks fenced code blocks so
//! line numbers in violations stay accurate, and runs four rules over the
//! masked content: Rule A (path + anchor resolution), Rule B (symbol
//! validation via syn), Rule C (line-number sole-locator), and Rule D
//! (cross-file glossary collision detection).

use crate::Violation;
use quote::ToTokens;
use regex::Regex;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;
use walkdir::{DirEntry, WalkDir};

/// Subdirectories to scan for `CONTEXT.md` / `CONTEXT-MAP.md` files.
const SCAN_DIRS: &[&str] = &["crates", "examples", "benchmarks", "platforms"];

/// Directory names excluded from the walk AND from per-file checks.
/// Only the literal names below — dotted crate directories such as
/// `camel-component-llm` (which contain dots) are not affected.
const EXCLUDED_DIRS: &[&str] = &["target", ".worktrees", "node_modules", "archive"];

/// Hoisted markdown-link regex. `check_paths_src` is called once per
/// discovered file, so a per-call `Regex::new` would re-compile on every
/// invocation.
static LINK_RE: OnceLock<Regex> = OnceLock::new();

/// Hoisted inline-bare-path regex. See `LINK_RE` for the rationale.
static BARE_RE: OnceLock<Regex> = OnceLock::new();

/// Hoisted backtick-quoted symbol regex. See `LINK_RE` for the rationale.
static SYMBOL_RE: OnceLock<Regex> = OnceLock::new();

/// Hoisted bare-line-ref regex (Rule C). Matches `<word>.rs:L?<digits>`.
static LINE_REF_RE: OnceLock<Regex> = OnceLock::new();

/// Hoisted `**<Term>:**` regex (Rule D). Captures the inner term
/// (excluding the `**` / `:**` markers) in group 1.
static BOLD_COLON_RE: OnceLock<Regex> = OnceLock::new();

fn link_re() -> &'static Regex {
    LINK_RE.get_or_init(|| Regex::new(r"\[([^\]]*)\]\(([^)]+)\)").expect("link regex compiles")) // allow-unwrap
}

fn bare_re() -> &'static Regex {
    BARE_RE.get_or_init(|| {
        Regex::new(r"[A-Za-z0-9_]+(?:/[A-Za-z0-9_]+)+\.[A-Za-z0-9_]+")
            .expect("bare path regex compiles") // allow-unwrap
    })
}

/// Match a backtick-quoted Rust definition symbol: either
/// `(fn|struct|enum|trait)\s+<ident>` (definition) or `<Ident>::<ident>`
/// (associated method). Group 1 is the inner token (without backticks).
/// Bare `impl <Ident>` and other backtick content do not match.
fn symbol_re() -> &'static Regex {
    SYMBOL_RE.get_or_init(|| {
        Regex::new(
            r"`((?:fn|struct|enum|trait)\s+[A-Za-z_][A-Za-z0-9_]*|[A-Za-z_][A-Za-z0-9_]*::[A-Za-z_][A-Za-z0-9_]*)`",
        )
        .expect("symbol regex compiles") // allow-unwrap
    })
}

/// Match a bare line-reference of the form `<word>.rs:L?<digits>`.
/// The optional `L` covers the `L80` style; `<word>` is the file stem.
fn line_ref_re() -> &'static Regex {
    LINE_REF_RE.get_or_init(|| Regex::new(r"\w+\.rs:L?\d+").expect("line ref regex compiles")) // allow-unwrap
}

/// Match a `**<Term>:**` bold-colon glossary line at the start of a line
/// (after trimming leading whitespace). Group 1 is the inner term
/// (excluding the `**` and `:**` markers). `[^*]+` rejects nested
/// `*` to keep the pattern simple.
fn bold_colon_re() -> &'static Regex {
    BOLD_COLON_RE
        .get_or_init(|| Regex::new(r"^\*\*([^*]+):\*\*").expect("bold colon regex compiles")) // allow-unwrap
}

/// True when `byte` may continue a Rust identifier (alnum or `_`).
/// Duplicated locally so Rule B does not depend on `crate::main` helpers.
fn is_ident_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// True when `type_name` appears in `s` as an identifier-boundary match
/// (e.g. "Producer" matches "Producer" and "Producer < X >" but not
/// "LazyJmsProducer"). Operates on bytes; valid for ASCII Rust idents.
fn type_in_string(s: &str, type_name: &str) -> bool {
    if type_name.is_empty() {
        return false;
    }
    let bytes = s.as_bytes();
    let mut start = 0usize;
    while let Some(rel) = s[start..].find(type_name) {
        let abs = start + rel;
        let end = abs + type_name.len();
        let left_ok = abs == 0 || !is_ident_byte(bytes[abs - 1]);
        let right_ok = end == bytes.len() || !is_ident_byte(bytes[end]);
        if left_ok && right_ok {
            return true;
        }
        start = abs + 1;
    }
    false
}

/// Parse every `.rs` file under `src_dir` (recursively) and concatenate
/// their top-level `syn::Item` lists into a single flat `Vec`. Unreadable
/// or unparseable files are silently skipped — the citation lint is a
/// best-effort cross-reference, not a hard parser, and a `syn` parse
/// failure on a single file must not block validation of the rest.
fn parse_crate_items(src_dir: &Path) -> Vec<syn::Item> {
    let mut items = Vec::new();
    if !src_dir.is_dir() {
        return items;
    }
    for entry in WalkDir::new(src_dir)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let file = match syn::parse_file(&content) {
            Ok(f) => f,
            Err(_) => continue,
        };
        items.extend(file.items);
    }
    items
}

/// True when `type_name` is defined as a struct, enum, or trait in
/// `items`. `syn::Ident` implements `PartialEq<&str>` so the literal
/// comparison is well-defined.
fn type_exists(items: &[syn::Item], type_name: &str) -> bool {
    items.iter().any(|item| match item {
        syn::Item::Struct(s) => s.ident == type_name,
        syn::Item::Enum(e) => e.ident == type_name,
        syn::Item::Trait(t) => t.ident == type_name,
        _ => false,
    })
}

/// True when an `Item::Impl` in `items` carries `type_name` as either its
/// self-type or (non-negative) trait-type, and that impl block contains
/// an `ImplItem::Fn` whose `sig.ident` equals `method`. Returns on the
/// first match.
fn method_in_impl_for_type(items: &[syn::Item], type_name: &str, method: &str) -> bool {
    for item in items {
        let syn::Item::Impl(impl_block) = item else {
            continue;
        };
        let self_str = impl_block.self_ty.to_token_stream().to_string();
        let mut type_in_block = type_in_string(&self_str, type_name);
        if !type_in_block
            && let Some((bang, path, _for_token)) = &impl_block.trait_
            && bang.is_none()
        {
            let trait_str = path.to_token_stream().to_string();
            type_in_block = type_in_string(&trait_str, type_name);
        }
        if !type_in_block {
            continue;
        }
        let has_method = impl_block
            .items
            .iter()
            .any(|impl_item| matches!(impl_item, syn::ImplItem::Fn(f) if f.sig.ident == method));
        if has_method {
            return true;
        }
    }
    false
}

/// True when any `Item::Impl` in `items` contains an `ImplItem::Fn`
/// whose `sig.ident` equals `fn_name`. Used by Rule B so that
/// `` `fn <ident>` `` citations resolve when the function is defined as
/// a method inside an impl block (e.g. `fn poll_ready` inside
/// `impl Service<Exchange> for XxxService`) rather than as a free
/// function.
fn fn_in_any_impl(items: &[syn::Item], fn_name: &str) -> bool {
    items.iter().any(|item| {
        let syn::Item::Impl(imp) = item else {
            return false;
        };
        imp.items
            .iter()
            .any(|ii| matches!(ii, syn::ImplItem::Fn(f) if f.sig.ident == fn_name))
    })
}

/// True when `type_name` is an `Item::Enum` in `items` AND `variant` is
/// one of its variant idents. Used by Rule B so that
/// `` `<Type>::<Variant>` `` citations (e.g. `CamelError::Stopped`,
/// `LogLevel::Error`) resolve as enum variants rather than being
/// mistaken for undefined methods.
fn enum_variant_exists(items: &[syn::Item], type_name: &str, variant: &str) -> bool {
    items.iter().any(|item| {
        matches!(item, syn::Item::Enum(e)
            if e.ident == type_name
            && e.variants.iter().any(|v| v.ident == variant))
    })
}

/// True when a top-level definition of `kind` (`"fn"`/`"struct"`/
/// `"enum"`/`"trait"`) named `ident` exists in `items`. Centralizes the
/// per-kind item match so the definition-form branches can fall back
/// across item sets without duplicating the match arms.
fn definition_exists(items: &[syn::Item], kind: &str, ident: &str) -> bool {
    items.iter().any(|item| match (kind, item) {
        ("fn", syn::Item::Fn(f)) => f.sig.ident == ident,
        ("struct", syn::Item::Struct(s)) => s.ident == ident,
        ("enum", syn::Item::Enum(e)) => e.ident == ident,
        ("trait", syn::Item::Trait(t)) => t.ident == ident,
        _ => false,
    })
}

/// True when any `Item::Trait` in `items` declares a `TraitItem::Fn`
/// whose `sig.ident` equals `fn_name`. Used by Rule B so that
/// `` `fn <ident>` `` citations resolve when the function is a trait
/// method with a default body (e.g. `fn bean` declared in the
/// `StepAccumulator` trait), not a free function or impl method.
fn fn_in_any_trait(items: &[syn::Item], fn_name: &str) -> bool {
    items.iter().any(|item| {
        let syn::Item::Trait(t) = item else {
            return false;
        };
        t.items
            .iter()
            .any(|ti| matches!(ti, syn::TraitItem::Fn(f) if f.sig.ident == fn_name))
    })
}

/// True when `type_name` is an `Item::Trait` in `items` AND `method` is
/// one of its `TraitItem::Fn` signature idents. Used by Rule B so that
/// `` `<Trait>::<method>` `` citations (e.g. `Expression::evaluate`,
/// `BeanProcessor::call`) resolve when the method is declared in the
/// trait body itself, not only when an impl block provides it.
fn method_in_trait_def(items: &[syn::Item], type_name: &str, method: &str) -> bool {
    items.iter().any(|item| {
        matches!(item, syn::Item::Trait(t)
            if t.ident == type_name
            && t.items.iter().any(|ti| matches!(ti, syn::TraitItem::Fn(f) if f.sig.ident == method)))
    })
}

/// Rule B: validate every backtick-quoted Rust definition token in
/// `masked_content` against the symbol table. For `fn`/`struct`/`enum`/
/// `trait` the definition must exist in `own_items`, falling back to
/// `workspace_items` so a crate's CONTEXT.md may legitimately cite a
/// symbol defined in another workspace crate (e.g. camel-processor
/// referencing `trait IdempotentRepository` from camel-api). A
/// `fn <ident>` citation ALSO resolves when the function is defined as a
/// method inside any impl block (in either item set). For
/// `<Type>::<member>`: if the type is not present in either item set, the
/// citation is treated as external and SKIPPED; otherwise the member
/// resolves when it is an enum variant of `<Type>`, a method declared in
/// `<Type>`'s trait body, or a method in an impl block whose self-type or
/// trait-type contains the type as an identifier boundary. Bare
/// `impl <Ident>` tokens (no `::member`) are not matched.
pub fn check_symbols_src(
    masked_content: &str,
    file_path: &str,
    own_items: &[syn::Item],
    workspace_items: &[syn::Item],
) -> Vec<Violation> {
    let mut violations = Vec::new();
    let is_context_map = file_path.ends_with("CONTEXT-MAP.md");
    let search_items: &[syn::Item] = if is_context_map {
        workspace_items
    } else {
        own_items
    };

    for (idx, line) in masked_content.lines().enumerate() {
        let line_no = idx + 1;
        for cap in symbol_re().captures_iter(line) {
            let sym = cap
                .get(1)
                .expect("group 1 always present") // allow-unwrap
                .as_str()
                .trim_end_matches(|c: char| !(c.is_ascii_alphanumeric() || c == '_'));

            // Helper: push a [symbol] violation for the current token.
            let push_violation = |v: &mut Vec<Violation>| {
                v.push(Violation {
                    file: file_path.to_string(),
                    line: line_no,
                    snippet: format!("[symbol] {} -> undefined: {sym}", line.trim()),
                });
            };

            if let Some(ident) = sym.strip_prefix("fn ") {
                let ident = ident.trim();
                // FP2: `fn <ident>` may be defined as a method inside an
                // impl block rather than as a free function. It also
                // resolves when declared as a trait method (with or
                // without a default body), and across the whole workspace
                // (cross-crate refs).
                let found = definition_exists(search_items, "fn", ident)
                    || definition_exists(workspace_items, "fn", ident)
                    || fn_in_any_impl(search_items, ident)
                    || fn_in_any_impl(workspace_items, ident)
                    || fn_in_any_trait(search_items, ident)
                    || fn_in_any_trait(workspace_items, ident);
                if !found {
                    push_violation(&mut violations);
                }
            } else if let Some(ident) = sym.strip_prefix("struct ") {
                let ident = ident.trim();
                let found = definition_exists(search_items, "struct", ident)
                    || definition_exists(workspace_items, "struct", ident);
                if !found {
                    push_violation(&mut violations);
                }
            } else if let Some(ident) = sym.strip_prefix("enum ") {
                let ident = ident.trim();
                let found = definition_exists(search_items, "enum", ident)
                    || definition_exists(workspace_items, "enum", ident);
                if !found {
                    push_violation(&mut violations);
                }
            } else if let Some(ident) = sym.strip_prefix("trait ") {
                let ident = ident.trim();
                let found = definition_exists(search_items, "trait", ident)
                    || definition_exists(workspace_items, "trait", ident);
                if !found {
                    push_violation(&mut violations);
                }
            } else if let Some((type_name, method)) = sym.split_once("::") {
                let type_name = type_name.trim();
                let method = method.trim();
                // Type existence is scoped to `search_items` (the crate's
                // own items for a per-crate CONTEXT.md, the full workspace
                // for CONTEXT-MAP.md). A type that is not local to the
                // file's scope is treated as external and SKIPPED — this
                // avoids name-collision false positives where a workspace
                // crate happens to define a type with the same simple name
                // as an external type (e.g. `Body`, `Redacted`).
                if !type_exists(search_items, type_name) {
                    // External type — SKIP per spec.
                    continue;
                }
                // FP3: `<Type>::<member>` may reference an enum variant
                // (e.g. `LogLevel::Error`). The member also resolves when
                // it is a method declared in `<Type>`'s trait body (e.g.
                // `Expression::evaluate`, `BeanProcessor::call`). Both
                // checks span own + workspace so cross-crate members
                // resolve. Otherwise fall through to the impl-block
                // method search.
                let is_variant = enum_variant_exists(own_items, type_name, method)
                    || enum_variant_exists(workspace_items, type_name, method);
                let is_trait_method = method_in_trait_def(own_items, type_name, method)
                    || method_in_trait_def(workspace_items, type_name, method);
                let method_items: &[syn::Item] = if is_context_map {
                    workspace_items
                } else {
                    own_items
                };
                if !is_variant
                    && !is_trait_method
                    && !method_in_impl_for_type(method_items, type_name, method)
                {
                    push_violation(&mut violations);
                }
            }
        }
    }

    violations
}

/// Rule C: flag a bare line-number citation of the form `<file>.rs:<digits>`
/// (or `<file>.rs:L<digits>`) ONLY when it is the sole locator on the line —
/// i.e. the line has no Rule-B recognized symbol citation
/// (`` `fn <ident>` ``, `` `struct <Ident>` ``, `` `enum <Ident>` ``,
/// `` `trait <Ident>` ``, or `` `<Type>::<method>` ``) to anchor it.
/// Table rows (lines starting with `|`) are skipped. Fenced code blocks
/// are already blanked by the masked input.
pub fn check_line_refs_src(masked_content: &str, file_path: &str) -> Vec<Violation> {
    let mut violations = Vec::new();
    let line_ref = line_ref_re();
    let symbol = symbol_re();
    for (idx, line) in masked_content.lines().enumerate() {
        let line_no = idx + 1;
        // Table rows are out of scope.
        if line.trim_start().starts_with('|') {
            continue;
        }
        if !line_ref.is_match(line) {
            continue;
        }
        // If a Rule-B symbol citation is on the same line, the symbol is
        // the durable locator — the line number is a convenience and is
        // exempt from the rule.
        if symbol.is_match(line) {
            continue;
        }
        violations.push(Violation {
            file: file_path.to_string(),
            line: line_no,
            snippet: format!(
                "[line-ref] {} -> bare line ref requires a symbol citation on the same line",
                line.trim()
            ),
        });
    }
    violations
}

/// Count leading `#` characters (1..=6) at the start of `trimmed` and
/// return the heading level. Returns `None` when the line is not a
/// valid markdown heading (zero or more than six `#`s, or no `#` at all).
fn heading_level(trimmed: &str) -> Option<usize> {
    if !trimmed.starts_with('#') {
        return None;
    }
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    Some(hashes)
}

/// Extract the title text of a markdown heading line (the part after the
/// leading `#`s and any whitespace). `level` is the number of leading
/// `#`s, already validated by `heading_level`.
fn heading_title(trimmed: &str, level: usize) -> &str {
    trimmed[level..].trim_start()
}

/// Normalize a heading title for glossary-section detection:
/// lowercase, trim, collapse internal whitespace runs to a single space.
/// Punctuation and the markdown anchor alphabet are NOT applied here —
/// the spec only requires case-insensitive, trimmed, exact-match
/// comparison against the three known titles.
fn normalize_glossary_title(title: &str) -> String {
    let lowered = title.to_lowercase();
    let trimmed = lowered.trim();
    trimmed.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// True iff the normalized title is EXACTLY one of the three known
/// glossary section names. Prefix matches like `glossary conventions`
/// are deliberately rejected.
fn is_glossary_title(normalized: &str) -> bool {
    matches!(normalized, "glossary" | "key terms" | "terminology")
}

/// Normalize a glossary term's inner content (the text between the
/// `**` and `:**` markers) for collision comparison:
///   1. lowercase
///   2. trim leading/trailing whitespace
///   3. collapse internal whitespace runs to a single space
///   4. strip any trailing colon (defense in depth)
///   5. trim again (collapse step can reintroduce trailing whitespace)
fn normalize_glossary_term(inner: &str) -> String {
    let lowered = inner.to_lowercase();
    let trimmed = lowered.trim();
    let collapsed: String = trimmed.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.trim_end_matches(':').trim().to_string()
}

/// Rule D (collection half): walk the masked content and return every
/// `**<Term>:**` line that appears inside a glossary section
/// (a heading whose normalized title is exactly `glossary`, `key terms`,
/// or `terminology`). The section terminates at the next heading of the
/// same or higher level (i.e. a heading with `#` count ≤ the section's
/// opening level). Fenced code blocks are blanked by the masked input
/// so no fence handling is needed here.
///
/// Each returned tuple is `(normalized_term, raw_term)` where `raw_term`
/// is the bold-colon substring (e.g. `**Exchange:**`) — not the entire
/// line, so surrounding prose is excluded.
pub fn collect_glossary_terms(masked_content: &str) -> Vec<(String, String)> {
    let mut terms: Vec<(String, String)> = Vec::new();
    let mut section_level: Option<usize> = None;
    let bold = bold_colon_re();

    for line in masked_content.lines() {
        let trimmed = line.trim_start();
        if let Some(level) = heading_level(trimmed) {
            let title = normalize_glossary_title(heading_title(trimmed, level));
            if is_glossary_title(&title) {
                // Opening (or re-opening) a glossary section.
                section_level = Some(level);
            } else if let Some(cur) = section_level
                && level <= cur
            {
                // Same-or-higher level heading closes the section.
                section_level = None;
            }
            continue;
        }

        if section_level.is_none() {
            continue;
        }
        if let Some(cap) = bold.captures(trimmed)
            && let Some(raw_match) = cap.get(0)
            && let Some(inner_match) = cap.get(1)
        {
            let raw = raw_match.as_str().to_string();
            let normalized = normalize_glossary_term(inner_match.as_str());
            if !normalized.is_empty() {
                terms.push((normalized, raw));
            }
        }
    }

    terms
}

/// Rule D (collision half): group files by normalized glossary term and
/// emit one `[glossary-collision]` violation per file after the first
/// (canonical) owner, naming the canonical owner in the snippet. Owners
/// are sorted ascending by file path so the canonical owner is
/// deterministic.
pub fn detect_glossary_collisions(
    terms_by_file: &[(String, Vec<(String, String)>)],
) -> Vec<Violation> {
    use std::collections::BTreeMap;
    let mut by_term: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (file, terms) in terms_by_file {
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (normalized, _raw) in terms {
            if seen.insert(normalized.clone()) {
                by_term
                    .entry(normalized.clone())
                    .or_default()
                    .push(file.clone());
            }
        }
    }

    let mut violations = Vec::new();
    for (_term, mut files) in by_term {
        if files.len() < 2 {
            continue;
        }
        files.sort();
        let canonical = files[0].clone();
        for collider in files.iter().skip(1) {
            violations.push(Violation {
                file: collider.clone(),
                line: 1,
                snippet: format!(
                    "[glossary-collision] {collider} -> duplicate of canonical glossary owner {canonical}"
                ),
            });
        }
    }

    violations
}

/// Replace fenced code blocks with empty lines, preserving line count so
/// violation line numbers stay accurate. Fence delimiter lines themselves
/// are preserved; only the content lines between them are blanked.
pub fn mask_fenced_code(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut in_fence = false;
    for line in content.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if in_fence {
            out.push('\n');
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Normalize a heading or link anchor to a slug suitable for membership
/// comparison against a list of heading slugs.
///
/// Pipeline:
/// 1. lowercase
/// 2. trim
/// 3. collapse internal whitespace runs to a single hyphen
/// 4. strip leading/trailing punctuation
/// 5. drop any character that is not `[a-z0-9-]`
pub fn normalize_anchor(heading: &str) -> String {
    let lowered = heading.to_lowercase();
    let trimmed = lowered.trim();
    let collapsed = trimmed.split_whitespace().collect::<Vec<_>>().join("-");
    let stripped: String = collapsed
        .chars()
        .skip_while(|c| !c.is_ascii_alphanumeric() && *c != '-')
        .collect();
    let stripped = stripped
        .trim_end_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .to_string();
    stripped
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
        .collect()
}

/// Return true iff `anchor` (after normalization) matches any `^#{1,6}\s+`
/// heading in `target_md_path`. A missing or unreadable file is treated as
/// "no anchor" (the caller will emit an anchor violation).
pub fn anchor_exists(target_md_path: &Path, anchor: &str) -> bool {
    let content = match std::fs::read_to_string(target_md_path) {
        Ok(c) => c,
        Err(_) => return false,
    };
    let needle = normalize_anchor(anchor);
    if needle.is_empty() {
        return false;
    }
    for line in content.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with('#') {
            continue;
        }
        // Count leading hashes (1..=6 only).
        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        if hashes == 0 || hashes > 6 {
            continue;
        }
        let heading_text = trimmed[hashes..].trim_start();
        if normalize_anchor(heading_text) == needle {
            return true;
        }
    }
    false
}

/// Lexically normalize a path (resolve `.` and `..` without touching the
/// filesystem) so that a relative path with `..` segments can be compared
/// against the workspace root.
fn normalize_path(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// Determine whether `path` (joined to `context_dir`) escapes
/// `workspace_root` via `..` segments. Purely lexical — does not consult
/// the filesystem, so it correctly flags `../../../etc/fake` (which
/// doesn't exist) as well as `../../../etc/passwd` (which does).
fn escapes_workspace(path: &str, context_dir: &Path, workspace_root: &Path) -> bool {
    let joined = context_dir.join(path);
    let normalized = normalize_path(&joined);
    let ws_normalized = normalize_path(workspace_root);
    !normalized.starts_with(&ws_normalized)
}

/// Validate a single resolved target: emit `[path]` or `[anchor]` violations
/// as appropriate. Centralizes the resolution + anchor logic so both the
/// markdown-link and inline-bare-path branches share it.
fn check_target(
    target: &str,
    line_no: usize,
    line: &str,
    file_path: &str,
    context_dir: &Path,
    workspace_root: &Path,
    violations: &mut Vec<Violation>,
) {
    // Fragment-only links target the current document.
    if target.starts_with('#') {
        return;
    }
    // External schemes are not linted.
    if target.starts_with("http://")
        || target.starts_with("https://")
        || target.starts_with("mailto:")
    {
        return;
    }

    // Split path / anchor at the first `#`.
    let (path_part, anchor_opt) = match target.find('#') {
        Some(idx) => (&target[..idx], Some(&target[idx + 1..])),
        None => (target, None),
    };

    if path_part.is_empty() {
        // Nothing to resolve; if there was an anchor it'd be a self-fragment,
        // which is already handled above.
        return;
    }

    if escapes_workspace(path_part, context_dir, workspace_root) {
        violations.push(Violation {
            file: file_path.to_string(),
            line: line_no,
            snippet: format!("[path] {} -> traversal rejected: {target}", line.trim()),
        });
        return;
    }

    // Try context_dir first, then workspace_root as a fallback.
    let resolved = {
        let cand1 = context_dir.join(path_part);
        if cand1.exists() {
            Some(cand1)
        } else {
            let cand2 = workspace_root.join(path_part);
            if cand2.exists() { Some(cand2) } else { None }
        }
    };

    let Some(resolved_path) = resolved else {
        violations.push(Violation {
            file: file_path.to_string(),
            line: line_no,
            snippet: format!("[path] {} -> not found: {target}", line.trim()),
        });
        return;
    };

    if let Some(anchor) = anchor_opt
        && resolved_path.extension().and_then(|e| e.to_str()) == Some("md")
        && !anchor_exists(&resolved_path, anchor)
    {
        violations.push(Violation {
            file: file_path.to_string(),
            line: line_no,
            snippet: format!(
                "[anchor] {} -> not found: {anchor} in {path_part}",
                line.trim()
            ),
        });
    }
}

/// Render `path` as a workspace-relative string for violation output.
fn relativize(path: &Path, workspace_root: &Path) -> String {
    path.strip_prefix(workspace_root)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| path.to_string_lossy().to_string())
}

/// Replace every inline code span (`` `...` ``) in `line` with runs of
/// spaces, preserving byte length so character offsets map 1:1 to the
/// original. Byte-level replacement keeps multi-byte UTF-8 sequences
/// intact outside spans and only emits ASCII spaces inside them, so the
/// output remains valid UTF-8. Used by `check_paths_src` so that bare
/// paths written as shorthand inside backticks (e.g.
/// `` `crates/camel-api/src/lib.rs` ``) are not validated as navigable
/// references — they are prose shorthand, not links.
fn strip_inline_code_spans(line: &str) -> String {
    let bytes = line.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut in_span = false;
    for &b in bytes {
        if b == b'`' {
            in_span = !in_span;
            out.push(b' ');
        } else if in_span {
            out.push(b' ');
        } else {
            out.push(b);
        }
    }
    // SAFETY: bytes outside backtick spans are copied verbatim; bytes
    // inside spans (and the backticks themselves) become ASCII spaces,
    // so UTF-8 well-formedness is preserved.
    String::from_utf8(out).expect("backtick strip preserves utf8") // allow-unwrap
}

/// Rule A: validate every markdown link `[text](target)` and every inline
/// bare path matching `[\w/]+\.\w+` that contains a `/`. `context_dir` is
/// the parent directory of the file being linted; `workspace_root` is the
/// fallback for known-root prefixes such as `crates/...`.
///
/// Bare-path detection runs over a backtick-stripped copy of each line so
/// that paths mentioned inside inline code spans (shorthand references,
/// not navigable links) are not validated. Markdown-link targets are still
/// detected on the original line.
pub fn check_paths_src(
    masked_content: &str,
    file_path: &str,
    context_dir: &Path,
    workspace_root: &Path,
) -> Vec<Violation> {
    let mut violations = Vec::new();

    for (idx, line) in masked_content.lines().enumerate() {
        let line_no = idx + 1;
        let mut link_target_ranges: Vec<(usize, usize)> = Vec::new();

        for cap in link_re().captures_iter(line) {
            let target_match = cap.get(2).expect("group 2 always present"); // allow-unwrap
            link_target_ranges.push((target_match.start(), target_match.end()));
            check_target(
                target_match.as_str(),
                line_no,
                line,
                file_path,
                context_dir,
                workspace_root,
                &mut violations,
            );
        }

        // Bare-path scan runs over the backtick-stripped line so that
        // shorthand path references inside inline code spans are not
        // treated as navigable links. Offsets match the original line
        // because stripping preserves byte length.
        let bare_line = strip_inline_code_spans(line);
        for cap in bare_re().captures_iter(&bare_line) {
            let m = cap.get(0).expect("group 0 always present"); // allow-unwrap
            if link_target_ranges
                .iter()
                .any(|(s, e)| m.start() >= *s && m.end() <= *e)
            {
                // Already covered as a markdown-link target.
                continue;
            }
            check_target(
                m.as_str(),
                line_no,
                line,
                file_path,
                context_dir,
                workspace_root,
                &mut violations,
            );
        }
    }

    violations
}

/// `filter_entry` predicate: prune excluded directories at the
/// `WalkDir` level so the walker never descends into them. Files are
/// never excluded by this predicate (file-level filtering happens via
/// `should_skip_path`, which is the belt-and-braces fallback).
fn is_excluded_dir(entry: &DirEntry, workspace_root: &Path) -> bool {
    if !entry.file_type().is_dir() {
        return false;
    }
    // Compare against the workspace-relative name only; the absolute
    // path may contain `.worktrees` (this script can run from a worktree
    // whose path itself lives under `.worktrees/...`), which would be
    // a false positive.
    let rel = entry
        .path()
        .strip_prefix(workspace_root)
        .unwrap_or(entry.path());
    rel.components()
        .any(|c| matches!(c, Component::Normal(s) if EXCLUDED_DIRS.iter().any(|e| s == *e)))
}

/// Belt-and-braces file-level check. Mirrors `is_excluded_dir` against
/// the relative path. Mostly redundant now that the walker prunes at
/// directory level, but kept as a defense in depth.
fn should_skip_path(path: &Path, workspace_root: &Path) -> bool {
    let rel = path.strip_prefix(workspace_root).unwrap_or(path);
    rel.components()
        .any(|c| matches!(c, Component::Normal(s) if EXCLUDED_DIRS.iter().any(|e| s == *e)))
}

/// Discover context files and run every per-file rule plus the cross-file
/// glossary pass. Returns an aggregated `Vec<Violation>`.
///
/// Discovery rule: walk `crates/`, `examples/`, `benchmarks/`, `platforms/`
/// for files named `CONTEXT.md` or `CONTEXT-MAP.md`, and also include the
/// root `CONTEXT-MAP.md`. Exclude `target/`, `.worktrees/`, `node_modules/`,
/// any `archive/` directory.
pub fn lint_context_citations(workspace_root: &Path) -> Result<Vec<Violation>, String> {
    let mut context_files: Vec<PathBuf> = Vec::new();

    let ctx_map = workspace_root.join("CONTEXT-MAP.md");
    if ctx_map.is_file() {
        context_files.push(ctx_map);
    }

    for dir_name in SCAN_DIRS {
        let dir = workspace_root.join(dir_name);
        if !dir.exists() {
            continue;
        }
        for entry in WalkDir::new(&dir)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| !is_excluded_dir(e, workspace_root))
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if should_skip_path(path, workspace_root) {
                continue;
            }
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name == "CONTEXT.md" || name == "CONTEXT-MAP.md" {
                context_files.push(path.to_path_buf());
            }
        }
    }

    // Build the workspace-wide symbol table once: every crate `src/`
    // directory is parsed into a flat `Vec<syn::Item>`. The aggregated
    // list backs `CONTEXT-MAP.md` validation; per-crate lookups use the
    // `crate_items` map keyed on the crate's `src/` path.
    let mut crate_items: std::collections::HashMap<PathBuf, Vec<syn::Item>> =
        std::collections::HashMap::new();
    let mut all_workspace_items: Vec<syn::Item> = Vec::new();
    for src_dir in collect_crate_src_dirs(workspace_root) {
        let items = parse_crate_items(&src_dir);
        all_workspace_items.extend(items.iter().cloned());
        crate_items.insert(src_dir, items);
    }

    let mut violations = Vec::new();

    // Stash masked content per file so the cross-file glossary pass can
    // re-read it without re-walking the disk. The vec holds
    // `(rel_path, masked_content)` pairs in discovery order.
    let mut masked_per_file: Vec<(String, String)> = Vec::with_capacity(context_files.len());

    for path in &context_files {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
        let masked = mask_fenced_code(&content);
        let rel_path = relativize(path, workspace_root);
        let context_dir = path.parent().unwrap_or(workspace_root);

        // Rule A: paths (Task 1).
        violations.extend(check_paths_src(
            &masked,
            &rel_path,
            context_dir,
            workspace_root,
        ));

        // Rule B: symbols (Task 2). Per-crate CONTEXT.md validates against
        // its own crate's `src/` as the primary symbol table, with the
        // aggregated workspace table as a fallback so a crate may cite a
        // symbol defined in another workspace crate (e.g. camel-processor
        // referencing `trait IdempotentRepository` from camel-api).
        // CONTEXT-MAP.md validates against the workspace table directly.
        // Both item sets are borrowed from the pre-built `crate_items`
        // cache / `all_workspace_items` vec — no deep clone per file.
        let is_context_map = path.file_name().and_then(|n| n.to_str()) == Some("CONTEXT-MAP.md");
        let own_items: &[syn::Item] = if is_context_map {
            &[]
        } else {
            let src_dir = context_dir.join("src");
            crate_items.get(&src_dir).map(Vec::as_slice).unwrap_or(&[])
        };
        let workspace_items: &[syn::Item] = &all_workspace_items;
        violations.extend(check_symbols_src(
            &masked,
            &rel_path,
            own_items,
            workspace_items,
        ));

        // Rule C: bare line refs (Task 3).
        violations.extend(check_line_refs_src(&masked, &rel_path));

        masked_per_file.push((rel_path, masked));
    }

    // Rule D: cross-file glossary collisions (Task 3). Build the
    // per-file term list from the stashed masked content, then dispatch
    // to the collision detector.
    let terms_by_file: Vec<(String, Vec<(String, String)>)> = masked_per_file
        .iter()
        .map(|(rel_path, masked)| (rel_path.clone(), collect_glossary_terms(masked)))
        .collect();
    violations.extend(detect_glossary_collisions(&terms_by_file));

    Ok(violations)
}

/// Walk `SCAN_DIRS` under `workspace_root` and collect every directory
/// that contains a `src/` subdirectory (i.e. every crate). Used to build
/// the workspace symbol table once instead of re-walking per file.
fn collect_crate_src_dirs(workspace_root: &Path) -> Vec<PathBuf> {
    let mut src_dirs = Vec::new();
    for dir_name in SCAN_DIRS {
        let scan_root = workspace_root.join(dir_name);
        if !scan_root.exists() {
            continue;
        }
        for entry in WalkDir::new(&scan_root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| !is_excluded_dir(e, workspace_root))
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_dir() {
                continue;
            }
            let src = entry.path().join("src");
            if src.is_dir() {
                src_dirs.push(src);
            }
        }
    }
    src_dirs
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Build a temp workspace from `(rel_path, content)` pairs.
    /// Returns the `TempDir`; its `Drop` impl cleans up on test exit
    /// regardless of whether assertions panic.
    fn tmp_workspace(files: &[(&str, &str)]) -> TempDir {
        let dir = tempfile::Builder::new()
            .prefix("xtask-ctx-test-")
            .tempdir()
            .expect("tempdir");
        for (rel_path, content) in files {
            let full = dir.path().join(rel_path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap(); // allow-unwrap
            std::fs::write(&full, content).unwrap(); // allow-unwrap
        }
        dir
    }

    #[test]
    fn mask_fenced_code_preserves_line_count() {
        let content = "hello\n```rust\nlet a = 1;\nlet b = 2;\nlet c = 3;\n```\nworld\n";
        let masked = mask_fenced_code(content);
        let in_lines: Vec<&str> = content.lines().collect();
        let out_lines: Vec<&str> = masked.lines().collect();
        assert_eq!(
            in_lines.len(),
            out_lines.len(),
            "line count must be preserved (in={}, out={})",
            in_lines.len(),
            out_lines.len()
        );
        // Fenced content lines (indices 2, 3, 4) must be blank.
        assert!(out_lines[2].is_empty(), "fenced line 2 must be blank");
        assert!(out_lines[3].is_empty(), "fenced line 3 must be blank");
        assert!(out_lines[4].is_empty(), "fenced line 4 must be blank");
        // Surrounding lines and fence delimiters are preserved.
        assert_eq!(out_lines[0], "hello");
        assert!(out_lines[1].starts_with("```"));
        assert!(out_lines[5].starts_with("```"));
        assert_eq!(out_lines[6], "world");
    }

    #[test]
    fn check_paths_link_target_exists() {
        let dir = tmp_workspace(&[("src/config.rs", "pub fn cfg() {}\n")]);
        let v = check_paths_src(
            "[cfg](./src/config.rs)",
            "CONTEXT.md",
            dir.path(),
            dir.path(),
        );
        assert!(
            v.iter().all(|x| !x.snippet.starts_with("[path]")),
            "expected no [path] violation, got: {v:?}"
        );
    }

    #[test]
    fn check_paths_dangling_path_flagged() {
        let dir = tmp_workspace(&[]);
        let v = check_paths_src("[old](./src/old.rs)", "CONTEXT.md", dir.path(), dir.path());
        let path_violations: Vec<&Violation> = v
            .iter()
            .filter(|x| x.snippet.starts_with("[path]"))
            .collect();
        assert_eq!(
            path_violations.len(),
            1,
            "expected one [path] violation, got: {v:?}"
        );
    }

    #[test]
    fn check_paths_anchor_resolves() {
        let dir = tmp_workspace(&[("error.md", "## Not Found Variant\n")]);
        let v = check_paths_src(
            "[x](./error.md#not-found-variant)",
            "CONTEXT.md",
            dir.path(),
            dir.path(),
        );
        assert!(
            v.iter().all(|x| !x.snippet.starts_with("[anchor]")),
            "expected no [anchor] violation, got: {v:?}"
        );
    }

    #[test]
    fn check_paths_dangling_anchor_flagged() {
        let dir = tmp_workspace(&[("error.md", "## Some Other Heading\n")]);
        let v = check_paths_src(
            "[x](./error.md#removed)",
            "CONTEXT.md",
            dir.path(),
            dir.path(),
        );
        let anchor_violations: Vec<&Violation> = v
            .iter()
            .filter(|x| x.snippet.starts_with("[anchor]"))
            .collect();
        assert_eq!(
            anchor_violations.len(),
            1,
            "expected one [anchor] violation, got: {v:?}"
        );
        assert!(
            anchor_violations[0].snippet.contains("removed"),
            "violation must name the missing anchor: {:?}",
            anchor_violations[0].snippet
        );
        assert!(
            anchor_violations[0].snippet.contains("error.md"),
            "violation must name the target file: {:?}",
            anchor_violations[0].snippet
        );
    }

    #[test]
    fn check_paths_external_and_fragment_excluded() {
        let dir = tmp_workspace(&[]);
        let v = check_paths_src(
            "[a](https://ex.com/d) and [b](#section)",
            "CONTEXT.md",
            dir.path(),
            dir.path(),
        );
        assert!(
            v.is_empty(),
            "external scheme and fragment-only must be skipped, got: {v:?}"
        );
    }

    #[test]
    fn check_paths_traversal_rejected() {
        let dir = tmp_workspace(&[]);
        let v = check_paths_src(
            "[s](../../../etc/passwd)",
            "CONTEXT.md",
            dir.path(),
            dir.path(),
        );
        let path_violations: Vec<&Violation> = v
            .iter()
            .filter(|x| x.snippet.starts_with("[path]"))
            .collect();
        assert_eq!(
            path_violations.len(),
            1,
            "expected one [path] violation for traversal, got: {v:?}"
        );
    }

    #[test]
    fn check_paths_inline_bare_path_exists() {
        let dir = tmp_workspace(&[("src/config.rs", "pub fn cfg() {}\n")]);
        let v = check_paths_src(
            "see src/config.rs for details",
            "CONTEXT.md",
            dir.path(),
            dir.path(),
        );
        assert!(
            v.iter().all(|x| !x.snippet.starts_with("[path]")),
            "inline bare path that resolves must not be flagged, got: {v:?}"
        );
    }

    #[test]
    fn lint_context_citations_discovers_context_files() {
        let dangling = "[x](./nonexistent.rs)\n";
        let dir = tmp_workspace(&[
            ("crates/camel-api/CONTEXT.md", dangling),
            ("examples/CONTEXT.md", dangling),
            ("CONTEXT-MAP.md", dangling),
            ("target/CONTEXT.md", dangling),
            (".hidden/CONTEXT.md", dangling),
        ]);
        let v = lint_context_citations(dir.path()).expect("lint runs");
        let files: Vec<String> = v.iter().map(|x| x.file.clone()).collect();
        let expected: &[&str] = &[
            "crates/camel-api/CONTEXT.md",
            "examples/CONTEXT.md",
            "CONTEXT-MAP.md",
        ];
        for exp in expected {
            assert!(
                files.iter().any(|f| f.ends_with(exp)),
                "expected violation for {exp}, got files: {files:?}"
            );
        }
        for forbidden in ["target/", ".hidden"] {
            assert!(
                files.iter().all(|f| !f.contains(forbidden)),
                "must not include {forbidden}, got files: {files:?}"
            );
        }
        // File paths must be workspace-relative (not absolute).
        assert!(
            files.iter().all(|f| !f.starts_with('/')),
            "file paths must be workspace-relative, got: {files:?}"
        );
    }

    /// Parse a single-item source snippet into a one-element `Vec<syn::Item>`.
    /// Test helper — panics on parse failure, which is acceptable because the
    /// inputs are literal Rust constructs authored in the test bodies.
    fn parse_item(src: &str) -> syn::Item {
        syn::parse_str(src).expect("test item parses")
    }

    /// Filter `vs` to the `[symbol]` tagged subset.
    fn symbol_violations(vs: &[Violation]) -> Vec<&Violation> {
        vs.iter()
            .filter(|x| x.snippet.starts_with("[symbol]"))
            .collect()
    }

    #[test]
    fn check_symbols_fn_exists() {
        let own_items = vec![parse_item("fn process_exchange() {}")];
        let v = check_symbols_src("`fn process_exchange`", "CONTEXT.md", &own_items, &[]);
        let sym = symbol_violations(&v);
        assert!(sym.is_empty(), "expected no [symbol] violation, got: {v:?}");
    }

    #[test]
    fn check_symbols_struct_renamed_flagged() {
        let own_items = vec![parse_item("struct RouteDslConfig;")];
        let v = check_symbols_src("`struct RouteConfig`", "CONTEXT.md", &own_items, &[]);
        let sym = symbol_violations(&v);
        assert_eq!(sym.len(), 1, "expected one [symbol] violation, got: {v:?}");
        assert!(
            sym[0].snippet.contains("RouteConfig"),
            "violation must name RouteConfig: {:?}",
            sym[0].snippet
        );
    }

    #[test]
    fn check_symbols_trait_impl_resolves() {
        let own_items = vec![parse_item(
            "impl Service for Producer { fn poll_ready(&self) {} }",
        )];
        let v = check_symbols_src("`Producer::poll_ready`", "CONTEXT.md", &own_items, &[]);
        let sym = symbol_violations(&v);
        assert!(sym.is_empty(), "expected no [symbol] violation, got: {v:?}");
    }

    #[test]
    fn check_symbols_generic_trait_impl_resolves() {
        // Mirror the spec scenario `impl Service<Exchange> for LazyJmsProducer`
        // (with a single-type name) so the ToTokens -> "Service < Exchange >"
        // -> identifier-boundary path is exercised by a test. Producer
        // appears only as the (non-generic) self-type; the type-name match
        // therefore goes through self_ty, not trait_ty.
        let own_items = vec![parse_item(
            "impl Service<Exchange> for Producer { fn poll_ready(&self) {} }",
        )];
        let v = check_symbols_src("`Producer::poll_ready`", "CONTEXT.md", &own_items, &[]);
        let sym = symbol_violations(&v);
        assert!(
            sym.is_empty(),
            "expected no [symbol] violation (generic trait impl), got: {v:?}"
        );
    }

    #[test]
    fn check_symbols_direct_impl_resolves() {
        let own_items = vec![parse_item("impl RouteErrorHandler { fn handle(&self) {} }")];
        let v = check_symbols_src("`RouteErrorHandler::handle`", "CONTEXT.md", &own_items, &[]);
        let sym = symbol_violations(&v);
        assert!(sym.is_empty(), "expected no [symbol] violation, got: {v:?}");
    }

    #[test]
    fn check_symbols_context_map_workspace_scope() {
        let own_items: Vec<syn::Item> = Vec::new();
        let workspace_items = vec![parse_item("fn compile_declarative_route_to_canonical() {}")];
        let v = check_symbols_src(
            "`fn compile_declarative_route_to_canonical`",
            "CONTEXT-MAP.md",
            &own_items,
            &workspace_items,
        );
        let sym = symbol_violations(&v);
        assert!(
            sym.is_empty(),
            "expected no [symbol] violation (CONTEXT-MAP.md searches workspace), got: {v:?}"
        );
    }

    #[test]
    fn check_symbols_wrong_type_method_flagged() {
        let own_items = vec![
            parse_item("struct CamelContext;"),
            parse_item("impl RouteChannelService { fn poll_ready(&self) {} }"),
        ];
        let v = check_symbols_src("`CamelContext::poll_ready`", "CONTEXT.md", &own_items, &[]);
        let sym = symbol_violations(&v);
        assert_eq!(sym.len(), 1, "expected one [symbol] violation, got: {v:?}");
    }

    #[test]
    fn check_symbols_external_type_skipped() {
        let own_items: Vec<syn::Item> = Vec::new();
        let workspace_items: Vec<syn::Item> = Vec::new();
        let v = check_symbols_src(
            "`DynamicMessage::decode`",
            "CONTEXT.md",
            &own_items,
            &workspace_items,
        );
        assert!(
            v.is_empty(),
            "external type must be skipped (no violation), got: {v:?}"
        );
    }

    #[test]
    fn check_symbols_non_symbol_not_validated() {
        let own_items: Vec<syn::Item> = Vec::new();
        let v = check_symbols_src("`config.watch`", "CONTEXT.md", &own_items, &[]);
        assert!(
            v.is_empty(),
            "non-symbol backtick token must not be validated, got: {v:?}"
        );
    }

    // ----- Task 4: tightened false-positive coverage -----

    #[test]
    fn check_paths_bare_path_inside_backticks_not_flagged() {
        let dir = tmp_workspace(&[("src/config.rs", "pub fn cfg() {}\n")]);
        // A path inside an inline code span is prose shorthand, not a
        // navigable link — even a DANGLING path must not be validated.
        let v = check_paths_src(
            "see `src/does/not/exist.rs` here",
            "CONTEXT.md",
            dir.path(),
            dir.path(),
        );
        assert!(
            v.iter().all(|x| !x.snippet.starts_with("[path]")),
            "bare path inside backticks must not be flagged, got: {v:?}"
        );
    }

    #[test]
    fn check_paths_bare_path_outside_backticks_still_flagged() {
        let dir = tmp_workspace(&[("src/config.rs", "pub fn cfg() {}\n")]);
        // Control: the same dangling path OUTSIDE backticks must still be
        // flagged, proving the backtick strip did not disable Rule A.
        let v = check_paths_src(
            "see src/does/not/exist.rs here",
            "CONTEXT.md",
            dir.path(),
            dir.path(),
        );
        assert!(
            v.iter().any(|x| x.snippet.starts_with("[path]")),
            "bare path outside backticks must be flagged, got: {v:?}"
        );
    }

    #[test]
    fn check_symbols_fn_in_impl_resolves() {
        // `fn poll_ready` is defined inside an impl block, not as a free
        // function — the citation must still resolve (FP2 fix).
        let own_items = vec![parse_item(
            "impl Service<Exchange> for XxxService { fn poll_ready(&mut self) {} }",
        )];
        let v = check_symbols_src("`fn poll_ready`", "CONTEXT.md", &own_items, &[]);
        let sym = symbol_violations(&v);
        assert!(
            sym.is_empty(),
            "fn defined as impl method must resolve, got: {v:?}"
        );
    }

    #[test]
    fn check_symbols_fn_in_trait_resolves() {
        // `fn bean` is declared as a trait method with a default body, not
        // as a free function or impl method — the citation must resolve.
        let own_items = vec![parse_item(
            "trait StepAccumulator { fn bean(mut self, name: &str) -> Self { self } }",
        )];
        let v = check_symbols_src("`fn bean`", "CONTEXT.md", &own_items, &[]);
        let sym = symbol_violations(&v);
        assert!(
            sym.is_empty(),
            "fn declared as a trait method must resolve, got: {v:?}"
        );
    }

    #[test]
    fn check_symbols_trait_method_member_resolves() {
        // `<Trait>::<method>` where the method is declared in the trait
        // body (e.g. `Expression::evaluate`) must resolve.
        let own_items = vec![parse_item(
            "trait Expression { fn evaluate(&self) -> Value; }",
        )];
        let v = check_symbols_src("`Expression::evaluate`", "CONTEXT.md", &own_items, &[]);
        let sym = symbol_violations(&v);
        assert!(
            sym.is_empty(),
            "trait method member citation must resolve, got: {v:?}"
        );
    }

    #[test]
    fn check_symbols_enum_variant_resolves() {
        // `CamelError::Stopped` is an enum variant, not a method — the
        // citation must resolve (FP3 fix).
        let own_items = vec![parse_item("enum CamelError { Stopped, Failed }")];
        let v = check_symbols_src("`CamelError::Stopped`", "CONTEXT.md", &own_items, &[]);
        let sym = symbol_violations(&v);
        assert!(
            sym.is_empty(),
            "enum variant citation must resolve, got: {v:?}"
        );
    }

    #[test]
    fn check_symbols_enum_unknown_variant_flagged() {
        // Control: a member that is neither an enum variant nor an impl
        // method must still be flagged.
        let own_items = vec![parse_item("enum CamelError { Stopped, Failed }")];
        let v = check_symbols_src("`CamelError::Nope`", "CONTEXT.md", &own_items, &[]);
        let sym = symbol_violations(&v);
        assert_eq!(
            sym.len(),
            1,
            "unknown enum member must be flagged, got: {v:?}"
        );
    }

    #[test]
    fn check_symbols_fn_undefined_flagged() {
        // Control: a fn that exists in no crate/trait/impl must be flagged.
        let own_items: Vec<syn::Item> = vec![];
        let v = check_symbols_src("`fn nonexistent_fn`", "CONTEXT.md", &own_items, &[]);
        let sym = symbol_violations(&v);
        assert_eq!(sym.len(), 1, "undefined fn must be flagged, got: {v:?}");
    }

    // ----- Task 3: Rule C (line refs) + Rule D (glossary) tests -----

    /// Filter `vs` to the `[line-ref]` tagged subset.
    fn line_ref_violations(vs: &[Violation]) -> Vec<&Violation> {
        vs.iter()
            .filter(|x| x.snippet.starts_with("[line-ref]"))
            .collect()
    }

    /// Filter `vs` to the `[glossary-collision]` tagged subset.
    fn glossary_collision_violations(vs: &[Violation]) -> Vec<&Violation> {
        vs.iter()
            .filter(|x| x.snippet.starts_with("[glossary-collision]"))
            .collect()
    }

    #[test]
    fn check_line_refs_symbol_only_passes() {
        // No line number on this line — no [line-ref] violation.
        let content = " see `fn run_steps` for details ";
        let v = check_line_refs_src(content, "CONTEXT.md");
        let line_refs = line_ref_violations(&v);
        assert!(
            line_refs.is_empty(),
            "expected no [line-ref] violation, got: {v:?}"
        );
    }

    #[test]
    fn check_line_refs_bare_line_number_flagged() {
        // Bare line number with no backtick symbol → [line-ref] violation.
        let content = "see config.rs:80 for the field";
        let v = check_line_refs_src(content, "CONTEXT.md");
        let line_refs = line_ref_violations(&v);
        assert_eq!(
            line_refs.len(),
            1,
            "expected one [line-ref] violation, got: {v:?}"
        );
        assert!(
            line_refs[0].snippet.contains("config.rs:80"),
            "violation must name the bare line ref: {:?}",
            line_refs[0].snippet
        );
    }

    #[test]
    fn check_line_refs_symbol_plus_line_allowed() {
        // Symbol on the same line exempts the line number from the rule.
        let content = " see `fn foo` at config.rs:80 ";
        let v = check_line_refs_src(content, "CONTEXT.md");
        let line_refs = line_ref_violations(&v);
        assert!(
            line_refs.is_empty(),
            "symbol on same line must exempt the line ref, got: {v:?}"
        );
    }

    #[test]
    fn check_line_refs_code_block_ignored() {
        // RAW content with a fenced block containing `error.rs:42`.
        // Masking blanks the block BEFORE check_line_refs_src runs, so
        // the rule sees an empty line and must not flag it.
        let content = "before\n```rust\nlet p = error.rs:42;\n```\nafter\n";
        let masked = mask_fenced_code(content);
        let v = check_line_refs_src(&masked, "CONTEXT.md");
        let line_refs = line_ref_violations(&v);
        assert!(
            line_refs.is_empty(),
            "fenced line ref must be ignored after masking, got: {v:?}"
        );
    }

    #[test]
    fn collect_glossary_unique_term() {
        let content = "## Glossary\n\n**Exchange:** a single message exchange.\n";
        let terms = collect_glossary_terms(content);
        assert_eq!(
            terms,
            vec![("exchange".to_string(), "**Exchange:**".to_string())],
            "expected one normalized term 'exchange', got: {terms:?}"
        );
    }

    #[test]
    fn collect_glossary_prefix_heading_excluded() {
        // `## Glossary conventions` is NOT the `Glossary` heading.
        let content = "## Glossary conventions\n\n**Term:** definition.\n";
        let terms = collect_glossary_terms(content);
        assert!(
            terms.is_empty(),
            "prefix heading must not open a glossary section, got: {terms:?}"
        );
    }

    #[test]
    fn collect_glossary_section_terminates() {
        // `## Notes` (same level) closes the `## Glossary` section.
        let content = "## Glossary\n\n**Foo:** first definition.\n\n## Notes\n\n**Foo:** duplicate outside.\n";
        let terms = collect_glossary_terms(content);
        assert_eq!(
            terms.len(),
            1,
            "expected one Foo (under Glossary only), got: {terms:?}"
        );
        assert_eq!(terms[0].0, "foo");
    }

    #[test]
    fn collect_glossary_non_section_bold_ignored() {
        // Bold-colon lines WITHOUT a Glossary heading must not be captured.
        let content = "**Questions:** a question.\n\n**Outcome:** a result.\n";
        let terms = collect_glossary_terms(content);
        assert!(
            terms.is_empty(),
            "bold lines outside a glossary section must be ignored, got: {terms:?}"
        );
    }

    #[test]
    fn collect_glossary_fenced_bold_ignored() {
        // RAW content: ## Glossary then a fenced block with a fake bold
        // term, then **Real:** outside the fence. Masking blanks the
        // fenced lines so only `Real` survives.
        let content = "## Glossary\n\n```\n**FakeTerm:** fake.\n```\n\n**Real:** actual.\n";
        let masked = mask_fenced_code(content);
        let terms = collect_glossary_terms(&masked);
        assert_eq!(
            terms,
            vec![("real".to_string(), "**Real:**".to_string())],
            "fenced FakeTerm must be ignored, got: {terms:?}"
        );
    }

    #[test]
    fn detect_glossary_collision_two_files() {
        // Both files own the same normalized term.
        let terms_by_file: Vec<(String, Vec<(String, String)>)> = vec![
            (
                "crates/a/CONTEXT.md".to_string(),
                vec![(
                    "canonical route spec".to_string(),
                    "**Canonical Route Spec:**".to_string(),
                )],
            ),
            (
                "crates/b/CONTEXT.md".to_string(),
                vec![(
                    "canonical route spec".to_string(),
                    "**Canonical Route Spec:**".to_string(),
                )],
            ),
        ];
        let v = detect_glossary_collisions(&terms_by_file);
        let collisions = glossary_collision_violations(&v);
        assert_eq!(
            collisions.len(),
            1,
            "expected one [glossary-collision] violation, got: {v:?}"
        );
        // Sorted ascending: crates/a/CONTEXT.md is canonical; crates/b is the collider.
        assert_eq!(collisions[0].file, "crates/b/CONTEXT.md");
        assert!(
            collisions[0].snippet.contains("crates/a/CONTEXT.md"),
            "violation must name the canonical owner, got: {:?}",
            collisions[0].snippet
        );
    }

    #[test]
    fn detect_glossary_normalized_collision() {
        // File A's "**Exchange:**" and file B's "**exchange :**" both
        // normalize to "exchange" — a single collision.
        let terms_by_file: Vec<(String, Vec<(String, String)>)> = vec![
            (
                "a/CONTEXT.md".to_string(),
                vec![("exchange".to_string(), "**Exchange:**".to_string())],
            ),
            (
                "b/CONTEXT.md".to_string(),
                vec![("exchange".to_string(), "**exchange :**".to_string())],
            ),
        ];
        let v = detect_glossary_collisions(&terms_by_file);
        let collisions = glossary_collision_violations(&v);
        assert_eq!(
            collisions.len(),
            1,
            "expected one normalized collision, got: {v:?}"
        );
    }

    #[test]
    fn mask_fenced_code_excludes_from_all_rules() {
        // RAW content where the fenced block contains: a `struct Fake`
        // backtick token, a `config.rs:99` line ref, and a `**FakeTerm:**`
        // bold line under a `## Glossary` heading. Masking must blank
        // ALL of them so the three rules (symbols, line-refs, glossary)
        // emit zero violations.
        let content = "## Glossary\n\n```rust\n\
                       let s: struct Fake = todo!();\n\
                       // also see config.rs:99 for context\n\
                       // and a fake term **FakeTerm:** here\n\
                       ```\n";
        let masked = mask_fenced_code(content);
        let symbol_v = check_symbols_src(&masked, "CONTEXT.md", &[], &[]);
        let line_v = check_line_refs_src(&masked, "CONTEXT.md");
        let terms = collect_glossary_terms(&masked);
        assert!(
            symbol_v.is_empty(),
            "fenced symbol must be masked out, got: {symbol_v:?}"
        );
        assert!(
            line_v.is_empty(),
            "fenced line-ref must be masked out, got: {line_v:?}"
        );
        assert!(
            terms.is_empty(),
            "fenced glossary term must be masked out, got: {terms:?}"
        );
    }
}
