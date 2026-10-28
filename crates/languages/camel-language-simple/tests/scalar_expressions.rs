//! Integration corpus for Simple bounded scalar expressions
//! (mission359 Task 1.3).
//!
//! Every test here owns one scenario from the blessed delta
//! `openspec/changes/simple-scalar-expressions/specs/language-value-boundary/spec.md`
//! or one camel-112 corpus case. Entrypoints follow the task convention:
//! predicates through `SimpleLanguage::create_predicate`, expressions and
//! templates through `create_expression` (whole-slot sources keep native
//! values), and parse-only failures through `create_predicate`.

use camel_language_api::{Body, Exchange, ExpressionErrorClass, Language, LanguageError, Value};
use camel_language_simple::SimpleLanguage;
use serde_json::json;

fn lang() -> SimpleLanguage {
    SimpleLanguage::new()
}

fn s(value: &str) -> Value {
    Value::String(value.to_string())
}

fn with_header(mut ex: Exchange, key: &str, value: Value) -> Exchange {
    ex.input.set_header(key, value);
    ex
}

fn with_property(mut ex: Exchange, key: &str, value: Value) -> Exchange {
    ex.set_property(key, value);
    ex
}

fn with_body(mut ex: Exchange, body: Body) -> Exchange {
    ex.input.body = body;
    ex
}

async fn eval_expr(src: &str, ex: &Exchange) -> Result<Value, LanguageError> {
    lang()
        .create_expression(src)
        .expect("expression source must compile")
        .evaluate(ex)
        .await
}

fn creation_error<T>(result: Result<T, LanguageError>) -> LanguageError {
    match result {
        Ok(_) => panic!("expected creation to fail"),
        Err(err) => err,
    }
}

fn assert_parse_error(err: &LanguageError) {
    assert!(
        matches!(err, LanguageError::ParseError { .. }),
        "expected parse error, got: {err:?}"
    );
}

fn assert_arithmetic_error(err: LanguageError) {
    assert!(
        matches!(
            err,
            LanguageError::EvalFailure {
                class: ExpressionErrorClass::Arithmetic,
                ..
            }
        ),
        "expected arithmetic failure, got: {err:?}"
    );
}

// --- Blessed delta scenarios -------------------------------------------------

/// Scenario: Precedence and parentheses
#[tokio::test]
async fn precedence_and_parentheses() {
    let lang = SimpleLanguage::new();
    let src = "int(header.a) + 2 * 3 >= int(header.b) - 1 && property.c || property.d";
    let pred = lang
        .create_predicate(src)
        .expect("precedence source must parse");

    let mut ex = Exchange::default();
    ex.input.set_header("a", s("1"));
    ex.input.set_header("b", s("7"));
    ex.set_property("c", false);
    ex.set_property("d", true);
    assert!(pred.matches(&ex).await.unwrap());

    // `2 * 3` groups before the sum.
    assert_eq!(
        eval_expr("${int(header.a) + 2 * 3}", &ex).await.unwrap(),
        json!(7)
    );
    // Parentheses regroup the sum before the product.
    assert_eq!(
        eval_expr("${(int(header.a) + 2) * 3}", &ex).await.unwrap(),
        json!(9)
    );

    // Discriminating precedence cases: an inverted `&&`/`||` order would flip
    // each of these results.
    for (src, expected) in [
        ("false && true || true", true),
        ("true || true && false", true),
        ("true || false && false", true),
        ("false && false || true", true),
        ("1 + 1 == 2", true),
        ("1 + 1 == 2 && true", true),
    ] {
        let pred = lang
            .create_predicate(src)
            .expect("precedence source must parse");
        assert_eq!(
            pred.matches(&Exchange::default()).await.unwrap(),
            expected,
            "predicate {src:?}"
        );
    }
}

/// Scenario: Comparison does not chain
#[tokio::test]
async fn comparison_does_not_chain() {
    let err = creation_error(lang().create_predicate("1 < 2 < 3"));
    assert_parse_error(&err);
}

