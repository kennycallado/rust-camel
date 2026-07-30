//! MiniJinja inline template rendering language for rust-camel.
//!
//! Implements the `Language`, `Expression`, and `Predicate` SPI from
//! `camel-language-api` backed by MiniJinja. Inline templates only —
//! external file loading is Phase 2 (bd rc-64if).
//!
//! The public compile/render path lives in [`engine`] and is shared with
//! the external `camel-template` Component (ADR-0047 Stage 2). The autoescape
//! wrapper validator is also re-exported for use by the Component's
//! startup-time compile pass.
//!
//! See ADR-0047 for the architectural contract.

pub mod autoescape_validator;
pub mod engine;
mod limited_writer;

pub use autoescape_validator::validate_autoescape_wrapper;
pub use engine::{MinijinjaExpression, ResolvedLimits, render};
pub use limited_writer::LimitedWriter;

// `Body`, `Exchange`, `Value` are only used in the test mods below; gate
// the import so production builds don't carry it. The items are pulled
// into each test mod via `use super::*;`.
#[cfg(test)]
use camel_api::{Body, Exchange, Value};
use camel_language_api::{Expression, Language, LanguageError, MinijinjaLimitsConfig, Predicate};

/// MiniJinja scripting language for rust-camel.
///
/// Inline templates only — no external file loading (Phase 2).
#[derive(Debug, Clone, Default)]
pub struct MinijinjaLanguage {
    /// Resource limits for the MiniJinja environment.
    /// Consumed by `create_expression` to derive `ResolvedLimits` per compile.
    limits: MinijinjaLimitsConfig,
}

impl MinijinjaLanguage {
    pub fn with_limits(limits: MinijinjaLimitsConfig) -> Self {
        Self { limits }
    }
}

impl Language for MinijinjaLanguage {
    fn name(&self) -> &'static str {
        "minijinja"
    }

    fn create_predicate(&self, _script: &str) -> Result<Box<dyn Predicate>, LanguageError> {
        Err(LanguageError::NotSupported {
            feature: "predicates".to_string(),
            language: "minijinja".to_string(),
        })
    }

    fn create_expression(&self, script: &str) -> Result<Box<dyn Expression>, LanguageError> {
        let limits = ResolvedLimits::from_config(&self.limits);
        let expr = MinijinjaExpression::compile(script, limits)?;
        Ok(Box::new(expr))
    }
}

#[cfg(test)]
mod language_tests {
    use super::*;
    use camel_language_api::{Language, LanguageError};

    #[test]
    fn name_returns_minijinja() {
        assert_eq!(MinijinjaLanguage::default().name(), "minijinja");
    }

    #[test]
    fn create_predicate_returns_not_supported_without_inspecting_script() {
        let result = MinijinjaLanguage::default().create_predicate("{% if x %}true{% endif %}");
        assert!(matches!(
            result,
            Err(LanguageError::NotSupported { feature, language })
                if feature == "predicates" && language == "minijinja"
        ));
    }
}

#[cfg(test)]
mod expression_tests {
    use super::*;
    use camel_api::Exchange;

    #[tokio::test]
    async fn compiles_template_once_across_evaluations() {
        let expr = MinijinjaExpression::compile(
            "{% autoescape \"none\" %}hi {{ headers.name }}{% endautoescape %}",
            ResolvedLimits::default(),
        )
        .expect("compile");

        let name = expr.template_name().to_string();
        // minijinja 2.21 has no `has_template` — use `get_template(...).is_ok()`
        assert!(expr.environment().get_template(&name).is_ok());
        assert_eq!(
            expr.compile_count(),
            1,
            "compile must register exactly once"
        );

        let mut ex = Exchange::default();
        ex.input
            .headers
            .insert("name".into(), Value::String("kenny".into()));

        let v1 = expr.evaluate(&ex).await.expect("first render");
        let v2 = expr.evaluate(&ex).await.expect("second render");
        let v3 = expr.evaluate(&ex).await.expect("third render");

        assert_eq!(v1, Value::String("hi kenny".to_string()));
        assert_eq!(v2, v1);
        assert_eq!(v3, v1);
        assert_eq!(expr.template_name(), name);
        assert!(expr.environment().get_template(&name).is_ok());
        assert_eq!(expr.compile_count(), 1, "evaluate must not re-compile");
    }

