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
    minijinja::Value::from(minijinja::value::Serde(&m))
}

/// An empty-but-defined context object.
fn ctx_empty() -> minijinja::Value {
    minijinja::Value::from(minijinja::value::Serde(BTreeMap::<&str, &str>::new()))
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
    let err = match result {
        Err(err @ TemplateReloadError::Render(_)) => err,
        other => panic!(
            "strict-undefined is a render-time failure and must surface as \
             TemplateReloadError::Render, got: {other:?}"
        ),
    };
    let display = err.to_string();
    assert!(
        display.starts_with("template render failed:"),
        "render-variant display must start with `template render failed:`, got: {display}"
    );
}

#[test]
fn template_set_compile_requires_autoescape() {
    // No top-level `{% autoescape %}` block — compile must reject.
    let snap = snapshot("page.html", "Hi {{name}}");
    let result = TemplateSet::compile(&snap, "page.html", MinijinjaLimitsConfig::default(), None);
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
    let result = TemplateSet::compile(&snap, "broken.html", MinijinjaLimitsConfig::default(), None);
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
/// `TemplateReloadError::Render("render: …")`.
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
        Err(TemplateReloadError::Render(msg)) => msg.clone(),
        other => panic!(
            "recursion bomb must fail-closed as TemplateReloadError::Render \
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

/// A filter is resolved at EVALUATION time, not compile time: an unknown
/// filter name parses cleanly and only fails while rendering. This pins
/// the render-phase mapping (`TemplateReloadError::Render`) for that
/// failure class.
#[tokio::test]
async fn template_set_unknown_filter_is_render_error() {
    let snap = snapshot(
        "filter.html",
        r#"{% autoescape "none" %}{{ x | nosuchfilter }}{% endautoescape %}"#,
    );
    // Compile must SUCCEED — minijinja resolves filters at evaluation
    // time, so an unknown filter is invisible to `add_template_owned`.
    let set = TemplateSet::compile(&snap, "filter.html", MinijinjaLimitsConfig::default(), None)
        .expect("compile must succeed — filter names are resolved at render time");

    let result = set
        .render_entry(ctx_empty(), ResolvedLimits::default())
        .await;
    let msg = match &result {
        Err(TemplateReloadError::Render(msg)) => msg.clone(),
        other => panic!(
            "unknown filter is a render-time failure and must surface as \
             TemplateReloadError::Render, got: {other:?}"
        ),
    };
    assert!(
        msg.contains("unknown filter"),
        "unknown-filter failure must report minijinja's `unknown filter` text, got: {msg}"
    );
}

/// The wall-clock timeout is enforced by `render_entry` (not by
/// `compile`), so a slow-but-valid loop surfaces as a render-phase
/// failure.
///
/// The test runs on tokio's paused clock (`start_paused = true`) to
/// remove the real-time race: `Timeout` polls the inner future before the
/// timer, so a render that exhausts its fuel before the 1 ms deadline is
/// observed would surface a fuel error instead. Auto-advance alone is not
/// enough here: an in-flight `spawn_blocking` keeps the runtime busy, so
/// the clock is advanced explicitly after the first poll registers the
/// timeout. That makes the deadline fire deterministically while the
/// fuel-bound render is still pending.
///
/// MiniJinja 2.24 refuses `range` spans over 100,000 elements, so the
/// work is expressed as nested 100,000-element loops rather than one
/// oversized `range`. The 1,000,000 fuel budget is deliberately finite:
/// the loop runs long enough for the 1 ms timeout to win, and the
/// (detached) blocking render still terminates promptly once fuel is
/// exhausted instead of pinning a worker thread.
#[tokio::test(start_paused = true)]
async fn template_set_execution_timeout_is_render_error() {
    let limits = ResolvedLimits {
        execution_timeout_ms: 1,
        fuel: 1_000_000,
        ..ResolvedLimits::default()
    };
    // Compile-time limits are the only place fuel is registered on the
    // Environment (`set_fuel`); raise it from the 100k default so the
    // loop does not trip fuel exhaustion before the timeout fires.
    let compile_limits = MinijinjaLimitsConfig {
        fuel: Some(limits.fuel),
        ..MinijinjaLimitsConfig::default()
    };
    let snap = snapshot(
        "slow.html",
        r#"{% autoescape "none" %}{% for i in range(0, 100000) %}{% for j in range(0, 100000) %}{% endfor %}{% endfor %}{% endautoescape %}"#,
    );
    let set = TemplateSet::compile(&snap, "slow.html", compile_limits, None)
        .expect("compile must succeed — the loop is syntactically valid");
    // `join!` polls the render first (registering the 1 ms `Sleep`) and
    // the advance second, so the paused clock jumps straight past the
    // deadline instead of waiting for the blocked runtime to auto-advance.
    let (result, ()) = tokio::join!(set.render_entry(ctx_empty(), limits), async {
        tokio::time::advance(std::time::Duration::from_millis(1)).await;
    });
    let msg = match &result {
        Err(TemplateReloadError::Render(msg)) => msg.clone(),
        other => panic!(
            "execution timeout is a render-time failure and must surface as \
             TemplateReloadError::Render, got: {other:?}"
        ),
    };
    assert!(
        msg.contains("minijinja execution timeout"),
        "timeout failure must report the camel-owned timeout text, got: {msg}"
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
    let result = TemplateSet::compile(&snap, "bad.html", MinijinjaLimitsConfig::default(), None);
    assert!(
        matches!(result, Err(TemplateReloadError::Compile(_))),
        "non-UTF-8 source must be rejected at COMPILE (control-plane → \
         TemplateReloadError::Compile), NOT silently accepted or deferred \
         to render, got: {result:?}"
    );
}

// --- 470-template-filters: allowlisted render filters (Task 1.1 red) ---

/// `truncate` must be a no-op when the input is at most `length` Unicode
/// scalar values long; no end marker is appended because no truncation
/// occurred.
#[tokio::test]
async fn truncate_shorter_than_limit_is_noop() {
    let snap = snapshot(
        "truncate.html",
        r#"{% autoescape "none" %}{{ name | truncate(10) }}{% endautoescape %}"#,
    );
    let set = TemplateSet::compile(
        &snap,
        "truncate.html",
        MinijinjaLimitsConfig::default(),
        None,
    )
    .expect("compile must succeed — the filter is resolved at render time");
    let rendered = set
        .render_entry(ctx_name("eleventy"), ResolvedLimits::default())
        .await
        .expect("render must succeed once `truncate` is registered");
    assert_eq!(
        rendered, "eleventy",
        "a value shorter than the limit must pass through untouched (no end marker)"
    );
}

/// Truncation is measured in Unicode scalar values, never bytes, so a
/// multibyte payload is cut cleanly (no partial UTF-8 sequence).
#[tokio::test]
async fn truncate_multibyte_stays_scalar_aligned() {
    let snap = snapshot(
        "truncate.html",
        r#"{% autoescape "none" %}{{ name | truncate(5) }}{% endautoescape %}"#,
    );
    let set = TemplateSet::compile(
        &snap,
        "truncate.html",
        MinijinjaLimitsConfig::default(),
        None,
    )
    .expect("compile must succeed — the filter is resolved at render time");
    let rendered = set
        .render_entry(ctx_name("cafés niños"), ResolvedLimits::default())
        .await
        .expect("render must succeed once `truncate` is registered");
    assert_eq!(
        rendered, "cafés…",
        "truncation must keep the first 5 Unicode scalar values and append \
         the default end marker, with no partial bytes"
    );
}

/// A custom end marker is appended only when truncation actually occurs.
#[tokio::test]
async fn truncate_custom_end_only_on_truncation() {
    let snap = snapshot(
        "truncate.html",
        r#"{% autoescape "none" %}{{ name | truncate(4, "[more]") }}|{{ short | truncate(4, "[more]") }}{% endautoescape %}"#,
    );
    let set = TemplateSet::compile(
        &snap,
        "truncate.html",
        MinijinjaLimitsConfig::default(),
        None,
    )
    .expect("compile must succeed — the filter is resolved at render time");
    let mut m: BTreeMap<&str, &str> = BTreeMap::new();
    m.insert("name", "abcdefgh");
    m.insert("short", "abc");
    let rendered = set
        .render_entry(
            minijinja::Value::from(minijinja::value::Serde(&m)),
            ResolvedLimits::default(),
        )
        .await
        .expect("render must succeed once `truncate` is registered");
    assert_eq!(
        rendered, "abcd[more]|abc",
        "the end marker must be appended only when the input was truncated"
    );
}

/// Negative, fractional, boolean, and string lengths are all invalid; each
/// must fail at render time on the length constraint (not on filter
/// resolution).
#[tokio::test]
async fn truncate_invalid_length_values_are_render_errors() {
    let cases = [
        ("negative", "{{ name | truncate(-2) }}"),
        ("fractional", "{{ name | truncate(2.5) }}"),
        ("boolean", "{{ name | truncate(true) }}"),
        ("string", r#"{{ name | truncate("2") }}"#),
    ];
    for (label, expr) in cases {
        let source = format!(r#"{{% autoescape "none" %}}{expr}{{% endautoescape %}}"#);
        let snap = snapshot("truncate.html", &source);
        let set = TemplateSet::compile(
            &snap,
            "truncate.html",
            MinijinjaLimitsConfig::default(),
            None,
        )
        .expect("compile must succeed — invalid lengths are render-time errors");
        let result = set
            .render_entry(ctx_name("abcdefgh"), ResolvedLimits::default())
            .await;
        let msg = match &result {
            Err(TemplateReloadError::Render(msg)) => msg.clone(),
            other => panic!(
                "invalid truncate length ({label}) must be a render-time failure \
                 and surface as TemplateReloadError::Render, got: {other:?}"
            ),
        };
        assert!(
            !msg.contains("unknown filter"),
            "invalid truncate length ({label}) must fail on the length constraint, \
             not on filter resolution; got: {msg}"
        );
    }
}

/// An integral length above `i64::MAX` is still valid: it simply exceeds
/// the input's char count, so the filter must be a no-op (no end marker)
/// rather than failing the i64 narrowing.
#[tokio::test]
async fn truncate_oversized_integral_length_is_noop() {
    let snap = snapshot(
        "truncate.html",
        r#"{% autoescape "none" %}{{ name | truncate(9223372036854775808) }}{% endautoescape %}"#,
    );
    let set = TemplateSet::compile(
        &snap,
        "truncate.html",
        MinijinjaLimitsConfig::default(),
        None,
    )
    .expect("compile must succeed — the filter is resolved at render time");
    let rendered = set
        .render_entry(ctx_name("abc"), ResolvedLimits::default())
        .await
        .expect("an oversized integral length must be accepted as a no-op");
    assert_eq!(
        rendered, "abc",
        "a length above i64::MAX must leave the string untouched (no end marker)"
    );
}

/// `dump` is deterministic JSON for the plain data shapes it is meant for:
/// objects (sorted keys), arrays, and scalars.
///
/// The fixture keys are deliberately non-alphabetical, so this test pins the
/// sorted-output contract for unsorted input; it does not prove that
/// `dump_filter` resorts, because minijinja stores object keys sorted
/// (`preserve_order` is off).
#[tokio::test]
async fn dump_serializes_object_array_scalar_deterministically() {
    let snap = snapshot(
        "dump.html",
        r#"{% autoescape "none" %}{{ obj | dump }}|{{ items | dump }}|{{ title | dump }}{% endautoescape %}"#,
    );
    let set = TemplateSet::compile(&snap, "dump.html", MinijinjaLimitsConfig::default(), None)
        .expect("compile must succeed — the filter is resolved at render time");
    let mut m: BTreeMap<&str, serde_json::Value> = BTreeMap::new();
    m.insert("obj", serde_json::json!({"b": 1, "a": 2}));
    m.insert("items", serde_json::json!([1, 2, 3]));
    m.insert("title", serde_json::json!("Ada"));
    let ctx = minijinja::Value::from(minijinja::value::Serde(&m));
    let first = set
        .render_entry(ctx.clone(), ResolvedLimits::default())
        .await
        .expect("render must succeed once `dump` is registered");
    assert_eq!(
        first, r#"{"a":2,"b":1}|[1,2,3]|"Ada""#,
        "dump must emit JSON with sorted object keys and no extra structure"
    );
    let second = set
        .render_entry(ctx, ResolvedLimits::default())
        .await
        .expect("second render must succeed");
    assert_eq!(
        first, second,
        "dump output must be byte-identical across renders of the same context"
    );
}

/// Direct unit call: a map whose key is a sequence has no JSON object
/// representation, so the filter must return `Err` instead of panicking or
/// silently lossy-converting.
#[test]
fn dump_filter_fails_on_unserializable_map() {
    // minijinja 3's `FromIterator for Value` builds a sequence, so construct
    // the sequence-keyed map explicitly through the serde wrapper.
    let fixture = minijinja::Value::from(minijinja::value::Serde(BTreeMap::from([(
        vec![1i64, 2],
        "x",
    )])));
    assert!(
        serde_json::to_string(&fixture).is_err(),
        "fixture precondition: a sequence-keyed map has no JSON object form"
    );
    let result = dump_filter(fixture);
    assert!(
        result.is_err(),
        "dump_filter must reject an unserializable value with Err, got: {result:?}"
    );
}

/// Render-path pin for the same scenario: the failure surfaces as
/// `TemplateReloadError::Render`, never as a panic or a compile error.
#[tokio::test]
async fn dump_unserializable_value_is_render_error() {
    let snap = snapshot(
        "dump.html",
        r#"{% autoescape "none" %}{{ broken | dump }}{% endautoescape %}"#,
    );
    let set = TemplateSet::compile(&snap, "dump.html", MinijinjaLimitsConfig::default(), None)
        .expect("compile must succeed — the filter is resolved at render time");
    let broken = minijinja::Value::from(minijinja::value::Serde(BTreeMap::from([(
        vec![1i64, 2],
        "x",
    )])));
    let ctx = minijinja::Value::from(minijinja::value::Serde(BTreeMap::from([(
        "broken", broken,
    )])));
    let result = set.render_entry(ctx, ResolvedLimits::default()).await;
    match &result {
        Err(TemplateReloadError::Render(_)) => {}
        other => panic!(
            "an unserializable dump input must be a render-time failure and \
             surface as TemplateReloadError::Render, got: {other:?}"
        ),
    }
}

/// `dump` returns ordinary text, so the active autoescape mode still
/// escapes it (the result must NOT be marked safe).
#[tokio::test]
async fn dump_output_stays_subject_to_autoescape() {
    let snap = snapshot(
        "dump.html",
        r#"{% autoescape "html" %}{{ obj | dump }}{% endautoescape %}"#,
    );
    let set = TemplateSet::compile(&snap, "dump.html", MinijinjaLimitsConfig::default(), None)
        .expect("compile must succeed — the filter is resolved at render time");
    let mut m: BTreeMap<&str, serde_json::Value> = BTreeMap::new();
    m.insert("obj", serde_json::json!({"path": "a/b"}));
    let rendered = set
        .render_entry(
            minijinja::Value::from(minijinja::value::Serde(&m)),
            ResolvedLimits::default(),
        )
        .await
        .expect("render must succeed once `dump` is registered");
    assert!(
        rendered.contains("&#x2f;"),
        "dump output must be escaped under html autoescape, got: {rendered}"
    );
    assert!(
        !rendered.contains("a/b"),
        "the raw `/` inside dump output must not survive html escaping, got: {rendered}"
    );
}

/// Allowlist pin: only the component's registered filters resolve; a name
/// it never registers stays an `unknown filter` render error.
#[tokio::test]
async fn only_allowlisted_component_filters_resolve() {
    let snap = ClosureSnapshot::from_entries(vec![
        (
            "truncate.html",
            br#"{% autoescape "none" %}{{ name | truncate(10) }}{% endautoescape %}"#,
        ),
        (
            "numerify.html",
            br#"{% autoescape "none" %}{{ name | numerify }}{% endautoescape %}"#,
        ),
    ]);
    let truncating = TemplateSet::compile(
        &snap,
        "truncate.html",
        MinijinjaLimitsConfig::default(),
        None,
    )
    .expect("compile must succeed — both entries parse");
    let rendered = truncating
        .render_entry(ctx_name("eleventy"), ResolvedLimits::default())
        .await
        .expect("the allowlisted `truncate` filter must resolve");
    assert_eq!(rendered, "eleventy");

    let unknown = TemplateSet::compile(
        &snap,
        "numerify.html",
        MinijinjaLimitsConfig::default(),
        None,
    )
    .expect("compile must succeed — filter names are resolved at render time");
    let result = unknown
        .render_entry(ctx_name("eleventy"), ResolvedLimits::default())
        .await;
    let msg = match &result {
        Err(TemplateReloadError::Render(msg)) => msg.clone(),
        other => panic!(
            "an unregistered filter name must be a render-time failure and \
             surface as TemplateReloadError::Render, got: {other:?}"
        ),
    };
    assert!(
        msg.contains("unknown filter"),
        "a name the component never registers must stay unregistered \
         (`unknown filter`), got: {msg}"
    );
}

/// Regression guard for the no-custom-escaper policy: `/` in an
/// expression is escaped to `&#x2f;` by minijinja's built-in html escaper.
#[tokio::test]
async fn slash_escapes_under_html_autoescape() {
    let snap = snapshot(
        "page.html",
        r#"{% autoescape "html" %}{{ path }}{% endautoescape %}"#,
    );
    let set = TemplateSet::compile(&snap, "page.html", MinijinjaLimitsConfig::default(), None)
        .expect("compile must succeed");
    let mut m: BTreeMap<&str, &str> = BTreeMap::new();
    m.insert("path", "a/b");
    let rendered = set
        .render_entry(
            minijinja::Value::from(minijinja::value::Serde(&m)),
            ResolvedLimits::default(),
        )
        .await
        .expect("render");
    assert!(
        rendered.contains("&#x2f;"),
        "html autoescape must escape `/` to `&#x2f;`, got: {rendered}"
    );
    assert!(
        !rendered.contains("a/b"),
        "the expression-emitted raw `/` must be absent, got: {rendered}"
    );
}
