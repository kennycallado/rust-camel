//! Mission 471: pin MiniJinja's strict-undefined missing-value contracts.
//!
//! The engine sets `UndefinedBehavior::Strict`, so an absent variable is a
//! render error rather than an empty string. The tests below pin that
//! behavior plus the two `default`-style opt-outs that keep working under it.

use camel_language_api::{Exchange, Language, Message, Value};
use camel_language_minijinja::MinijinjaLanguage;

/// Empty exchange: no `missing` header is set, so the path is undefined.
fn empty_exchange() -> Exchange {
    Exchange::new(Message::default())
}

/// Assert a render failure whose redacted kind text names the undefined value.
fn assert_undefined_render_error(result: Result<Value, camel_language_api::LanguageError>) {
    let err = result.expect_err("strict undefined must fail the render");
    let msg = err.to_string().to_lowercase();
    assert!(
        msg.contains("undefined") || msg.contains("missing"),
        "error must reference the undefined value: {msg}"
    );
}

#[tokio::test]
async fn missing_interpolation_fails_the_render() {
    let expr = MinijinjaLanguage::default()
        .create_expression(r#"{% autoescape "none" %}x{{ headers.missing }}x{% endautoescape %}"#)
        .expect("template must compile");
    let ex = empty_exchange();

    assert_undefined_render_error(expr.evaluate(&ex).await);
}

#[tokio::test]
async fn missing_conditional_fails_the_render() {
    let expr = MinijinjaLanguage::default()
        .create_expression(
            r#"{% autoescape "none" %}{% if headers.missing %}y{% endif %}{% endautoescape %}"#,
        )
        .expect("template must compile");
    let ex = empty_exchange();

    assert_undefined_render_error(expr.evaluate(&ex).await);
}

/// `| default('')` is the documented way to opt out of strict undefined for a
/// single interpolation. Verified empirically against minijinja 2.24.0
/// (pinned in `Cargo.lock`): the empty-string fallback renders as `[]`
/// (no stray whitespace).
#[tokio::test]
async fn minijinja_default_filter_opt_out_renders() {
    let expr = MinijinjaLanguage::default()
        .create_expression(
            r#"{% autoescape "none" %}[{{ headers.missing | default('') }}]{% endautoescape %}"#,
        )
        .expect("template must compile");
    let ex = empty_exchange();

    let rendered = expr.evaluate(&ex).await.expect("default filter renders");
    assert_eq!(rendered, Value::String("[]".to_string()));
}

/// `is defined` guards the read without touching the value, so the `else`
/// branch renders instead of failing. Verified empirically.
#[tokio::test]
async fn minijinja_defined_test_opt_out_renders() {
    let expr = MinijinjaLanguage::default()
        .create_expression(
            r#"{% autoescape "none" %}{% if headers.missing is defined %}yes{% else %}no{% endif %}{% endautoescape %}"#,
        )
        .expect("template must compile");
    let ex = empty_exchange();

    let rendered = expr.evaluate(&ex).await.expect("defined test renders");
    assert_eq!(rendered, Value::String("no".to_string()));
}