    #[tokio::test]
    async fn missing_path_fails_under_strict_undefined() {
        let expr = MinijinjaExpression::compile(
            "{% autoescape \"none\" %}{{ headers.missing.path }}{% endautoescape %}",
            ResolvedLimits::default(),
        )
        .expect("compile");
        let ex = Exchange::default();
        let result = expr.evaluate(&ex).await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string().to_lowercase();
        assert!(
            msg.contains("undefined") || msg.contains("missing"),
            "error must reference the undefined path: {msg}"
        );
    }

    #[test]
    fn resolved_limits_defaults_match_spec() {
        let r = ResolvedLimits::from_config(&MinijinjaLimitsConfig::default());
        assert_eq!(r.max_template_source_size, 1 << 20);
        assert_eq!(r.max_context_size, 4 << 20);
        assert_eq!(r.max_output_size, 4 << 20);
        assert_eq!(r.fuel, 100_000);
        assert_eq!(r.max_recursion_depth, 64);
        assert_eq!(r.execution_timeout_ms, 5_000);
    }

    #[test]
    fn resolved_limits_per_field_override() {
        let cases: [(MinijinjaLimitsConfig, &str); 6] = [
            (
                MinijinjaLimitsConfig {
                    max_template_source_size: Some(7),
                    ..Default::default()
                },
                "max_template_source_size",
            ),
            (
                MinijinjaLimitsConfig {
                    max_context_size: Some(7),
                    ..Default::default()
                },
                "max_context_size",
            ),
            (
                MinijinjaLimitsConfig {
                    max_output_size: Some(7),
                    ..Default::default()
                },
                "max_output_size",
            ),
            (
                MinijinjaLimitsConfig {
                    fuel: Some(7),
                    ..Default::default()
                },
                "fuel",
            ),
            (
                MinijinjaLimitsConfig {
                    max_recursion_depth: Some(7),
                    ..Default::default()
                },
                "max_recursion_depth",
            ),
            (
                MinijinjaLimitsConfig {
                    execution_timeout_ms: Some(7),
                    ..Default::default()
                },
                "execution_timeout_ms",
            ),
        ];
        for (cfg, field_name) in cases {
            let r = ResolvedLimits::from_config(&cfg);
            let actual: usize = match field_name {
                "max_template_source_size" => r.max_template_source_size,
                "max_context_size" => r.max_context_size,
                "max_output_size" => r.max_output_size,
                "fuel" => r.fuel as usize,
                "max_recursion_depth" => r.max_recursion_depth as usize,
                "execution_timeout_ms" => r.execution_timeout_ms as usize,
                _ => unreachable!(),
            };
            assert_eq!(actual, 7, "field {field_name} should be overridden to 7");
        }
        let all = MinijinjaLimitsConfig {
            max_template_source_size: Some(1),
            max_context_size: Some(2),
            max_output_size: Some(3),
            fuel: Some(4),
            max_recursion_depth: Some(5),
            execution_timeout_ms: Some(6),
        };
        let r = ResolvedLimits::from_config(&all);
        assert_eq!(r.max_template_source_size, 1);
        assert_eq!(r.max_context_size, 2);
        assert_eq!(r.max_output_size, 3);
        assert_eq!(r.fuel, 4);
        assert_eq!(r.max_recursion_depth, 5);
        assert_eq!(r.execution_timeout_ms, 6);
    }

    #[test]
    fn oversized_source_trips_at_compile_as_parse_error() {
        let limits = ResolvedLimits {
            max_template_source_size: 8,
            ..Default::default()
        };
        let huge = format!(
            "{{% autoescape \"none\" %}}{}{{% endautoescape %}}",
            "x".repeat(64)
        );
        let result = MinijinjaExpression::compile(&huge, limits);
        assert!(
            matches!(result, Err(LanguageError::ParseError { .. })),
            "oversized source must surface as ParseError, got: {result:?}"
        );
        let msg = result.unwrap_err().to_string().to_lowercase();
        assert!(msg.contains("source") || msg.contains("size"));
    }

