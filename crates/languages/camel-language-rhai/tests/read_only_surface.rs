//! Read-surface regression (change `mission359-languagep1wave`, tasks 3.3 and
//! 3.4).
//!
//! The read-only surface now exposes the `properties` map alongside `body`
//! and `headers`, and the script-step engine registers `header()`/`property()`
//! backed by the PRE-run snapshot maps. A reader therefore observes the
//! pre-run value even after the script mutates the corresponding map, while
//! the mutated map is what propagates back to the exchange (M7). The rhai
//! `set_*` rejection hint names the working map-assignment surface; JS is
//! unchanged.
//!
//! Task 3.4 additionally rejects assignment to the read-only roots
//! `body`/`headers`/`properties` at create time (conservatively, including a
//! local that shadows the name). The rejection is an assignment-specific
//! diagnostic, distinct from the `set_*()` callee one, and keeps the
//! `script:` hint.

use bytes::Bytes;
use camel_language_api::{
    Body, Exchange, ExpressionErrorClass, Language, LanguageError, Message, Value,
};
use camel_language_rhai::RhaiLanguage;

/// Assert that a read-only script is rejected at create time as a forbidden
/// assignment (not as a forbidden callee): the diagnostic names the
/// assignment, keeps the `script:` hint, and never borrows the callee wording.
fn assert_read_only_assignment_rejected(script: &str) {
    let err = match RhaiLanguage::new().create_expression(script) {
        Ok(_) => panic!("read-only assignment must be rejected at create time: {script}"),
        Err(err) => err,
    };
    let LanguageError::ParseError { reason, .. } = &err else {
        panic!("expected ParseError for {script}, got: {err:?}");
    };
    assert!(
        reason.contains("assignment"),
        "diagnostic must name the assignment for {script}: {reason}"
    );
    assert!(
        reason.contains("script:"),
        "diagnostic must keep the `script:` hint for {script}: {reason}"
    );
    assert!(
        !reason.contains("() cannot be used"),
        "assignment diagnostic must not use the callee phrasing for {script}: {reason}"
    );
}