/// Scenario: Deferred constructs are parse errors
#[tokio::test]
async fn deferred_constructs_rejected() {
    let cases = [
        ("1.5", "floats are deferred"),
        ("7 / 2", "division is deferred"),
        ("10 % 3", "modulo is deferred"),
        ("'a' + 'b'", "string concatenation is deferred"),
        ("x ? y : z", "ternary is deferred"),
        ("n in (1, 2)", "in is deferred"),
        ("a ?? b", "null-coalescing is deferred"),
        ("2 ** 3", "exponentiation is deferred"),
    ];
    for (src, reason) in cases {
        let err = creation_error(lang().create_predicate(src));
        assert!(
            matches!(err, LanguageError::ParseError { .. }),
            "source {src:?} must be a parse error, got: {err:?}"
        );
        assert!(
            err.to_string().contains(reason),
            "source {src:?} must name {reason:?}, got: {err}"
        );
    }
}

/// Scenario: Bare identifier and clock call are rejected
#[tokio::test]
async fn bare_identifier_and_clock_rejected() {
    for src in ["x == 1", "now() > 1000"] {
        let err = creation_error(lang().create_predicate(src));
        assert_parse_error(&err);
    }
    assert!(lang().create_predicate("property.x == 1").is_ok());
}

/// Scenario: Accepted calls parse
#[tokio::test]
async fn accepted_calls_parse() {
    let src = "min(int(header.base), int(header.cap)) <= int(property.nowMs) \
               && default(property.giveUpAt, '0') != 'null'";
    assert!(lang().create_predicate(src).is_ok());
}

/// Scenario: Integer literal range boundaries
#[tokio::test]
async fn integer_literal_range_boundaries() {
    let lang = SimpleLanguage::new();
    assert!(
        lang.create_predicate("9223372036854775807 == 9223372036854775807")
            .is_ok()
    );
    assert!(
        lang.create_predicate("-9223372036854775808 == -9223372036854775808")
            .is_ok()
    );
    assert_eq!(
        eval_expr("${9223372036854775807}", &Exchange::default())
            .await
            .unwrap(),
        json!(i64::MAX)
    );
    assert_eq!(
        eval_expr("${-9223372036854775808}", &Exchange::default())
            .await
            .unwrap(),
        json!(i64::MIN)
    );

    for src in ["9223372036854775808", "99999999999999999999"] {
        let err = creation_error(lang.create_predicate(src));
        assert_parse_error(&err);
    }

    assert_arithmetic_error(
        eval_expr("${9223372036854775807 + 1}", &Exchange::default())
            .await
            .unwrap_err(),
    );
    assert_arithmetic_error(
        eval_expr("${-9223372036854775808 - 1}", &Exchange::default())
            .await
            .unwrap_err(),
    );
}

/// Scenario: Integer conversion from numeric strings
#[tokio::test]
async fn int_from_numeric_strings() {
    let ex = with_header(Exchange::default(), "count", s("42"));
    assert_eq!(
        eval_expr("${int(header.count)}", &ex).await.unwrap(),
        json!(42)
    );

    let ex = with_property(Exchange::default(), "nowMs", s("1750000000000"));
    assert_eq!(
        eval_expr("${int(property.nowMs)}", &ex).await.unwrap(),
        json!(1_750_000_000_000i64)
    );
}

/// Scenario: Malformed and out-of-range conversion is a catchable error
#[tokio::test]
async fn malformed_conversion_catchable() {
    for value in ["abc", " 7", "1.5", "18446744073709551615"] {
        let ex = with_header(Exchange::default(), "v", s(value));
        let err = eval_expr("${int(header.v)}", &ex).await.unwrap_err();
        assert!(
            matches!(err, LanguageError::ConversionError { .. }),
            "value {value:?} must fail conversion, got: {err:?}"
        );
    }

    let ex = with_header(Exchange::default(), "v", Value::Bool(true));
    let err = eval_expr("${int(header.v)}", &ex).await.unwrap_err();
    assert!(
        matches!(err, LanguageError::ConversionError { .. }),
        "{err:?}"
    );

    let ex = with_property(
        Exchange::default(),
        "u64val",
        Value::Number(serde_json::Number::from(u64::MAX)),
    );
    let err = eval_expr("${int(property.u64val)}", &ex).await.unwrap_err();
    assert!(
        matches!(err, LanguageError::ConversionError { .. }),
        "{err:?}"
    );
}