    #[test]
    fn missing_autoescape_wrapper_returns_parse_error_naming_wrapper() {
        let result = MinijinjaExpression::compile("hi {{ name }}", ResolvedLimits::default());
        assert!(matches!(result, Err(LanguageError::ParseError { .. })));
        let msg = result.unwrap_err().to_string().to_lowercase();
        assert!(
            msg.contains("autoescape"),
            "error must name autoescape: {msg}"
        );
    }

    #[test]
    fn nested_wrapper_returns_parse_error() {
        let src = "{% autoescape \"html\" %}{% autoescape \"json\" %}x{% endautoescape %}{% endautoescape %}";
        let result = MinijinjaExpression::compile(src, ResolvedLimits::default());
        assert!(matches!(result, Err(LanguageError::ParseError { .. })));
    }

    #[tokio::test]
    async fn recursion_depth_exhaustion_trips_at_render() {
        let limits = ResolvedLimits {
            max_recursion_depth: 4,
            ..Default::default()
        };
        let src = "{% autoescape \"none\" %}{% macro r(n) %}{% if n > 0 %}{{ r(n - 1) }}{% endif %}{% endmacro %}{{ r(99) }}{% endautoescape %}";
        let expr = MinijinjaExpression::compile(src, limits).expect("compile");
        let result = expr.evaluate(&Exchange::default()).await;
        assert!(result.is_err(), "recursion-depth must trip");
    }
}

#[cfg(test)]
mod happy_path_tests {
    use super::*;
    use camel_api::Exchange;

