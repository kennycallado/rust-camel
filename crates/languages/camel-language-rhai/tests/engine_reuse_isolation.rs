//! Engine-reuse isolation and semantics (change `rhai-engine-reuse`, task 1.3).
//!
//! Step 1 builds a fresh engine per evaluation and shares only immutable
//! prebuilt modules (the standard package, the JSON host module and the stream
//! guards). No mutable per-evaluation state is retained on the language
//! object, so residue after ANY unwind — including a hypothetical engine panic
//! — is structurally impossible. These tests pin the observable abort paths
//! (throw, early return, host error, operations limit, timeout) and the
//! per-evaluation semantics (pre-run readers, stream counters, transactional
//! write-back and interleaved evaluations) so a future engine cache that
//! crosses the isolation boundary fails loudly.

use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use bytes::Bytes;
use camel_api::error::CamelError;
use camel_api::{Body, StreamBody};
use camel_language_api::{
    Exchange, ExpressionErrorClass, Language, LanguageError, Message, MutatingExpression,
    RhaiLimitsConfig, Value,
};
use camel_language_rhai::RhaiLanguage;
use futures::stream::{BoxStream, StreamExt};

const SENTINEL: &str = "sentinel";

/// Evaluate the `properties` map on a FRESH exchange with a FRESH expression
/// object and assert the sentinel key never appears.
async fn assert_no_sentinel_in_fresh_properties(lang: &RhaiLanguage) {
    let expr = lang
        .create_expression("properties")
        .expect("properties map read must compile");
    let ex = Exchange::new(Message::default());
    let value = expr.evaluate(&ex).await.expect("fresh eval must succeed");
    let Value::Object(map) = value else {
        panic!("`properties` must evaluate to a map, got: {value:?}");
    };
    assert!(
        !map.contains_key(SENTINEL),
        "no sentinel residue expected, got keys: {:?}",
        map.keys().collect::<Vec<_>>()
    );
}