/// Scenario: Checked arithmetic catches overflow
#[tokio::test]
async fn checked_arithmetic_overflow() {
    let ex = with_property(Exchange::default(), "n", s(&i64::MAX.to_string()));
    assert_arithmetic_error(eval_expr("${int(property.n) + 1}", &ex).await.unwrap_err());
}

/// Scenario: Arithmetic rejects non-integer operands
#[tokio::test]
async fn arithmetic_rejects_non_integers() {
    let ex = with_header(Exchange::default(), "f", s("1.5"));
    let err = eval_expr("${header.f * 2}", &ex).await.unwrap_err();
    assert!(matches!(err, LanguageError::TypeMismatch { .. }), "{err:?}");

    let ex = with_property(Exchange::default(), "flag", Value::Bool(true));
    let err = eval_expr("${property.flag + 1}", &ex).await.unwrap_err();
    assert!(matches!(err, LanguageError::TypeMismatch { .. }), "{err:?}");
}

/// Scenario: String conversion rules
#[tokio::test]
async fn string_conversion_rules() {
    let ex = with_header(Exchange::default(), "h", s("x"));
    assert_eq!(
        eval_expr("${string(header.h)}", &ex).await.unwrap(),
        json!("x")
    );

    let ex = with_header(
        Exchange::default(),
        "n",
        Value::Number(serde_json::Number::from(42)),
    );
    assert_eq!(
        eval_expr("${string(header.n)}", &ex).await.unwrap(),
        json!("42")
    );

    let ex = with_property(Exchange::default(), "flag", Value::Bool(true));
    assert_eq!(
        eval_expr("${string(property.flag)}", &ex).await.unwrap(),
        json!("true")
    );

    // An unset header maps to the empty string.
    assert_eq!(
        eval_expr("${string(header.missing)}", &Exchange::default())
            .await
            .unwrap(),
        json!("")
    );

    let ex = with_property(Exchange::default(), "arr", Value::Array(vec![json!(1)]));
    let err = eval_expr("${string(property.arr)}", &ex).await.unwrap_err();
    assert!(
        matches!(err, LanguageError::ConversionError { .. }),
        "{err:?}"
    );
}

/// Scenario: Default is lazy and treats null-like values as missing
#[tokio::test]
async fn default_lazy_missing_rules() {
    // Unset first operand selects the fallback.
    assert_eq!(
        eval_expr("${default(property.giveUpAt, '100')}", &Exchange::default())
            .await
            .unwrap(),
        json!("100")
    );

    // `""` and `"null"` are missing; `"0"` and `false` are present.
    for value in ["", "null"] {
        let ex = with_header(Exchange::default(), "flag", s(value));
        assert_eq!(
            eval_expr("${default(header.flag, 'x')}", &ex)
                .await
                .unwrap(),
            json!("x"),
            "value {value:?}"
        );
    }
    let ex = with_header(Exchange::default(), "flag", s("0"));
    assert_eq!(
        eval_expr("${default(header.flag, 'x')}", &ex)
            .await
            .unwrap(),
        json!("0")
    );
    let ex = with_header(Exchange::default(), "flag", Value::Bool(false));
    assert_eq!(
        eval_expr("${default(header.flag, 'x')}", &ex)
            .await
            .unwrap(),
        Value::Bool(false)
    );

    // A present first operand keeps the fallback unevaluated.
    let ex = with_header(
        with_header(Exchange::default(), "flag", s("present")),
        "bad",
        s("abc"),
    );
    assert_eq!(
        eval_expr("${default(header.flag, int(header.bad))}", &ex)
            .await
            .unwrap(),
        json!("present")
    );

    // Missing first operand evaluates the failing fallback.
    let ex = with_header(Exchange::default(), "bad", s("abc"));
    let err = eval_expr("${default(header.flag, int(header.bad))}", &ex)
        .await
        .unwrap_err();
    assert!(
        matches!(err, LanguageError::ConversionError { .. }),
        "{err:?}"
    );

    // An erroring first operand propagates.
    let err = eval_expr("${default(int(header.bad), 'x')}", &ex)
        .await
        .unwrap_err();
    assert!(
        matches!(err, LanguageError::ConversionError { .. }),
        "{err:?}"
    );
}