    async fn render(src: &str, exchange: &Exchange) -> String {
        let expr = MinijinjaExpression::compile(src, ResolvedLimits::default()).expect("compile");
        let v = expr.evaluate(exchange).await.expect("render");
        match v {
            Value::String(s) => s,
            other => panic!("expected String, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn html_render_escapes() {
        let src = "{% autoescape \"html\" %}<p>{{ headers.x }}</p>{% endautoescape %}";
        let mut ex = Exchange::default();
        ex.input
            .headers
            .insert("x".into(), Value::String("<b>".into()));
        assert_eq!(render(src, &ex).await, "<p>&lt;b&gt;</p>");
    }

    // Note: plan expected `a\"b` (4 chars), but MiniJinja's `AutoEscape::Json`
    // serialises the value as a JSON string literal — including surrounding
    // quotes. Correct expectation is the full 6-char JSON serialization.
    // Requires minijinja `json` cargo feature (enabled in workspace Cargo.toml).
    #[tokio::test]
    async fn json_render_escapes() {
        let src = "{% autoescape \"json\" %}{{ headers.x }}{% endautoescape %}";
        let mut ex = Exchange::default();
        ex.input
            .headers
            .insert("x".into(), Value::String("a\"b".into()));
        assert_eq!(render(src, &ex).await, "\"a\\\"b\"");
    }

    #[tokio::test]
    async fn none_render_emits_verbatim() {
        let src = "{% autoescape \"none\" %}{{ headers.x }}{% endautoescape %}";
        let mut ex = Exchange::default();
        ex.input
            .headers
            .insert("x".into(), Value::String("<b>".into()));
        assert_eq!(render(src, &ex).await, "<b>");
    }

    #[tokio::test]
    async fn for_loop_iterates() {
        let src = "{% autoescape \"none\" %}{% for i in headers.items %}{{ i }}{% endfor %}{% endautoescape %}";
        let mut ex = Exchange::default();
        ex.input
            .headers
            .insert("items".into(), serde_json::json!([1, 2, 3]));
        assert_eq!(render(src, &ex).await, "123");
    }

    #[tokio::test]
    async fn if_conditional_branches() {
        let src = "{% autoescape \"none\" %}{% if headers.x %}yes{% else %}no{% endif %}{% endautoescape %}";
        let mut ex = Exchange::default();
        ex.input
            .headers
            .insert("x".into(), Value::String("true".into()));
        assert_eq!(render(src, &ex).await, "yes");
    }

    // Note: plan expected `a+b%2Fc` (HTML form-style encoding with `+` for
    // space). MiniJinja's `urlencode` filter uses RFC 3986 percent-encoding
    // (via the `percent-encoding` crate): space is `%20`, `/` is unreserved
    // in this position and stays literal. Requires minijinja `urlencode`
    // cargo feature (enabled in workspace Cargo.toml).
    #[tokio::test]
    async fn urlencode_filter_renders() {
        let src = "{% autoescape \"none\" %}{{ headers.x | urlencode }}{% endautoescape %}";
        let mut ex = Exchange::default();
        ex.input
            .headers
            .insert("x".into(), Value::String("a b/c".into()));
        assert_eq!(render(src, &ex).await, "a%20b/c");
    }

    #[tokio::test]
    async fn upper_filter_renders() {
        let src = "{% autoescape \"none\" %}{{ headers.x |upper }}{% endautoescape %}";
        let mut ex = Exchange::default();
        ex.input
            .headers
            .insert("x".into(), Value::String("abc".into()));
        assert_eq!(render(src, &ex).await, "ABC");
    }

    #[tokio::test]
    async fn default_filter_when_missing() {
        let src = "{% autoescape \"none\" %}{{ headers.missing |default(\"fallback\") }}{% endautoescape %}";
        assert_eq!(render(src, &Exchange::default()).await, "fallback");
    }

    #[tokio::test]
    async fn inline_macro_expands() {
        let src = "{% autoescape \"none\" %}{% macro greet(n) %}hi {{ n }}{% endmacro %}{{ greet(headers.name) }}{% endautoescape %}";
        let mut ex = Exchange::default();
        ex.input
            .headers
            .insert("name".into(), Value::String("kenny".into()));
        assert_eq!(render(src, &ex).await, "hi kenny");
    }
}

#[cfg(test)]
mod context_exposure_tests {
    use super::*;
    use camel_api::Exchange;

    // S3 — template cannot reach CamelContext, filesystem, env vars, registries.
    // These tests are negative-existence invariants: they pass at first run
    // by design (Task 7 registered no globals). They serve as regression
    // pins: if a future change accidentally introduces a registration, the
    // test catches it.

    #[tokio::test]
    async fn no_env_global() {
        let expr = MinijinjaExpression::compile(
            "{% autoescape \"none\" %}{{ env(\"PATH\") }}{% endautoescape %}",
            ResolvedLimits::default(),
        )
        .expect("compile");
        assert!(expr.evaluate(&Exchange::default()).await.is_err());
    }

    #[tokio::test]
    async fn no_filesystem_global() {
        let expr = MinijinjaExpression::compile(
            "{% autoescape \"none\" %}{{ load_file(\"/etc/passwd\") }}{% endautoescape %}",
            ResolvedLimits::default(),
        )
        .expect("compile");
        assert!(expr.evaluate(&Exchange::default()).await.is_err());
    }
}

#[cfg(test)]
mod bounded_io_test {
    use super::*;
    use camel_api::Exchange;

    #[tokio::test]
    async fn oversize_output_trips_limited_writer() {
        let limits = ResolvedLimits {
            max_output_size: 8,
            ..Default::default()
        };
        let expr = MinijinjaExpression::compile(
            "{% autoescape \"none\" %}{% for i in range(0, 100000) %}x{% endfor %}{% endautoescape %}",
            limits,
        ).expect("compile");
        let result = expr.evaluate(&Exchange::default()).await;
        assert!(result.is_err(), "render must trip max-output-size");
        let msg = result.unwrap_err().to_string().to_lowercase();
        assert!(msg.contains("output") || msg.contains("limit"));
    }

    #[tokio::test]
    async fn oversize_context_trips_before_full_allocation() {
        let limits = ResolvedLimits {
            max_context_size: 8,
            ..Default::default()
        };
        let expr = MinijinjaExpression::compile(
            "{% autoescape \"none\" %}{{ body }}{% endautoescape %}",
            limits,
        )
        .expect("compile");
        let mut ex = Exchange::default();
        ex.input.body = Body::from("x".repeat(1024));
        let result = expr.evaluate(&ex).await;
        assert!(result.is_err(), "must trip max-context-size");
        let msg = result.unwrap_err().to_string().to_lowercase();
        assert!(msg.contains("context") || msg.contains("limit"));
    }
}

#[cfg(test)]
mod body_stream_rejection_test {
    use super::*;
    use bytes::Bytes;
    use camel_api::body::{StreamBody, StreamMetadata};
    use camel_api::error::CamelError;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[tokio::test]
    async fn stream_body_rejected_with_stream_cache_hint() {
        let expr = MinijinjaExpression::compile(
            "{% autoescape \"none\" %}{{ body }}{% endautoescape %}",
            ResolvedLimits::default(),
        )
        .expect("compile");

        let stream = futures::stream::empty::<Result<Bytes, CamelError>>();
        let body_stream = Body::Stream(StreamBody {
            stream: Arc::new(Mutex::new(Some(Box::pin(stream)))),
            metadata: StreamMetadata {
                size_hint: None,
                content_type: None,
                origin: None,
            },
        });
        let mut ex = Exchange::default();
        ex.input.body = body_stream;

        let result = expr.evaluate(&ex).await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("stream_cache"),
            "error must mention stream_cache: {msg}"
        );
    }
}

#[cfg(test)]
mod timeout_test {
    use super::*;
    use camel_api::Exchange;

