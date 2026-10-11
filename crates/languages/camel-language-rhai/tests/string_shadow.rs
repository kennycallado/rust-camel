//! Pure `trim` shadow regression (change `mission359-languagep1wave`,
//! task 3.2).
//!
//! Rhai's stock `StandardPackage` registers `trim(&mut ImmutableString)` as an
//! in-place method that returns unit `()`. As a consequence
//! `let x = "   "; x.trim() != ""` was ALWAYS true (unit never equals a
//! string) and a discard-statement form silently mutated the subject. The
//! crate now registers a PURE `trim(ImmutableString) -> String` after the
//! standard package, which replaces the mutating registration. These tests pin
//! the pure contract in both the read-only (`create_expression`) and script
//! step (`create_mutating_expression`) engines.

use camel_language_api::{Exchange, Language, Message, Value};
use camel_language_rhai::RhaiLanguage;

fn exchange_with_body(body: &str) -> Exchange {
    Exchange::new(Message::new(body))
}

/// `trim()` returns a string, so equality against the trimmed text holds.
#[tokio::test]
async fn trim_returns_string() {
    let expr = RhaiLanguage::new()
        .create_expression(r#"let x = "  a  "; x.trim() == "a""#)
        .expect("trim expression compiles");
    let ex = exchange_with_body("body");
    assert_eq!(
        expr.evaluate(&ex).await.expect("trim must eval"),
        Value::Bool(true)
    );
}

/// The blank check is now correct: a whitespace-only string trims to `""`, so
/// `== ""` is true and `!= ""` is false (the old in-place/unit trim made every
/// non-empty comparison false or true-inverted).
#[tokio::test]
async fn trim_blank_check_is_correct() {
    let lang = RhaiLanguage::new();
    let ex = exchange_with_body("body");

    let eq = lang
        .create_expression(r#"let x = "   "; x.trim() == """#)
        .expect("trim expression compiles");
    assert_eq!(
        eq.evaluate(&ex).await.expect("trim must eval"),
        Value::Bool(true),
        "a whitespace-only string must trim to the empty string"
    );

    let ne = lang
        .create_expression(r#"let x = "   "; x.trim() != """#)
        .expect("trim expression compiles");
    assert_eq!(
        ne.evaluate(&ex).await.expect("trim must eval"),
        Value::Bool(false),
        "blank != '' must be false once trim is pure"
    );
}

/// The discard-statement form does NOT mutate the subject: `x` still holds the
/// original padded text after `x.trim();`.
#[tokio::test]
async fn trim_does_not_mutate_subject() {
    let expr = RhaiLanguage::new()
        .create_expression(r#"let x = "  a  "; x.trim(); x == "  a  ""#)
        .expect("trim expression compiles");
    let ex = exchange_with_body("body");
    assert_eq!(
        expr.evaluate(&ex).await.expect("trim must eval"),
        Value::Bool(true),
        "trim must not mutate the subject"
    );
}

/// The shadow is registered on the BASE engine, so the mutating/script-step
/// engine has it too.
#[tokio::test]
async fn trim_shadow_applies_in_script_step() {
    let expr = RhaiLanguage::new()
        .create_mutating_expression(r#"let x = "  a  "; x.trim() == "a""#)
        .expect("trim script compiles");
    let mut ex = exchange_with_body("body");
    assert_eq!(
        expr.evaluate(&mut ex).await.expect("trim must eval"),
        Value::Bool(true)
    );
}

/// The mutating engine's discard-statement form also leaves the subject
/// untouched (regression for the old silent in-place mutation).
#[tokio::test]
async fn trim_does_not_mutate_subject_in_script_step() {
    let expr = RhaiLanguage::new()
        .create_mutating_expression(r#"let x = "  a  "; x.trim(); x == "  a  ""#)
        .expect("trim script compiles");
    let mut ex = exchange_with_body("body");
    assert_eq!(
        expr.evaluate(&mut ex).await.expect("trim must eval"),
        Value::Bool(true),
        "trim must not mutate the subject in a script step"
    );
}