/// Scenario: Min and max select on i64
#[tokio::test]
async fn min_max_select() {
    let base = Exchange::default();
    let ex = with_header(with_header(base, "base", s("500")), "cap", s("3000"));
    assert_eq!(
        eval_expr("${min(int(header.base) * 2, int(header.cap))}", &ex)
            .await
            .unwrap(),
        json!(1000)
    );
    assert_eq!(
        eval_expr("${max(int(header.base), int(header.cap))}", &ex)
            .await
            .unwrap(),
        json!(3000)
    );
}

/// Scenario: Equality never throws on type mismatch
#[tokio::test]
async fn equality_no_throw_type_mismatch() {
    let ex = with_property(Exchange::default(), "flag", Value::Bool(true));
    let ex = with_header(ex, "ok", s("true"));
    assert_eq!(
        eval_expr("${property.flag == header.ok}", &ex)
            .await
            .unwrap(),
        json!(false)
    );
    assert_eq!(
        eval_expr("${property.flag != header.ok}", &ex)
            .await
            .unwrap(),
        json!(true)
    );
}

/// Scenario: Null equality and the unset-vs-empty distinction
#[tokio::test]
async fn null_equality_unset_vs_empty() {
    let ex = with_header(Exchange::default(), "b", s(""));
    assert_eq!(
        eval_expr("${header.a == header.a}", &ex).await.unwrap(),
        json!(true)
    );
    assert_eq!(
        eval_expr("${header.a == header.b}", &ex).await.unwrap(),
        json!(false)
    );
    assert_eq!(
        eval_expr("${header.a != header.b}", &ex).await.unwrap(),
        json!(true)
    );
}

/// Scenario: Numeric string coercion is exact beyond float precision
#[tokio::test]
async fn numeric_string_exact_beyond_f64() {
    let ex = with_header(Exchange::default(), "big", s("9007199254740993"));
    let ex = with_header(ex, "n", s("42"));
    assert_eq!(
        eval_expr("${header.big == 9007199254740993}", &ex)
            .await
            .unwrap(),
        json!(true)
    );
    assert_eq!(
        eval_expr("${header.n == 43}", &ex).await.unwrap(),
        json!(false)
    );
    assert_eq!(
        eval_expr("${header.big == 9007199254740992}", &ex)
            .await
            .unwrap(),
        json!(false)
    );
}

/// Scenario: Ordering rules
#[tokio::test]
async fn ordering_rules() {
    // Null operands make ordering false.
    assert_eq!(
        eval_expr("${header.n > 1}", &Exchange::default())
            .await
            .unwrap(),
        json!(false)
    );

    let ex = with_header(with_header(Exchange::default(), "a", s("10")), "b", s("9"));
    assert_eq!(
        eval_expr("${header.a > header.b}", &ex).await.unwrap(),
        json!(true)
    );

    let ex = with_header(
        with_header(Exchange::default(), "w", s("apple")),
        "v",
        s("9"),
    );
    assert_eq!(
        eval_expr("${header.w > header.v}", &ex).await.unwrap(),
        json!(true)
    );

    let ex = with_header(Exchange::default(), "dec", s("1.5"));
    let err = eval_expr("${header.dec > 1}", &ex).await.unwrap_err();
    assert!(matches!(err, LanguageError::TypeMismatch { .. }), "{err:?}");

    let ex = with_header(Exchange::default(), "flag", Value::Bool(true));
    let err = eval_expr("${header.flag > 0}", &ex).await.unwrap_err();
    assert!(matches!(err, LanguageError::TypeMismatch { .. }), "{err:?}");
}