    #[tokio::test]
    async fn timeout_fires_on_cpu_bound_render() {
        // Calibration deviation from spec (MiniJinja 2.21 cap = 100k elements
        // per range, see functions.rs:326 — `range(0, 10000000)` rejects at
        // render time). Nested range(0,1000) × range(0,1000) = 1M valid
        // iterations, each emitting {{ i }} so the work is non-trivial.
        // fuel=100B guarantees fuel does not exhaust before the 50ms wall-clock
        // fires (test runs ~130ms, confirming timeout wins).
        // Residual-worker caveat (§4.2): blocking thread keeps running after
        // timeout; fuel + recursion/output bounds cap runaway work.
        let limits = ResolvedLimits {
            fuel: 100_000_000_000,
            execution_timeout_ms: 50,
            ..Default::default()
        };
        let expr = MinijinjaExpression::compile(
            "{% autoescape \"none\" %}{% for i in range(0, 1000) %}{% for j in range(0, 1000) %}{{ i }}{% endfor %}{% endfor %}{% endautoescape %}",
            limits,
        ).expect("compile");
        let result = expr.evaluate(&Exchange::default()).await;
        assert!(result.is_err(), "timeout must fire");
        let msg = result.unwrap_err().to_string().to_lowercase();
        assert!(msg.contains("timeout"), "expected timeout, got: {msg}");
    }
}

#[cfg(test)]
mod source_not_from_header_test {
    use super::*;

    // S1 — header values rendered as data, never re-compiled. This test
    // verifies the negative existential that MiniJinja rendering is
    // non-recursive (header bytes are interpolated as text, never re-parsed
    // as a template). It passes at first run given the Task 7+8 impl; it
    // serves as a regression pin against any future change that might add
    // header-driven source selection.

    #[tokio::test]
    async fn header_value_emitted_verbatim_never_recompiled() {
        let bound = "{% autoescape \"none\" %}{{ headers.template_source }}{% endautoescape %}";
        let expr = MinijinjaExpression::compile(bound, ResolvedLimits::default()).expect("compile");

        let mut ex = Exchange::default();
        // Adversarial header value that LOOKS like a template — must NOT be
        // re-compiled or executed. MiniJinja rendering is non-recursive, and
        // `autoescape "none"` performs no escaping.
        ex.input.headers.insert(
            "template_source".into(),
            Value::String(
                "{% autoescape \"none\" %}evil{{ body.sensitive }}{% endautoescape %}".into(),
            ),
        );
        ex.input.body = Body::from("SECRET");

        let v = expr.evaluate(&ex).await.expect("render");
        let s = match v {
            Value::String(s) => s,
            other => panic!("expected String, got {other:?}"),
        };

        // The header value is emitted verbatim (no re-compile).
        assert_eq!(
            s,
            "{% autoescape \"none\" %}evil{{ body.sensitive }}{% endautoescape %}"
        );
        // The SECRET body was never reached by a re-compiled header.
        assert!(!s.contains("SECRET"));
    }
}

#[cfg(test)]
mod filter_boundary_test {
    use super::*;

    // S8 — no shell/SQL escape; urlencode available. These tests verify the
    // negative existential that no `shell_escape` / `sql_escape` filter was
    // registered (Task 7 explicitly registers nothing). They pass at first run
    // by design and serve as regression pins.

