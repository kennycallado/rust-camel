//! Compiled template set for the external template component (ADR-0047 Stage 2,
//! Phase 4 / Task 4.1).
//!
//! [`TemplateSet`] holds a compiled `minijinja::Environment<'static>` built once
//! from a [`ClosureSnapshot`], plus the entry template name. [`compile`] enforces
//! the ADR-0047 top-level `{% autoescape %}` wrapper contract on the entry and
//! registers strict-undefined + fuel/recursion bounds. [`render_entry`] renders
//! the pre-compiled entry via the same geometry the inline engine uses
//! (`spawn_blocking`, `tokio::time::timeout`, and `LimitedWriter`):
//! compile-once, evaluate-many — it does NOT recompile from source.
//!
//! [`compile`]: TemplateSet::compile
//! [`render_entry`]: TemplateSet::render_entry

use std::sync::Arc;

use arc_swap::ArcSwap;
use camel_language_api::MinijinjaLimitsConfig;
use camel_language_minijinja::{LimitedWriter, ResolvedLimits, validate_autoescape_wrapper};

use crate::closure::ClosureSnapshot;
use crate::config::AutoescapeMode;
use crate::error::TemplateReloadError;

/// A compiled template set: an owned `minijinja::Environment<'static>` plus the
/// entry template name.
///
/// Built once from a [`ClosureSnapshot`] by [`compile`](Self::compile) and
/// rendered many times by [`render_entry`](Self::render_entry). Held inside a
/// [`SharedTemplates`] swap cell so Phase-5 hot-reload can atomically replace
/// the whole compiled set without disturbing in-flight renders.
///
/// `env` is `Arc`-wrapped so [`render_entry`] can hand an owned clone to the
/// `spawn_blocking` closure (which must be `'static`), mirroring the inline
/// engine's `Arc<Environment<'static>>` in
/// `camel_language_minijinja::engine::MinijinjaExpression`. This is the sole
/// divergence from the task's literal `{ env: Environment<'static> }` field
/// type and is required to reuse the engine's render geometry against a
/// pre-compiled environment.
///
/// [`render_entry`]: Self::render_entry
#[allow(dead_code)] // pub(crate) struct; consumed by Phase 4/5 component lifecycle.
#[derive(Debug)]
pub(crate) struct TemplateSet {
    env: Arc<minijinja::Environment<'static>>,
    entry: String,
}

/// Atomic swap cell holding the active [`TemplateSet`]. Mirrors the gRPC
/// server's `Arc<ArcSwap<...>>` sharing pattern (server.rs:43): the outer `Arc`
/// shares the cell across endpoint/producer clones; `ArcSwap::load` snapshots
/// the current compiled set for the lifetime of a single render.
#[allow(dead_code)] // pub(crate) type alias; consumed by Phase 4/5 component lifecycle.
pub(crate) type SharedTemplates = Arc<ArcSwap<TemplateSet>>;

impl TemplateSet {
    /// An empty set, used ONLY as the [`SharedTemplates`] seed before `start()`
    /// installs a compiled set. Never rendered: [`render_entry`] on an empty
    /// set fails at template lookup (`entry == ""`).
    ///
    /// [`render_entry`]: Self::render_entry
    #[allow(dead_code)] // pub fn; consumed by Phase 4/5 component lifecycle.
    pub fn empty() -> Self {
        Self {
            env: Arc::new(minijinja::Environment::new()),
            entry: String::new(),
        }
    }