/// Scenario: Strict boolean results stay
#[tokio::test]
async fn strict_boolean_results_stay() {
    let ex = with_header(Exchange::default(), "n", s("41"));
    let pred = lang()
        .create_predicate("int(header.n) + 1")
        .expect("arithmetic predicate must parse");
    let err = pred.matches(&ex).await.unwrap_err();
    assert!(matches!(err, LanguageError::TypeMismatch { .. }), "{err:?}");

    // `&&` short-circuits: the right side is never evaluated.
    let pred = lang()
        .create_predicate("false && header.missing > 1")
        .expect("short-circuit predicate must parse");
    assert!(!pred.matches(&Exchange::default()).await.unwrap());

    let ex = with_header(Exchange::default(), "bad", s("abc"));
    let pred = lang()
        .create_predicate("false && int(header.bad) > 1")
        .expect("short-circuit predicate must parse");
    assert!(!pred.matches(&ex).await.unwrap());
}

/// Scenario: Parse-time limits
#[tokio::test]
async fn parse_time_limits() {
    let lang = SimpleLanguage::new();

    // Valid long sum chain > 16 KiB: only the source-size limit can reject it.
    let oversized = format!("{}1", "1 + ".repeat(4200));
    let err = creation_error(lang.create_predicate(&oversized));
    assert_parse_error(&err);
    assert!(
        err.to_string().contains("source exceeds 16 KiB limit"),
        "expected source-size limit, got: {err}"
    );

    // Valid balanced doubling tree: nodes > 512 while depth stays logarithmic.
    let mut tree = "header.a + header.b".to_string();
    for _ in 0..8 {
        tree = format!("({tree} + {tree})");
    }
    let err = creation_error(lang.create_predicate(&tree));
    assert_parse_error(&err);
    assert!(
        err.to_string().contains("AST exceeds 512 node limit"),
        "expected node limit, got: {err}"
    );

    // 65 nested parentheses exceed the depth limit; 64 parse.
    let nested_65 = format!("{}header.a{}", "(".repeat(65), ")".repeat(65));
    let err = creation_error(lang.create_predicate(&nested_65));
    assert_parse_error(&err);
    assert!(
        err.to_string().contains("64 depth limit"),
        "expected depth limit, got: {err}"
    );

    let nested_64 = format!("{}header.a{}", "(".repeat(64), ")".repeat(64));
    assert!(lang.create_predicate(&nested_64).is_ok());
}

/// Scenario: Output limit at evaluation
#[tokio::test]
async fn output_limit_at_evaluation() {
    let big = "A".repeat(1_048_577);
    let ex = with_header(Exchange::default(), "big", Value::String(big));
    let err = eval_expr("x${header.big}", &ex).await.unwrap_err();
    assert!(
        matches!(
            err,
            LanguageError::EvalFailure {
                class: ExpressionErrorClass::Limit,
                ..
            }
        ),
        "{err:?}"
    );
    // Payload-free diagnostics: no exchange data leaks into the error text.
    assert!(!err.to_string().contains(&"A".repeat(16)), "{err}");
}

/// Scenario: Node-limited source evaluates within the work budget
#[tokio::test]
async fn node_limited_source_within_work_budget() {
    // 160 slots count 481 AST nodes (one Interpolated plus three per slot),
    // close to the 512-node ceiling while staying far under 4096 work units.
    let src = "${int(header.n)}".repeat(160);
    let expr = lang()
        .create_expression(&src)
        .expect("481-node template must parse");
    let ex = with_header(Exchange::default(), "n", s("1"));
    let val = expr.evaluate(&ex).await.unwrap();
    assert_eq!(val, json!("1".repeat(160)));
}