    #[tokio::test]
    async fn shell_escape_filter_not_registered() {
        let expr = MinijinjaExpression::compile(
            "{% autoescape \"none\" %}{{ headers.x |shell_escape }}{% endautoescape %}",
            ResolvedLimits::default(),
        )
        .expect("compile");
        let mut ex = Exchange::default();
        ex.input
            .headers
            .insert("x".into(), Value::String("whoami".into()));
        let result = expr.evaluate(&ex).await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string().to_lowercase();
        assert!(msg.contains("filter") || msg.contains("unknown"));
    }

    #[tokio::test]
    async fn sql_escape_filter_not_registered() {
        let expr = MinijinjaExpression::compile(
            "{% autoescape \"none\" %}{{ headers.x |sql_escape }}{% endautoescape %}",
            ResolvedLimits::default(),
        )
        .expect("compile");
        let mut ex = Exchange::default();
        ex.input
            .headers
            .insert("x".into(), Value::String("DROP TABLE".into()));
        assert!(expr.evaluate(&ex).await.is_err());
    }

    // Note: plan expected "a+b" (HTML form-style). MiniJinja's urlencode uses
    // RFC 3986 percent-encoding (percent-encoding crate): space is %20.
    // Same correction as Task 8 (see bd rc-i231).
    #[tokio::test]
    async fn urlencode_filter_is_registered() {
        let expr = MinijinjaExpression::compile(
            "{% autoescape \"none\" %}{{ headers.x |urlencode }}{% endautoescape %}",
            ResolvedLimits::default(),
        )
        .expect("compile");
        let mut ex = Exchange::default();
        ex.input
            .headers
            .insert("x".into(), Value::String("a b".into()));
        let v = expr.evaluate(&ex).await.expect("render");
        assert_eq!(v, Value::String("a%20b".into()));
    }
}

#[cfg(test)]
mod fuel_test {
    use super::*;

    // S5/S6 — fuel exhaustion trips a render-time error.
    #[tokio::test]
    async fn fuel_exhaustion_returns_error() {
        let limits = ResolvedLimits {
            fuel: 10,
            ..Default::default()
        };
        let expr = MinijinjaExpression::compile(
            "{% autoescape \"none\" %}{% for i in range(0, 100000) %}x{% endfor %}{% endautoescape %}",
            limits,
        ).expect("compile");
        let result = expr.evaluate(&Exchange::default()).await;
        assert!(result.is_err(), "fuel must trip on heavy render");
        let msg = result.unwrap_err().to_string().to_lowercase();
        assert!(
            msg.contains("fuel") || msg.contains("instruction"),
            "error must reference fuel/instruction: {msg}"
        );
    }

    // Calibration — representative renders complete successfully at a fuel
    // budget well below the 100 000 default. Uses a threshold-sweep because
    // MiniJinja's Environment::fuel() returns the configured budget, not
    // residual fuel (so direct before/after observation is not available
    // without reaching into render `State`).
    //
    // The threshold (10_000) is intentionally tight: if a representative
    // render fails at 10_000 fuel, the default needs to rise OR the operator
    // needs clearer guidance. If it succeeds, we have a 10x safety margin
    // against the 100 000 default.
    //
    // (JSON template includes literal `"` and bare `{` chars in source.
    // Autoescape validator tokenizer fixed to handle bare `{` as literal text;
    // MiniJinja 2.21 renders the template correctly.)
    #[tokio::test]
    async fn representative_renders_complete_at_tight_fuel_threshold() {
        let cases: &[(&str, &str)] = &[
            (
                "html",
                "<html><body><h1>{{ headers.title }}</h1><ul>{% for i in headers.items %}<li>{{ i }}</li>{% endfor %}</ul></body></html>",
            ),
            (
                "json",
                "{\"v\":[{% for i in headers.items %}{{ i }}{% if not loop.last %},{% endif %}{% endfor %}]}",
            ),
            ("none", "{{ headers.system }} prompt: {{ body }}"),
        ];
        for (mode, src) in cases {
            let wrapped = format!("{{% autoescape \"{mode}\" %}}{src}{{% endautoescape %}}");
            // Tight fuel budget — proves cost < 10_000 (< 10% of default).
            let expr = MinijinjaExpression::compile(
                &wrapped,
                ResolvedLimits {
                    fuel: 10_000,
                    ..Default::default()
                },
            )
            .expect("compile");

            let mut ex = Exchange::default();
            ex.input
                .headers
                .insert("title".into(), Value::String("T".into()));
            ex.input
                .headers
                .insert("system".into(), Value::String("S".into()));
            ex.input
                .headers
                .insert("items".into(), serde_json::json!([1, 2, 3]));
            ex.input.body = Body::from("body text");

            let v = expr.evaluate(&ex).await.unwrap_or_else(|e| {
                panic!(
                    "{mode} render must complete at fuel=10_000 (well under the 100_000 default); \
                 if it fails, raise the threshold here and update spec §4.1 / crate README to \
                 reflect the actual cost. Error: {e}"
                )
            });
            assert!(
                matches!(v, Value::String(_)),
                "{mode} should render to string"
            );
        }
    }
}

#[cfg(test)]
mod engine_render_public_path {
    // Task 1.2 acceptance: `engine::render` is reachable as a public path.
    // The spec test calls `camel_language_minijinja::engine::render("{{x}}",
    // &value, ResolvedLimits::default()).await`; the source is wrapped in
    // the required `{% autoescape "none" %}` block (per the S7 contract
    // enforced by the lexical validator) so the render actually produces
    // the asserted output rather than failing at compile time.
    use super::*;
    use minijinja::Value as MjValue;