    /// Compile a [`ClosureSnapshot`] into a render-ready [`TemplateSet`].
    ///
    /// Enforces, in order:
    /// 1. The entry is present in the snapshot and valid UTF-8.
    /// 2. Pre-scan the entry source ([`contains_autoescape_statement`]). An
    ///    explicit `{% autoescape %}`/`{% endautoescape %}` statement routes
    ///    to [`validate_autoescape_wrapper`] exactly as before: pass means the
    ///    explicit block wins (compiled as-is), any failure is a `Compile`
    ///    error.
    /// 3. True absence falls back to `default_autoescape`: `Some(mode)` wraps
    ///    the entry source in `{% autoescape "<mode>" %}` …
    ///    `{% endautoescape %}` (no added whitespace); `None` keeps today's
    ///    rejection from [`validate_autoescape_wrapper`].
    /// 4. MiniJinja parse/compile of every closure entry via
    ///    `add_template_owned`, under strict-undefined + configured
    ///    fuel/recursion bounds. Only the entry may be compiled from the
    ///    wrapped source; non-entry members always register their raw snapshot
    ///    source so include targets keep their own escaping semantics.
    ///
    /// All failures map to [`TemplateReloadError::Compile`]. The entry name
    /// registered into the `Environment` equals `entry`, so a later
    /// [`render_entry`](Self::render_entry) resolves it via
    /// `env.get_template(&self.entry)`.
    #[allow(dead_code)] // pub fn; consumed by Phase 4/5 component lifecycle.
    pub fn compile(
        snapshot: &ClosureSnapshot,
        entry: &str,
        render_limits: MinijinjaLimitsConfig,
        default_autoescape: Option<AutoescapeMode>,
    ) -> Result<Self, TemplateReloadError> {
        let limits = ResolvedLimits::from_config(&render_limits);
        // The entry must be present in the closure and valid UTF-8.
        let entry_file = snapshot.entries().get(entry).ok_or_else(|| {
            TemplateReloadError::Compile(format!("entry {entry:?} not in closure snapshot"))
        })?;
        let entry_source = std::str::from_utf8(&entry_file.bytes).map_err(|e| {
            TemplateReloadError::Compile(format!("entry {entry:?} is not utf-8: {e}"))
        })?;

        // ADR-0047 §S7 + rc-6mwze Decision 2 — precedence is explicit
        // statement > configured default > reject. The config only fills TRUE
        // absence: any detected statement (even one the validator will reject)
        // goes through the validator unchanged, so a configured default can
        // never repair an invalid explicit mode or a partial block. The
        // validator therefore has exactly one call site, in the fallback arm.
        let entry_compiled_source = match default_autoescape {
            // The injected block is byte-for-byte what the operator would have
            // written; no whitespace is added around the entry source.
            Some(mode) if !contains_autoescape_statement(entry_source) => format!(
                "{{% autoescape \"{}\" %}}{}{{% endautoescape %}}",
                mode.as_str(),
                entry_source
            ),
            _ => {
                validate_autoescape_wrapper(entry_source).map_err(|e| {
                    TemplateReloadError::Compile(format!("autoescape wrapper: {e}"))
                })?;
                entry_source.to_string()
            }
        };

        let mut env = minijinja::Environment::new();
        env.set_undefined_behavior(minijinja::UndefinedBehavior::Strict);
        env.set_fuel(Some(limits.fuel));
        env.set_recursion_limit(limits.max_recursion_depth as usize);

        // Register every closure member under its snapshot name so the entry can
        // resolve `include`/`extends`/`import`/`from` targets. The entry name in
        // the Environment equals `entry`, matching the lookup in `render_entry`.
        // Only the entry uses `entry_compiled_source`; every other member
        // registers its raw snapshot source.
        for (name, file) in snapshot.entries() {
            let source = if name == entry {
                entry_compiled_source.as_str()
            } else {
                std::str::from_utf8(&file.bytes).map_err(|e| {
                    TemplateReloadError::Compile(format!("template {name:?} is not utf-8: {e}"))
                })?
            };
            env.add_template_owned(name.clone(), source.to_string())
                .map_err(|e| TemplateReloadError::Compile(format!("compile {name:?}: {e}")))?;
        }

        Ok(Self {
            env: Arc::new(env),
            entry: entry.to_string(),
        })
    }