/// Scenario: Unrecognized slot is a compile error
#[tokio::test]
async fn unrecognized_slot_compile_error() {
    let err = creation_error(lang().create_expression("${unknown}"));
    assert_parse_error(&err);
}

/// Scenario: Unknown language is an evaluation failure
#[tokio::test]
async fn unknown_language_evaluation_failure() {
    let expr = lang()
        .create_expression("${nope:x}")
        .expect("unknown language must compile");
    let err = expr.evaluate(&Exchange::default()).await.unwrap_err();
    assert!(matches!(err, LanguageError::EvalError(_)), "{err:?}");
}

/// Scenario: Slot expression evaluates
#[tokio::test]
async fn slot_expression_evaluates() {
    let ex = with_property(Exchange::default(), "nowMs", s("1750000000000"));
    let ex = with_header(ex, "delay", s("500"));
    assert_eq!(
        eval_expr("retry-at-${int(property.nowMs) + int(header.delay)}", &ex)
            .await
            .unwrap(),
        json!("retry-at-1750000000500")
    );
}

/// Scenario: Nested slots are a parse error
#[tokio::test]
async fn nested_slots_parse_error() {
    let err = creation_error(lang().create_expression("${default(header.x, '${header.y}')}"));
    assert_parse_error(&err);
}

/// Scenario: Whole-slot expressions keep native results
#[tokio::test]
async fn whole_slot_native_results() {
    let ex = with_header(Exchange::default(), "n", s("21"));
    assert_eq!(
        eval_expr("${int(header.n) * 2}", &ex).await.unwrap(),
        json!(42)
    );
}

// --- camel-112 corpus fidelity ----------------------------------------------

/// camel-112 corpus §1: due check short-circuits on skip and compares epoch
/// milliseconds exactly (`nowMs` property; `at` falls back to 0).
#[tokio::test]
async fn corpus_due_check() {
    let lang = SimpleLanguage::new();
    let src = "property.skipHandled != true \
               && int(property.nowMs) >= int(default(body, '0'))";
    let pred = lang.create_predicate(src).expect("due check must parse");

    let base = with_property(Exchange::default(), "nowMs", s("1750000000000"));
    let base = with_property(base, "skipHandled", Value::Bool(false));

    for body in [Body::Text(String::new()), Body::Text("null".to_string())] {
        assert!(
            pred.matches(&with_body(base.clone(), body)).await.unwrap(),
            "empty/null body defaults `at` to 0"
        );
    }

    // One millisecond after `nowMs` is not yet due; 1.8e12 range stays exact.
    let after = with_body(base, Body::Text("1750000000001".to_string()));
    assert!(!pred.matches(&after).await.unwrap());
}

/// camel-112 corpus §1: retry-at deadline rendered through a template.
#[tokio::test]
async fn corpus_retry_at_template() {
    let ex = with_property(Exchange::default(), "nowMs", s("1750000000000"));
    let ex = with_header(ex, "delay", s("500"));
    assert_eq!(
        eval_expr(
            "retry-at-${string(int(property.nowMs) + int(header.delay))}",
            &ex
        )
        .await
        .unwrap(),
        json!("retry-at-1750000000500")
    );
}

/// camel-112 corpus §1: alert threshold comparison.
#[tokio::test]
async fn corpus_limit_compare() {
    let lang = SimpleLanguage::new();
    let ex = with_header(Exchange::default(), "n", s("5"));
    let ex = with_header(ex, "limit", s("3"));
    let ex = with_header(ex, "alertAttempts", s("6"));

    let over_limit = lang
        .create_predicate("int(header.n) >= int(header.limit)")
        .expect("limit compare must parse");
    assert!(over_limit.matches(&ex).await.unwrap());

    let over_alert = lang
        .create_predicate("int(header.n) >= int(header.alertAttempts)")
        .expect("alert compare must parse");
    assert!(!over_alert.matches(&ex).await.unwrap());
}