    #[tokio::test]
    async fn engine_render_is_public() {
        let value = MjValue::from_serialize(serde_json::json!({ "x": "y" }));
        let rendered = engine::render(
            "{% autoescape \"none\" %}{{x}}{% endautoescape %}",
            &value,
            ResolvedLimits::default(),
        )
        .await
        .expect("render");
        assert_eq!(rendered, "y");
    }
}

#[cfg(test)]
mod evaluate_compile_once {
    // Compile-once invariant (AC8) regression. The first cut of Task 1.2
    // routed `Expression::evaluate` through the public `engine::render`,
    // which calls `MinijinjaExpression::compile` afresh every invocation.
    // That broke the AC8 invariant silently: the per-instance `compile_count`
    // stayed at 1 (a fresh internal `MinijinjaExpression` carried its own
    // counter), so `compiles_template_once_across_evaluations` passed even
    // though the template was being re-parsed on every evaluate.
    //
    // The fix renders the pre-compiled `self.environment()` directly. This
    // test guards the invariant by snapshotting the per-source compile
    // counter around an evaluate burst and asserting the delta is zero. The
    // probe is keyed by source string, so the test uses a unique source
    // (`TEST_SOURCE`) that no other test compiles, making the probe
    // race-free against parallel test execution.
    use super::*;
    use camel_api::Exchange;

    /// Unique source string for this test only — keyed in the per-source
    /// probe map so concurrent compiles from other tests do not interfere.
    const TEST_SOURCE: &str = "{% autoescape \"none\" %}compile-once-regression-7f3a {{ headers.name }}{% endautoescape %}";

    #[tokio::test]
    async fn evaluate_does_not_invoke_compile() {
        let before = MinijinjaExpression::compile_count_for_source(TEST_SOURCE);
        let expr =
            MinijinjaExpression::compile(TEST_SOURCE, ResolvedLimits::default()).expect("compile");
        let after_compile = MinijinjaExpression::compile_count_for_source(TEST_SOURCE);
        assert_eq!(
            after_compile,
            before + 1,
            "compile must bump per-source probe by exactly 1 \
             (before={before}, after_compile={after_compile})"
        );
        // Per-instance invariant: still 1 after the compile above.
        assert_eq!(expr.compile_count(), 1);

        let mut ex = Exchange::default();
        ex.input
            .headers
            .insert("name".into(), Value::String("kenny".into()));

        for _ in 0..3 {
            let v = expr.evaluate(&ex).await.expect("render");
            assert_eq!(
                v,
                Value::String("compile-once-regression-7f3a kenny".into())
            );
        }
        let after_evaluates = MinijinjaExpression::compile_count_for_source(TEST_SOURCE);
        assert_eq!(
            after_evaluates, after_compile,
            "evaluate must not bump per-source probe (compile-once invariant) \
             (after_compile={after_compile}, after_evaluates={after_evaluates})"
        );
        // Per-instance invariant again: still 1 after the evaluate burst.
        assert_eq!(expr.compile_count(), 1);
    }
}