/// Read-only expressions can read the `properties` map directly.
#[tokio::test]
async fn read_only_properties_map_is_readable() {
    let expr = RhaiLanguage::new()
        .create_expression(r#"properties["p"]"#)
        .expect("properties map read must compile");
    let mut ex = Exchange::new(Message::default());
    ex.set_property("p", "pv");
    assert_eq!(
        expr.evaluate(&ex).await.expect("eval"),
        Value::String("pv".into())
    );
}

/// The script-step engine provides `property()` backed by the snapshot.
#[tokio::test]
async fn script_step_property_reader_works() {
    let expr = RhaiLanguage::new()
        .create_mutating_expression(r#"property("p")"#)
        .expect("property reader must compile");
    let mut ex = Exchange::new(Message::default());
    ex.set_property("p", "pv");
    assert_eq!(
        expr.evaluate(&mut ex).await.expect("eval"),
        Value::String("pv".into())
    );
}

/// The script-step engine provides `header()` backed by the snapshot.
#[tokio::test]
async fn script_step_header_reader_works() {
    let expr = RhaiLanguage::new()
        .create_mutating_expression(r#"header("h")"#)
        .expect("header reader must compile");
    let mut msg = Message::default();
    msg.set_header("h", "hv");
    let mut ex = Exchange::new(msg);
    assert_eq!(
        expr.evaluate(&mut ex).await.expect("eval"),
        Value::String("hv".into())
    );
}

/// The rhai-only rejection hint names the working map-assignment surface.
#[tokio::test]
async fn set_property_hint_names_map_assignment() {
    let lang = RhaiLanguage::new();
    let err = match lang.create_expression(r#"set_property("a", "b")"#) {
        Ok(_) => panic!("read-only setter must be rejected at create time"),
        Err(err) => err,
    };
    let text = err.to_string();
    assert!(
        text.contains("headers[") || text.contains("properties["),
        "hint must name a map assignment, got: {text}"
    );
}

/// Rhai exposes an empty body as unit; `body = ()` on a `Bytes` body is a real
/// change and clears the body to `Empty`.
#[tokio::test]
async fn rhai_unit_on_bytes_becomes_empty() {
    let expr = RhaiLanguage::new()
        .create_mutating_expression(r#"body = ()"#)
        .expect("unit body assignment must compile");
    let mut ex = Exchange::new(Message {
        body: Body::Bytes(Bytes::from(vec![1_u8, 2])),
        ..Default::default()
    });
    expr.evaluate(&mut ex).await.expect("eval");
    assert!(matches!(ex.input.body, Body::Empty));
}

/// `body = ()` on an already-`Empty` body is unchanged.
#[tokio::test]
async fn rhai_unit_on_empty_unchanged() {
    let expr = RhaiLanguage::new()
        .create_mutating_expression(r#"body = ()"#)
        .expect("unit body assignment must compile");
    let mut ex = Exchange::new(Message::default());
    expr.evaluate(&mut ex).await.expect("eval");
    assert!(matches!(ex.input.body, Body::Empty));
}

/// The `header()`/`property()` readers observe the PRE-run snapshot: after an
/// in-script map assignment the reader still returns the old value, while the
/// mutated map propagates back to the exchange.
#[tokio::test]
async fn script_reader_sees_pre_run_snapshot() {
    let lang = RhaiLanguage::new();

    let expr = lang
        .create_mutating_expression(
            r#"let a = header("k"); headers["k"] = "new"; let b = header("k"); [a, b]"#,
        )
        .expect("header reader script must compile");
    let mut msg = Message::default();
    msg.set_header("k", "old");
    let mut ex = Exchange::new(msg);
    let val = expr.evaluate(&mut ex).await.expect("eval");
    assert_eq!(
        val,
        Value::Array(vec![
            Value::String("old".into()),
            Value::String("old".into())
        ]),
        "readers must reflect the pre-run snapshot"
    );
    assert_eq!(
        ex.input.headers.get("k"),
        Some(&Value::String("new".into())),
        "the mutated map must propagate"
    );

    let expr = lang
        .create_mutating_expression(
            r#"let a = property("p"); properties["p"] = "newp"; let b = property("p"); [a, b]"#,
        )
        .expect("property reader script must compile");
    let mut ex = Exchange::new(Message::default());
    ex.set_property("p", "old");
    let val = expr.evaluate(&mut ex).await.expect("eval");
    assert_eq!(
        val,
        Value::Array(vec![
            Value::String("old".into()),
            Value::String("old".into())
        ]),
        "readers must reflect the pre-run snapshot"
    );
    assert_eq!(
        ex.properties.get("p"),
        Some(&Value::String("newp".into())),
        "the mutated map must propagate"
    );
}

/// Same-value assignment preserves the stored type: assigning the integer `1`
/// writes nothing, while assigning `1.0` is a type-sensitive int-to-float
/// change and is written back (only the observable value is asserted; the
/// stored handle is not observable).
#[tokio::test]
async fn same_value_property_assignment_is_noop() {
    let lang = RhaiLanguage::new();

    let expr = lang
        .create_mutating_expression(r#"properties["p"] = 1"#)
        .expect("int assignment must compile");
    let mut ex = Exchange::new(Message::default());
    ex.set_property("p", Value::from(1));
    expr.evaluate(&mut ex).await.expect("eval");
    let stored = ex.properties.get("p").expect("p must remain");
    assert_eq!(stored.as_i64(), Some(1), "integer must stay an integer");

    let expr = lang
        .create_mutating_expression(r#"properties["p"] = 1.0"#)
        .expect("float assignment must compile");
    let mut ex = Exchange::new(Message::default());
    ex.set_property("p", Value::from(1));
    expr.evaluate(&mut ex).await.expect("eval");
    let stored = ex.properties.get("p").expect("p must remain");
    assert_eq!(
        stored.as_f64(),
        Some(1.0),
        "int-to-float change must be written back as 1.0"
    );
    assert_eq!(
        stored.as_i64(),
        None,
        "the int-to-float change is type-sensitive"
    );
}

// --- Task 3.4: reject read-only assignments (conservative) ------------------

/// `headers["x"] = "1"` on the read-only surface.
#[tokio::test]
async fn read_only_index_assignment_rejected() {
    assert_read_only_assignment_rejected(r#"headers["x"] = "1""#);
}

/// `body = "changed"` on the read-only surface.
#[tokio::test]
async fn read_only_body_assignment_rejected() {
    assert_read_only_assignment_rejected(r#"body = "changed""#);
}

/// Nested index target: `properties["a"]["b"] = "c"`.
#[tokio::test]
async fn read_only_nested_assignment_rejected() {
    assert_read_only_assignment_rejected(r#"properties["a"]["b"] = "c""#);
}

/// Compound `+=` on a read-only root.
#[tokio::test]
async fn read_only_compound_update_rejected() {
    assert_read_only_assignment_rejected(r#"headers["x"] += 1"#);
}

/// Dot target: `headers.foo = "1"` (root resolution descends `Expr::Dot`).
#[tokio::test]
async fn read_only_dot_assignment_rejected() {
    assert_read_only_assignment_rejected(r#"headers.foo = "1""#);
}

/// Conservative: a local that shadows a read-only root is rejected too; no
/// scope analysis is attempted (documented false positive).
#[tokio::test]
async fn read_only_shadowing_local_rejected() {
    assert_read_only_assignment_rejected(r#"let headers = #{}; headers["x"] = 1"#);
}

/// A nested assignment inside a constant-foldable block is still caught; the
/// walk runs on the unoptimized (`OptimizationLevel::None`) AST, so the
/// optimizer cannot hide or drop the assignment before validation.
#[tokio::test]
async fn read_only_assignment_in_constant_branch_rejected() {
    assert_read_only_assignment_rejected(r#"if true { headers["x"] = "y" } true"#);
}

/// Aliasing a read-only root is fine: `let x = headers; x` performs no
/// assignment to a forbidden root.
#[tokio::test]
async fn read_only_alias_is_not_rejected() {
    let expr = RhaiLanguage::new()
        .create_expression(r#"let x = headers; x"#)
        .expect("aliasing a read-only root must compile");
    let mut ex = Exchange::new(Message::default());
    ex.input
        .headers
        .insert("k".to_string(), Value::String("v".into()));
    assert_eq!(
        expr.evaluate(&ex).await.expect("eval"),
        Value::Object(
            [("k".to_string(), Value::String("v".into()))]
                .into_iter()
                .collect()
        )
    );
    // The exchange is unchanged: a read-only alias is a copy, not a handle.
    assert_eq!(ex.input.headers.get("k"), Some(&Value::String("v".into())));
}

/// Plain index reads and pure property/method reads stay accepted.
#[tokio::test]
async fn read_only_plain_read_is_ok() {
    let expr = RhaiLanguage::new()
        .create_expression(r#"headers["x"]"#)
        .expect("index read must compile");
    let mut msg = Message::default();
    msg.set_header("x", "v");
    let ex = Exchange::new(msg);
    assert_eq!(
        expr.evaluate(&ex).await.expect("eval"),
        Value::String("v".into())
    );
}

/// A pure method/property read (`body.len`) is not an assignment and stays
/// accepted; `len` is a registered string getter.
#[tokio::test]
async fn read_only_pure_method_ok() {
    let expr = RhaiLanguage::new()
        .create_expression("body.len")
        .expect("body.len must compile");
    let ex = Exchange::new(Message::new("hello"));
    assert_eq!(expr.evaluate(&ex).await.expect("eval"), Value::from(5));
}

/// The mutating (script-step) surface still accepts the same map assignment
/// and propagates it back to the exchange.
#[tokio::test]
async fn mutating_assignment_still_works() {
    let expr = RhaiLanguage::new()
        .create_mutating_expression(r#"headers["x"] = "1""#)
        .expect("mutating assignment must compile");
    let mut ex = Exchange::new(Message::default());
    expr.evaluate(&mut ex).await.expect("eval");
    assert_eq!(ex.input.headers.get("x"), Some(&Value::String("1".into())));
}

/// Task 3.3 minor: the script-step engine registers `header()`/`property()`
/// only; `set_property()` stays unregistered, so a script call fails at eval
/// with a `FunctionNotFound` error (no new `set_*` surface).
#[tokio::test]
async fn mutating_set_property_is_function_not_found() {
    let expr = RhaiLanguage::new()
        .create_mutating_expression(r#"set_property("a", "b")"#)
        .expect("script compiles; rejection is dynamic, not compile-time");
    let mut ex = Exchange::new(Message::default());
    let err = expr
        .evaluate(&mut ex)
        .await
        .expect_err("set_property must not resolve");
    match &err {
        LanguageError::EvalFailure { class, detail, .. } => {
            assert_eq!(
                *class,
                ExpressionErrorClass::FunctionNotFound,
                "expected FunctionNotFound, got: {err:?}"
            );
            assert_eq!(
                detail.as_deref(),
                Some("function not found"),
                "detail must be the redacted static string"
            );
        }
        other => panic!("expected EvalFailure FunctionNotFound, got: {other:?}"),
    }
}