    /// Render the already-compiled entry against `context`.
    ///
    /// Mirrors the inline engine's render geometry: a `spawn_blocking` closure
    /// runs the synchronous MiniJinja render into a [`LimitedWriter`] bounded by
    /// `render_limits.max_output_size`, wrapped in a `tokio::time::timeout` of
    /// `render_limits.execution_timeout_ms`. It renders `self.env` directly —
    /// it does NOT call `engine::render` (which recompiles from source),
    /// preserving the compile-once invariant.
    ///
    /// Render-time failures (strict-undefined, fuel exhaustion, output limit,
    /// timeout, join error) are mapped to [`TemplateReloadError::Compile`].
    #[allow(dead_code)] // pub fn; consumed by Phase 4/5 component lifecycle.
    pub async fn render_entry(
        &self,
        context: minijinja::Value,
        render_limits: ResolvedLimits,
    ) -> Result<String, TemplateReloadError> {
        let env = Arc::clone(&self.env);
        let entry = self.entry.clone();
        // `context` arrives by value; it is moved into the `'static` closure.
        let max_output = render_limits.max_output_size as u64;
        let timeout = std::time::Duration::from_millis(render_limits.execution_timeout_ms);

        let join = tokio::task::spawn_blocking(move || -> Result<String, TemplateReloadError> {
            let tmpl = env
                .get_template(&entry)
                .map_err(|e| TemplateReloadError::Compile(format!("template lookup: {e}")))?;
            let mut buf = Vec::new();
            let mut writer = LimitedWriter::new(&mut buf, max_output);
            tmpl.render_captured_to(&context, &mut writer)
                .map_err(|e| TemplateReloadError::Compile(format!("render: {e}")))?;
            String::from_utf8(buf)
                .map_err(|e| TemplateReloadError::Compile(format!("non-utf8 output: {e}")))
        });

        match tokio::time::timeout(timeout, join).await {
            Ok(Ok(rendered)) => rendered,
            Ok(Err(join_err)) => Err(TemplateReloadError::Compile(format!(
                "minijinja spawn_blocking join: {join_err}"
            ))),
            Err(_) => Err(TemplateReloadError::Compile(
                "minijinja execution timeout".to_string(),
            )),
        }
    }
}

