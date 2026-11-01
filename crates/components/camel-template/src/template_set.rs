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

/// The allowlisted `truncate` render filter: cut a string to `length` Unicode
/// scalar values, appending `end` (default `…`) only when truncation occurred.
///
/// `length` must be an integral, non-negative number; booleans, strings,
/// sequences, and maps are rejected by the kind check before MiniJinja's
/// coercing integer conversion can accept them. Non-string input is an error.
fn truncate_filter(
    value: minijinja::Value,
    length: minijinja::Value,
    end: Option<String>,
) -> Result<minijinja::Value, minijinja::Error> {
    let invalid_length = || {
        minijinja::Error::new(
            minijinja::ErrorKind::InvalidOperation,
            "truncate: length must be a non-negative integer",
        )
    };
    if length.kind() != minijinja::value::ValueKind::Number {
        return Err(invalid_length());
    }
    // Validate integrality/non-negativity in the f64 domain rather than
    // narrowing to i64: an integral length above i64::MAX (for example u64
    // 9223372036854775808) is still a valid length and must reach the no-op
    // path, not be rejected. f64 is exact for any length a real string can
    // reach, so this loses nothing observable.
    let length = f64::try_from(length).map_err(|_| invalid_length())?;
    if length < 0.0 || length.fract() != 0.0 {
        return Err(invalid_length());
    }
    let s = value.as_str().ok_or_else(|| {
        minijinja::Error::new(
            minijinja::ErrorKind::InvalidOperation,
            "truncate: value must be a string",
        )
    })?;
    if (s.chars().count() as f64) <= length {
        return Ok(minijinja::Value::from(s));
    }
    // The guard above means the length is below the char count (a small,
    // usize-representable value), so the cast cannot saturate.
    let mut truncated: String = s.chars().take(length as usize).collect();
    truncated.push_str(end.as_deref().unwrap_or("…"));
    Ok(minijinja::Value::from(truncated))
}

/// The allowlisted `dump` render filter: deterministic JSON text for the value.
///
/// On serialization failure returns an `InvalidOperation` error naming the
/// constraint. The result is ordinary text and stays subject to the active
/// autoescape mode (it is deliberately not marked safe).
fn dump_filter(value: minijinja::Value) -> Result<minijinja::Value, minijinja::Error> {
    serde_json::to_string(&value)
        .map(minijinja::Value::from)
        .map_err(|e| {
            minijinja::Error::new(
                minijinja::ErrorKind::InvalidOperation,
                format!("dump: value is not JSON-serializable: {e}"),
            )
        })
}

/// Register exactly the component's allowlisted render filters. No functions
/// or globals are added; any other filter name stays an `unknown filter` error.
fn register_allowlisted_filters(env: &mut minijinja::Environment<'static>) {
    env.add_filter("truncate", truncate_filter);
    env.add_filter("dump", dump_filter);
}

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
        register_allowlisted_filters(&mut env);

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

    /// The entry template name this set was compiled for. Used by the producer
    /// for render-failure logging (never consulted to choose what to render).
    pub(crate) fn entry_name(&self) -> &str {
        &self.entry
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
    /// Render-time failures (strict-undefined, unknown filter/function,
    /// template lookup at render time, fuel exhaustion, output limit,
    /// timeout, join error, non-UTF-8 output) are mapped to
    /// [`TemplateReloadError::Render`], keeping a camel-owned sub-phase prefix
    /// for provenance.
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
                .map_err(|e| TemplateReloadError::Render(format!("template lookup: {e}")))?;
            let mut buf = Vec::new();
            let mut writer = LimitedWriter::new(&mut buf, max_output);
            tmpl.render_captured_to(&context, &mut writer)
                .map_err(|e| TemplateReloadError::Render(format!("render: {e}")))?;
            String::from_utf8(buf)
                .map_err(|e| TemplateReloadError::Render(format!("non-utf8 output: {e}")))
        });

        match tokio::time::timeout(timeout, join).await {
            Ok(Ok(rendered)) => rendered,
            Ok(Err(join_err)) => Err(TemplateReloadError::Render(format!(
                "minijinja spawn_blocking join: {join_err}"
            ))),
            Err(_) => Err(TemplateReloadError::Render(
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
mod tests;