/// Rhai scripts are panic-free by design — arithmetic overflow, division by
/// zero and bad indexing are typed errors — so `throw` is the scriptable
/// abrupt-termination path (the "deliberate panic" scenario of the spec).
#[tokio::test]
async fn throw_abort_leaves_no_sentinel() {
    let lang = RhaiLanguage::new();
    let expr = lang
        .create_mutating_expression(r#"properties["sentinel"] = "leaked"; throw "boom";"#)
        .expect("throw script must compile");
    let mut ex = Exchange::new(Message::default());
    let err = expr
        .evaluate(&mut ex)
        .await
        .expect_err("throw must abort evaluation");
    assert!(
        matches!(
            &err,
            LanguageError::EvalFailure {
                class: ExpressionErrorClass::Runtime,
                ..
            }
        ),
        "throw must surface as a Runtime failure, got: {err:?}"
    );
    assert_no_sentinel_in_fresh_properties(&lang).await;
}

/// The supported early-return path is a normal function-level early exit
/// (`?` is reserved-unimplemented in rhai 1.26); the evaluation SUCCEEDS.
#[tokio::test]
async fn early_return_leaves_no_sentinel() {
    let lang = RhaiLanguage::new();
    let expr = lang
        .create_mutating_expression(
            r#"
            fn setup(props) {
                props["sentinel"] = "leaked";
                if true {
                    return props;
                }
                props["unreached"] = "never";
                props
            }
            properties = setup(properties);
            "#,
        )
        .expect("early-return script must compile");
    let mut ex = Exchange::new(Message::default());
    expr.evaluate(&mut ex)
        .await
        .expect("early return must complete the evaluation");
    assert_eq!(
        ex.properties.get(SENTINEL).and_then(Value::as_str),
        Some("leaked"),
        "the sentinel was written before the early return"
    );
    assert!(
        !ex.properties.contains_key("unreached"),
        "the early return must skip the trailing statements"
    );
    assert_no_sentinel_in_fresh_properties(&lang).await;
}

/// A typed host error (`parse_json` with a non-string argument) aborts the
/// evaluation; no residue survives.
#[tokio::test]
async fn host_error_leaves_no_sentinel() {
    let lang = RhaiLanguage::new();
    let expr = lang
        .create_mutating_expression(r#"properties["sentinel"] = "leaked"; parse_json(42)"#)
        .expect("host-error script must compile");
    let mut ex = Exchange::new(Message::default());
    let err = expr
        .evaluate(&mut ex)
        .await
        .expect_err("non-string parse_json must abort evaluation");
    assert!(
        matches!(
            &err,
            LanguageError::TypeMismatch {
                expected,
                actual,
                ..
            } if expected == "string" && actual == "i64"
        ),
        "parse_json(42) must surface as a TypeMismatch, got: {err:?}"
    );
    assert_no_sentinel_in_fresh_properties(&lang).await;
}

/// Operations-limit abort on a tiny `max_operations`; no residue survives.
#[tokio::test]
async fn operation_abort_leaves_no_sentinel() {
    let lang = RhaiLanguage::with_limits(RhaiLimitsConfig {
        max_operations: Some(50),
        ..Default::default()
    });
    let expr = lang
        .create_mutating_expression(r#"properties["sentinel"] = "leaked"; while true { }"#)
        .expect("loop script must compile");
    let mut ex = Exchange::new(Message::default());
    let err = expr
        .evaluate(&mut ex)
        .await
        .expect_err("infinite loop must trip max_operations");
    assert!(
        matches!(
            &err,
            LanguageError::EvalFailure {
                class: ExpressionErrorClass::Limit,
                ..
            }
        ),
        "operations limit must surface as a Limit failure, got: {err:?}"
    );
    assert_no_sentinel_in_fresh_properties(&lang).await;
}

/// Captured-map readers stay per-evaluation: `header()` sees the PRE-run
/// snapshot even after the script mutates the map, while the committed
/// exchange carries the mutation.
#[tokio::test]
async fn reader_pre_run_value_after_mutation() {
    let expr = RhaiLanguage::new()
        .create_mutating_expression(
            r#"let before = header("h1"); headers["h1"] = "new"; let after = header("h1"); [before, after]"#,
        )
        .expect("header reader script must compile");
    let mut msg = Message::default();
    msg.set_header("h1", "v1");
    let mut ex = Exchange::new(msg);
    let value = expr.evaluate(&mut ex).await.expect("eval");
    assert_eq!(
        value,
        Value::Array(vec![Value::String("v1".into()), Value::String("v1".into())]),
        "before and after must both read the pre-run snapshot"
    );
    assert_eq!(
        ex.input.headers.get("h1"),
        Some(&Value::String("new".into())),
        "the mutated map must propagate to the exchange"
    );
}

/// A route timeout must not corrupt the next evaluation. The finite counter
/// loop is seconds of work, so the 50 ms timeout fires first; the finite bound
/// also terminates the surviving blocking worker (no thread leaks past the
/// test). No `std::thread::sleep` — the wait is inside the rhai script.
#[tokio::test]
async fn timeout_does_not_corrupt_next_eval() {
    let lang = RhaiLanguage::with_limits(RhaiLimitsConfig {
        execution_timeout_ms: Some(50),
        max_operations: Some(1_000_000_000),
        ..Default::default()
    });
    let expr = lang
        .create_mutating_expression("let x = 0; while x < 200_000_000 { x += 1; }")
        .expect("slow counter script must compile");
    let mut ex = Exchange::new(Message::default());
    let err = expr
        .evaluate(&mut ex)
        .await
        .expect_err("the route timeout must fire");
    assert!(
        matches!(
            &err,
            LanguageError::EvalFailure {
                class: ExpressionErrorClass::Timeout,
                ..
            }
        ),
        "expected the TIMEOUT class (not an operations limit), got: {err:?}"
    );

    // Immediately evaluate a trivial expression with default limits.
    let fresh = RhaiLanguage::new();
    let trivial = fresh
        .create_expression("1 + 1")
        .expect("trivial expression must compile");
    let ex = Exchange::new(Message::default());
    assert_eq!(
        trivial.evaluate(&ex).await.expect("trivial eval"),
        Value::from(2),
        "a timed-out evaluation must not corrupt the next one"
    );
}

/// Four threads with default limits and four with a tiny `max_operations`
/// evaluate the same 5-clause predicate concurrently; every outcome matches
/// its own limits.
#[test]
fn concurrent_mixed_limits_are_independent() {
    const PREDICATE: &str = r#"header("a") == 1 && header("b") == 2 && header("c") == 3 && header("d") == 4 && header("e") == 5"#;

    fn evaluate_with(limits: RhaiLimitsConfig) -> Result<bool, LanguageError> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        let lang = RhaiLanguage::with_limits(limits);
        let pred = lang
            .create_predicate(PREDICATE)
            .expect("5-clause predicate must compile");
        let mut msg = Message::default();
        msg.set_header("a", 1);
        msg.set_header("b", 2);
        msg.set_header("c", 3);
        msg.set_header("d", 4);
        msg.set_header("e", 5);
        let ex = Exchange::new(msg);
        rt.block_on(pred.matches(&ex))
    }

    let handles: Vec<_> = (0..8)
        .map(|i| {
            std::thread::spawn(move || {
                if i % 2 == 0 {
                    // 4 default-limits threads expect Ok(true).
                    (false, evaluate_with(RhaiLimitsConfig::default()))
                } else {
                    // 4 tiny-limits threads expect an operations-limit error.
                    (
                        true,
                        evaluate_with(RhaiLimitsConfig {
                            max_operations: Some(20),
                            ..Default::default()
                        }),
                    )
                }
            })
        })
        .collect();

    for handle in handles {
        let (tiny, outcome) = handle.join().expect("worker thread must not panic");
        if tiny {
            let err = outcome.expect_err("tiny max_operations must trip on the predicate");
            assert!(
                matches!(
                    &err,
                    LanguageError::EvalFailure {
                        class: ExpressionErrorClass::Limit,
                        ..
                    }
                ),
                "tiny limits must surface Limit, got: {err:?}"
            );
        } else {
            assert!(
                outcome.expect("default limits must evaluate the predicate"),
                "default-limits thread must match"
            );
        }
    }
}

/// Stream refusal counters are fresh per evaluation: a prior materializing
/// refusal must not change the verdict or error text of the next evaluation,
/// which sees only its own forgiving access plus the materializing read.
#[tokio::test]
async fn stream_counters_fresh_per_eval() {
    fn stream_exchange() -> Exchange {
        let stream: BoxStream<'static, Result<Bytes, CamelError>> =
            futures::stream::empty().boxed();
        let msg = Message {
            body: Body::Stream(StreamBody {
                stream: Arc::new(tokio::sync::Mutex::new(Some(stream))),
                metadata: Default::default(),
            }),
            ..Default::default()
        };
        Exchange::new(msg)
    }

    const FORGIVING_THEN_MATERIALIZING: &str =
        "let r = 0; try { body.to_string(); r = 1; } catch (err) { r = 42; }; let x = body; r";

    let lang = RhaiLanguage::new();

    // eval #1: materializing read is refused.
    let materializing = lang
        .create_expression("let x = body; 42")
        .expect("materializing read must compile");
    let err1 = materializing
        .evaluate(&stream_exchange())
        .await
        .expect_err("materializing read must refuse");
    assert!(
        matches!(
            &err1,
            LanguageError::ConversionError { source_type, .. } if source_type == "Body::Stream"
        ),
        "eval #1 must refuse with a Body::Stream conversion error, got: {err1:?}"
    );

    // eval #2: forgiving access first, then the materializing read.
    let seen_eval = lang
        .create_expression(FORGIVING_THEN_MATERIALIZING)
        .expect("forgiving/materializing script must compile");
    let err2 = seen_eval
        .evaluate(&stream_exchange())
        .await
        .expect_err("the raw capture after a caught guard must still refuse");
    assert!(
        matches!(
            &err2,
            LanguageError::ConversionError { source_type, .. } if source_type == "Body::Stream"
        ),
        "eval #2 must refuse with a Body::Stream conversion error, got: {err2:?}"
    );

    // eval #3 (control): a FRESH language instance + fresh expression object +
    // fresh exchange, so it provably never saw eval #1 — identical verdict and
    // error text.
    let control_lang = RhaiLanguage::new();
    let control = control_lang
        .create_expression(FORGIVING_THEN_MATERIALIZING)
        .expect("control script must compile");
    let err3 = control
        .evaluate(&stream_exchange())
        .await
        .expect_err("the control eval must refuse identically");
    assert_eq!(
        err2.to_string(),
        err3.to_string(),
        "refusal text must be identical without inherited stream counters"
    );
    assert!(
        matches!(
            &err3,
            LanguageError::ConversionError { source_type, .. } if source_type == "Body::Stream"
        ),
        "control eval must refuse with a Body::Stream conversion error, got: {err3:?}"
    );
}

/// Mutating eval is a validate-all-then-commit transaction: an error leaves
/// every field untouched; a success commits the map writes and leaves an
/// untouched header byte-identical.
#[tokio::test]
async fn transactional_write_back_unchanged() {
    let lang = RhaiLanguage::new();

    // (a) throw aborts with no write-back.
    let aborting = lang
        .create_mutating_expression(
            r#"headers["h1"] = "m"; properties["out"] = "x"; throw "boom";"#,
        )
        .expect("aborting script must compile");
    let mut msg = Message::new("original-body");
    msg.set_header("h0", "v0");
    msg.set_header("h1", "v1");
    let mut ex = Exchange::new(msg);
    ex.set_property("k0", "p0");
    let body_before = ex.input.body.clone();
    let headers_before = ex.input.headers.clone();
    let properties_before = ex.properties.clone();
    aborting
        .evaluate(&mut ex)
        .await
        .expect_err("throw must fail the transaction");
    assert_eq!(ex.input.body, body_before, "body must be untouched");
    assert_eq!(
        ex.input.headers, headers_before,
        "headers must be untouched"
    );
    assert_eq!(
        ex.properties, properties_before,
        "properties must be untouched"
    );

    // (b) the same script without the throw commits the writes.
    let committing = lang
        .create_mutating_expression(r#"headers["h1"] = "m"; properties["out"] = "x";"#)
        .expect("committing script must compile");
    let mut msg = Message::new("original-body");
    msg.set_header("h0", "v0");
    msg.set_header("h1", "v1");
    let mut ex = Exchange::new(msg);
    committing.evaluate(&mut ex).await.expect("eval");
    assert_eq!(
        ex.input.headers.get("h1"),
        Some(&Value::String("m".into())),
        "the mutated header must commit"
    );
    assert_eq!(
        ex.input.headers.get("h0"),
        Some(&Value::String("v0".into())),
        "an untouched header must stay byte-identical"
    );
    assert_eq!(
        ex.properties.get("out"),
        Some(&Value::String("x".into())),
        "the mutated property must commit"
    );
    assert_eq!(
        ex.input.body,
        Body::Text("original-body".to_string()),
        "the body must be unchanged"
    );
}

/// Evaluations of two expression objects interleaved from two threads leave no
/// residue: the reader never observes the other evaluation's sentinel. Step 1
/// has no shared per-eval snapshot state, so nesting residue is structurally
/// impossible; this test pins the interleaving behavior.
#[test]
fn nested_interleaved_evaluations_leave_no_residue() {
    const ITERATIONS: usize = 50;
    const ROUND_TIMEOUT: Duration = Duration::from_secs(10);

    let lang = RhaiLanguage::new();
    let writer: Arc<Box<dyn MutatingExpression>> = Arc::new(
        lang.create_mutating_expression(r#"properties["sentinel"] = "leaked"; throw "boom";"#)
            .expect("sentinel script must compile"),
    );
    let reader = lang
        .create_expression(r#"[property("k1"), property("sentinel")]"#)
        .expect("reader script must compile");

    // Bounded per-round ping-pong: writer eval -> writer done -> reader eval
    // -> reader done. This establishes the alternating interleaving instead of
    // two free-running loops. recv_timeout (never recv) turns a stall into a
    // clear test failure rather than a hang.
    let (writer_done_tx, writer_done_rx) = mpsc::channel::<()>();
    let (reader_done_tx, reader_done_rx) = mpsc::channel::<()>();

    let worker_writer = Arc::clone(&writer);
    let worker = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("worker runtime");
        let mut ex = Exchange::new(Message::default());
        let mut errors = Vec::with_capacity(ITERATIONS);
        for _ in 0..ITERATIONS {
            let err = rt
                .block_on(worker_writer.evaluate(&mut ex))
                .expect_err("the sentinel writer must always fail");
            errors.push(err.to_string());
            writer_done_tx
                .send(())
                .expect("the reader side must still be alive");
            reader_done_rx
                .recv_timeout(ROUND_TIMEOUT)
                .unwrap_or_else(|e| panic!("writer timed out waiting for the reader round: {e}"));
        }
        errors
    });

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime");
    let mut msg = Message::default();
    msg.set_header("h1", "v1");
    let mut ex = Exchange::new(msg);
    ex.set_property("k1", "pv");
    for _ in 0..ITERATIONS {
        writer_done_rx
            .recv_timeout(ROUND_TIMEOUT)
            .unwrap_or_else(|e| panic!("reader timed out waiting for the writer round: {e}"));
        let value = rt.block_on(reader.evaluate(&ex)).expect("reader eval");
        assert_eq!(
            value,
            Value::Array(vec![Value::String("pv".into()), Value::Null,]),
            "the reader must never observe the other evaluation's sentinel"
        );
        reader_done_tx
            .send(())
            .expect("the writer side must still be alive");
    }

    let errors = worker.join().expect("writer thread must not panic");
    assert!(
        errors.iter().all(|e| e == &errors[0]),
        "the writer's errors must be stable: {errors:?}"
    );

    // Both objects remain usable: neither is poisoned by the interleaving.
    let mut writer_ex = Exchange::new(Message::default());
    let err = rt
        .block_on(writer.evaluate(&mut writer_ex))
        .expect_err("the writer object must stay usable");
    assert_eq!(err.to_string(), errors[0], "writer error must stay stable");
    let value = rt
        .block_on(reader.evaluate(&ex))
        .expect("the reader object must stay usable");
    assert_eq!(
        value,
        Value::Array(vec![Value::String("pv".into()), Value::Null])
    );
}