/// Scan `source` for a MiniJinja statement block whose tag content starts with
/// `autoescape` or `endautoescape` after stripping whitespace-control markers.
///
/// Whitespace-control markers (`-` and `+`) are stripped from both ends of the
/// block's content before matching, so `{%- autoescape "html" -%}` and
/// `{%+ endautoescape +%}` are detected. The scan intentionally MAY over-count
/// (for example, an `autoescape` mention inside a `{% comment %}` block counts
/// as present). Over-counting is safe: it only skips the configured default and
/// falls through to [`validate_autoescape_wrapper`], which stays fail-closed.
/// The scan must never under-count a real statement block.
fn contains_autoescape_statement(source: &str) -> bool {
    let mut rest = source;
    while let Some(open) = rest.find("{%") {
        let after_open = &rest[open + 2..];
        let Some(close) = after_open.find("%}") else {
            return false;
        };
        let body = &after_open[..close];
        // Whitespace-control markers sit directly against `{%` and `%}`.
        let body = body.trim_start_matches(['-', '+']);
        let body = body.trim_end_matches(['-', '+']);
        let body = body.trim_start();
        if body.starts_with("autoescape") || body.starts_with("endautoescape") {
            return true;
        }
        rest = &after_open[close + 2..];
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AutoescapeMode;
    use camel_language_api::MinijinjaLimitsConfig;
    use std::collections::BTreeMap;

    /// Build a single-entry [`ClosureSnapshot`] for the compile/render tests,
    /// avoiding filesystem setup.
    fn snapshot(entry: &str, source: &str) -> ClosureSnapshot {
        ClosureSnapshot::from_single_entry(entry, source.as_bytes().to_vec())
    }

    /// A one-key context map `{ name: k }`, built without the `value!` macro
    /// (which collides with the `minijinja::value` module name in scope).
    fn ctx_name(k: &str) -> minijinja::Value {
        let mut m: BTreeMap<&str, &str> = BTreeMap::new();
        m.insert("name", k);
        minijinja::Value::from_serialize(&m)
    }

    /// An empty-but-defined context object.
    fn ctx_empty() -> minijinja::Value {
        minijinja::Value::from_serialize(BTreeMap::<&str, &str>::new())
    }

    #[tokio::test]
    async fn template_set_compile_and_render() {
        let snap = snapshot(
            "page.html",
            r#"{% autoescape "none" %}Hi {{name}}{% endautoescape %}"#,
        );
        let set = TemplateSet::compile(&snap, "page.html", MinijinjaLimitsConfig::default(), None)
            .expect("compile");
        let rendered = set
            .render_entry(ctx_name("k"), ResolvedLimits::default())
            .await
            .expect("render");
        assert_eq!(rendered, "Hi k");
    }

    #[tokio::test]
    async fn template_set_compile_strict_undefined() {
        // The entry references `nope`, which is undefined under strict-undefined
        // behaviour at RENDER time (compile succeeds — the name merely parses).
        let snap = snapshot(
            "page.html",
            r#"{% autoescape "none" %}{{nope}}{% endautoescape %}"#,
        );
        let set = TemplateSet::compile(&snap, "page.html", MinijinjaLimitsConfig::default(), None)
            .expect("compile");
        let result = set
            .render_entry(ctx_empty(), ResolvedLimits::default())
            .await;
        assert!(
            matches!(result, Err(TemplateReloadError::Compile(_))),
            "strict-undefined must surface as Compile, got: {result:?}"
        );
    }

    #[test]
    fn template_set_compile_requires_autoescape() {
        // No top-level `{% autoescape %}` block — compile must reject.
        let snap = snapshot("page.html", "Hi {{name}}");
        let result =
            TemplateSet::compile(&snap, "page.html", MinijinjaLimitsConfig::default(), None);
        assert!(
            matches!(result, Err(TemplateReloadError::Compile(_))),
            "missing top-level autoescape wrapper must surface as Compile, got: {result:?}"
        );
    }

    #[tokio::test]
    async fn default_html_escapes_lt_payload() {
        // `.txt` is deliberate: MiniJinja's name-based default callback only
        // escapes `.html`, so escaped output here proves the configured
        // `html` default actually wrapped the entry.
        let snap = snapshot("page.txt", "Hi {{ name }}");
        let set = TemplateSet::compile(
            &snap,
            "page.txt",
            MinijinjaLimitsConfig::default(),
            Some(AutoescapeMode::Html),
        )
        .expect("compile");
        let rendered = set
            .render_entry(ctx_name("<b>"), ResolvedLimits::default())
            .await
            .expect("render");
        assert!(
            rendered.contains("&lt;b&gt;"),
            "configured html default must escape the payload, got: {rendered}"
        );
        assert!(
            !rendered.contains("<b>"),
            "raw payload must not survive html escaping, got: {rendered}"
        );
    }

    #[tokio::test]
    async fn default_none_keeps_payload_verbatim() {
        // `.html` is deliberate: the name-based callback alone would escape,
        // so verbatim output proves the `none` wrapper overrode it.
        let snap = snapshot("page.html", "Hi {{ name }}");
        let set = TemplateSet::compile(
            &snap,
            "page.html",
            MinijinjaLimitsConfig::default(),
            Some(AutoescapeMode::None),
        )
        .expect("compile");
        let rendered = set
            .render_entry(ctx_name("<b>"), ResolvedLimits::default())
            .await
            .expect("render");
        assert_eq!(rendered, "Hi <b>");
    }

    #[tokio::test]
    async fn explicit_json_block_overrides_html_config() {
        let snap = snapshot(
            "page.html",
            r#"{% autoescape "json" %}{{ name }}{% endautoescape %}"#,
        );
        let set = TemplateSet::compile(
            &snap,
            "page.html",
            MinijinjaLimitsConfig::default(),
            Some(AutoescapeMode::Html),
        )
        .expect("compile");
        let rendered = set
            .render_entry(ctx_name("<b>"), ResolvedLimits::default())
            .await
            .expect("render");
        assert_eq!(
            rendered, "\"<b>\"",
            "explicit json block must win over the html config (json output \
             is a quoted literal), got: {rendered}"
        );
        assert!(
            !rendered.contains("&lt;"),
            "html config must not be applied when an explicit block wins, got: {rendered}"
        );
    }

    #[test]
    fn invalid_explicit_mode_still_rejects_with_config() {
        let snap = snapshot(
            "page.html",
            r#"{% autoescape "htm" %}{{ name }}{% endautoescape %}"#,
        );
        let result = TemplateSet::compile(
            &snap,
            "page.html",
            MinijinjaLimitsConfig::default(),
            Some(AutoescapeMode::Html),
        );
        assert!(
            matches!(result, Err(TemplateReloadError::Compile(_))),
            "the config must not repair an invalid explicit mode, got: {result:?}"
        );
    }

    #[test]
    fn whitespace_control_invalid_mode_still_rejects_with_config() {
        let snap = snapshot(
            "page.html",
            r#"{%- autoescape "htm" -%}{{ name }}{%- endautoescape -%}"#,
        );
        let result = TemplateSet::compile(
            &snap,
            "page.html",
            MinijinjaLimitsConfig::default(),
            Some(AutoescapeMode::Html),
        );
        assert!(
            matches!(result, Err(TemplateReloadError::Compile(_))),
            "the pre-scan must catch whitespace-control statement forms, got: {result:?}"
        );
    }

    #[test]
    fn stray_endautoescape_still_rejects_with_config() {
        let snap = snapshot("page.html", "{% endautoescape %}Hi");
        let result = TemplateSet::compile(
            &snap,
            "page.html",
            MinijinjaLimitsConfig::default(),
            Some(AutoescapeMode::Html),
        );
        assert!(
            matches!(result, Err(TemplateReloadError::Compile(_))),
            "a stray endautoescape must reject even with a configured default, got: {result:?}"
        );
    }

    #[tokio::test]
    async fn include_target_keeps_own_escape_block() {
        let snap = ClosureSnapshot::from_entries(vec![
            ("page.html", br#"{% include "partial.txt" %}"#),
            (
                "partial.txt",
                br#"{% autoescape "html" %}<b>{{ name }}</b>{% endautoescape %}"#,
            ),
        ]);
        let set = TemplateSet::compile(
            &snap,
            "page.html",
            MinijinjaLimitsConfig::default(),
            Some(AutoescapeMode::None),
        )
        .expect("compile");
        let rendered = set
            .render_entry(ctx_name("<b>"), ResolvedLimits::default())
            .await
            .expect("render");
        assert!(
            rendered.contains("&lt;b&gt;"),
            "escaping must come from the include target's own block, not the \
             entry's configured default, got: {rendered}"
        );
    }

    #[test]
    fn wrapped_parse_failure_still_fails_set() {
        let snap = snapshot("page.html", "{% for x in items %}no end");
        let result = TemplateSet::compile(
            &snap,
            "page.html",
            MinijinjaLimitsConfig::default(),
            Some(AutoescapeMode::Html),
        );
        assert!(
            matches!(result, Err(TemplateReloadError::Compile(_))),
            "a parse failure of the wrapped source must fail the whole set, got: {result:?}"
        );
    }

    /// Control-plane mapping: an invalid MiniJinja template is rejected
    /// at COMPILE time (not silently deferred to first render). Mirrors
    /// Apache Camel's template-not-found / malformed-input suite, which
    /// surfaces parse failures at route startup so a misconfigured route
    /// fails closed before serving any request.
    ///
    /// The unclosed `{% for %}` is a parse error: minijinja's parser
    /// requires `{% endfor %}`. Because `compile` calls
    /// `env.add_template_owned`, the parser runs there — the error
    /// surfaces as `TemplateReloadError::Compile`, the same variant
    /// `start_route` maps to a `CamelError::TemplateReload` and a
    /// `Failed` route status. A render-time-only test would miss this
    /// property because the templates never reach `render_entry`.
    #[test]
    fn template_set_compile_rejects_parse_error() {
        // Unclosed `{% for %}` block — minijinja's parser rejects at
        // `add_template_owned` time inside `compile`.
        let snap = snapshot(
            "broken.html",
            r#"{% autoescape "none" %}{% for x in items %}{{x}}{% endautoescape %}"#,
        );
        let result =
            TemplateSet::compile(&snap, "broken.html", MinijinjaLimitsConfig::default(), None);
        assert!(
            matches!(result, Err(TemplateReloadError::Compile(_))),
            "parse error must surface as TemplateReloadError::Compile (control-plane), \
             not as a render-time ProcessorError, got: {result:?}"
        );
    }

    /// F2a / G13 — happy-path multi-entry `{% include %}` resolves within a
    /// [`TemplateSet`]. Mirrors Apache Camel's nested-template suite, which
    /// proves the include/extends/import path is wired before the
    /// depth-guard (F2b) is exercised. Without this test, a regression that
    /// lost the closure-name-to-Environment binding could pass F2b by never
    /// reaching the recursive include at all.
    #[tokio::test]
    async fn template_set_renders_multi_entry_include() {
        // Two-entry closure: `page.html` (the entry) includes `partial.html`
        // (the include target). Both wrap in `{% autoescape "none" %}` so
        // the expected output is the verbatim string `HEAD[inner]TAIL` —
        // no html-entity escape and no extra whitespace.
        let snap = ClosureSnapshot::from_entries(vec![
            (
                "page.html",
                br#"{% autoescape "none" %}HEAD[{% include "partial.html" %}]TAIL{% endautoescape %}"#,
            ),
            (
                "partial.html",
                br#"{% autoescape "none" %}inner{% endautoescape %}"#,
            ),
        ]);
        let set = TemplateSet::compile(&snap, "page.html", MinijinjaLimitsConfig::default(), None)
            .expect("compile must succeed — entry + include are both valid templates");
        let rendered = set
            .render_entry(ctx_empty(), ResolvedLimits::default())
            .await
            .expect("render");
        assert_eq!(
            rendered, "HEAD[inner]TAIL",
            "include within a TemplateSet must resolve to the registered entry name; \
             a regression here would also break F2b's recursion-bomb"
        );
    }

    /// F2b / G13 — recursion bomb trips the depth guard, fail-closed. A
    /// self-including entry must NOT hang, NOT stack-overflow, and NOT
    /// panic. The `Environment::set_recursion_limit` registered at compile
    /// time is the runtime guard: every `{% include %}` increments the
    /// depth counter, and exceeding `max_recursion_depth` causes minijinja
    /// to return an error, which `render_entry` wraps as
    /// `TemplateReloadError::Compile("render: …")`.
    ///
    /// The recursion limit is tightened to 8 AT COMPILE TIME (the only place
    /// `set_recursion_limit` is honored — render_entry does not read
    /// max_recursion_depth) so the bomb is bounded shallowly and the test
    /// stays fast. The error prefix
    /// `render:` is camel-owned and stable across minijinja versions — the
    /// minijinja-internal message text is intentionally NOT pinned, matching
    /// the `producer_fails_closed_on_output_cap` discipline.
    #[tokio::test]
    async fn template_set_fails_closed_on_recursion_bomb() {
        // Self-including entry: render of `bomb.html` does
        // `{% include "bomb.html" %}`, which resolves to itself via the
        // Environment's template map (every closure member is registered by
        // name at compile time). Each include pushes the recursion counter;
        // exceeding `max_recursion_depth` aborts the render.
        let snap = ClosureSnapshot::from_single_entry(
            "bomb.html",
            br#"{% autoescape "none" %}{% include "bomb.html" %}{% endautoescape %}"#.to_vec(),
        );
        // The recursion limit is honored ONLY at compile time
        // (env.set_recursion_limit in compile, template_set.rs L105);
        // render_entry does not read max_recursion_depth. Pass a tight
        // config to compile so the bomb is bounded shallowly (8).
        let limits = MinijinjaLimitsConfig {
            max_recursion_depth: Some(8),
            ..MinijinjaLimitsConfig::default()
        };
        let set = TemplateSet::compile(&snap, "bomb.html", limits, None)
            .expect("compile must succeed — a self-include is syntactically valid");

        let result = set
            .render_entry(ctx_empty(), ResolvedLimits::default())
            .await;
        let err_msg = match &result {
            Err(TemplateReloadError::Compile(msg)) => msg.clone(),
            other => panic!(
                "recursion bomb must fail-closed as TemplateReloadError::Compile \
                 (no hang, no stack overflow, no panic), got: {other:?}"
            ),
        };
        // Pin provenance: the failure must originate from the RENDER path
        // (not template lookup, not spawn_blocking join, not timeout). The
        // `render:` prefix is camel-owned and stable across minijinja
        // versions; the minijinja-internal message ("recursion limit",
        // "stack overflow", etc.) is intentionally NOT pinned.
        assert!(
            err_msg.contains("render:"),
            "recursion-bomb rejection must originate from the render path \
             (render_entry prefix); got: {err_msg}"
        );
    }

    /// F3 / G6 — non-UTF-8 template source rejected at COMPILE
    /// (control-plane), not silently deferred to render. Mirrors Apache
    /// Camel's encoding-validation suite: a template file containing bytes
    /// that are not valid UTF-8 must fail the startup build so a
    /// misconfigured route fails closed BEFORE serving any request.
    ///
    /// `TemplateSet::compile` enforces UTF-8 on every closure entry's
    /// `bytes: Vec<u8>` via `std::str::from_utf8` (template_set.rs:94-113) —
    /// the rejection happens at compile time, mapped to
    /// `TemplateReloadError::Compile`, which `StartupBuildHandle::start`
    /// surfaces as `CamelError::TemplateReload` and a `Failed` route
    /// status. The lifecycle layer (filesystem read) only delivers raw
    /// bytes; the compile boundary is where the UTF-8 invariant lives.
    #[test]
    fn template_set_compile_rejects_non_utf8() {
        // 0xFF 0xFE is not a valid UTF-8 lead byte (valid lead bytes are
        // 0x00-0x7F, 0xC2-0xF4). `compile` runs `std::str::from_utf8` on
        // the raw entry bytes BEFORE any parsing (autoescape wrapper or
        // minijinja), so the invalid-byte position is irrelevant — the
        // whole slice is rejected at the first invalid byte. The wrapper
        // is included only so the bytes resemble a real template.
        let mut invalid = br#"{% autoescape "none" %}"#.to_vec();
        invalid.extend_from_slice(&[0xFF, 0xFE]);
        invalid.extend_from_slice(br#"{% endautoescape %}"#);
        let snap = ClosureSnapshot::from_single_entry("bad.html", invalid);
        let result =
            TemplateSet::compile(&snap, "bad.html", MinijinjaLimitsConfig::default(), None);
        assert!(
            matches!(result, Err(TemplateReloadError::Compile(_))),
            "non-UTF-8 source must be rejected at COMPILE (control-plane → \
             TemplateReloadError::Compile), NOT silently accepted or deferred \
             to render, got: {result:?}"
        );
    }
}