/// camel-112 corpus §2: existing Redis key templates need no new grammar.
#[tokio::test]
async fn corpus_redis_key_templates() {
    let ex = with_property(Exchange::default(), "region", s("emea"));
    let ex = with_header(ex, "inboxKey", s("112:emea:inbox"));

    for (src, expected) in [
        ("112:${property.region}:inbox", "112:emea:inbox"),
        ("${header.inboxKey}:retry-at", "112:emea:inbox:retry-at"),
        (
            "112:${property.region}:outbox:attempts",
            "112:emea:outbox:attempts",
        ),
    ] {
        assert_eq!(
            eval_expr(src, &ex).await.unwrap(),
            json!(expected),
            "template {src:?}"
        );
    }
}

/// camel-112 corpus §3: null-safe shapes (empty/null body, unset property,
/// unset header short-circuit).
#[tokio::test]
async fn corpus_null_safe_shapes() {
    let lang = SimpleLanguage::new();

    let body_guard = lang
        .create_predicate("body != '' && body != 'null'")
        .expect("body guard must parse");
    for (body, expected) in [
        (Body::Text("x".to_string()), true),
        (Body::Text(String::new()), false),
        (Body::Text("null".to_string()), false),
    ] {
        let ex = with_body(Exchange::default(), body);
        assert_eq!(
            body_guard.matches(&ex).await.unwrap(),
            expected,
            "body shape"
        );
    }

    // Unset property compared to a boolean is false, never an error.
    let flag = lang
        .create_predicate("property.flag == true")
        .expect("flag guard must parse");
    assert!(!flag.matches(&Exchange::default()).await.unwrap());

    let ex = with_header(Exchange::default(), "n", s("5"));
    let ex = with_header(ex, "limit", s("3"));
    let outcome = lang
        .create_predicate("header.outcome == 'unknown' && int(header.n) >= int(header.limit)")
        .expect("outcome guard must parse");
    // The numeric half is true; the false equality short-circuits to false.
    assert!(!outcome.matches(&ex).await.unwrap());
    let numeric_half = lang
        .create_predicate("int(header.n) >= int(header.limit)")
        .expect("numeric half must parse");
    assert!(numeric_half.matches(&ex).await.unwrap());
}

/// Binding ruling deviation (design `simple-scalar-expressions`): null stays
/// equal to null, so the corpus's two-unset guard is NOT implemented as
/// `body == property.skipXml`; callers use explicit presence guards.
#[tokio::test]
async fn two_unset_presence_guard() {
    let lang = SimpleLanguage::new();
    let raw = lang
        .create_predicate("body == property.skipXml")
        .expect("raw equality must parse");
    assert!(
        raw.matches(&Exchange::default()).await.unwrap(),
        "two unset operands compare equal (null == null stays true)"
    );

    let guard = lang
        .create_predicate("body != null && property.skipXml != null && body == property.skipXml")
        .expect("presence guard must parse");
    assert!(
        !guard.matches(&Exchange::default()).await.unwrap(),
        "caller-side presence guard rejects two unset values"
    );

    let ex = with_property(Exchange::default(), "skipXml", s("x"));
    assert!(
        !raw.matches(&ex).await.unwrap(),
        "one side set is unequal to unset body"
    );
}

/// camel-112 corpus §1 backoff comment: capped exponential backoff
/// (`2^(n-1)`) and JSON construction stay in rhai; Simple rejects them.
#[tokio::test]
async fn deferred_cases_documented() {
    let lang = SimpleLanguage::new();

    let backoff = "min(int(header.base) * 2^(header.n - 1), int(header.cap))";
    match lang.create_predicate(backoff) {
        Ok(_) => panic!("backoff must be a parse error"),
        Err(err) => assert_parse_error(&err),
    }

    // A JSON/map construction with a dynamic value needs an expression inside
    // the braces, which the bounded grammar does not accept.
    for json_source in ["{'a': int(header.n)}", "{\"a\": int(header.n)}"] {
        match lang.create_predicate(json_source) {
            Ok(_) => panic!("json source {json_source:?} must be a parse error"),
            Err(err) => assert_parse_error(&err),
        }
    }
}
