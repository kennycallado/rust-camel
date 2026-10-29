//! Per-verb conformance kit seeded from the language-boundary audit.
//!
//! The table below pins the audit repros that apply to the XPath language
//! boundary: R1 (parse errors propagate as errors, never a silent `false`),
//! R2 (non-finite numbers — `+Inf`, `-Inf`, and `NaN` — are refused on both
//! verbs), R4 (typed predicate results — no truthiness coercion of numbers or
//! strings), and R5 (refusal diagnostics never embed document content, checked
//! on both the raw `LanguageError` text and the mapped `CamelError` text).
//! R3 is N/A for this crate: it pins the simple-language `${body}`
//! stringification surface, which is not an XPath conversion surface.
//!
//! Source of the case matrix: `.opencode/fleet/language-boundary-audit-matrix-20261002.md`.
//! Cases are seeded by hand; do not regenerate them from the matrix.

use super::XPathLanguage;
use super::test_support::mapped_class_and_cause;
use camel_api::ExpressionErrorClass;
use camel_language_api::Body;
use camel_language_api::Exchange;
use camel_language_api::Language;
use camel_language_api::LanguageError;
use camel_language_api::Message;

/// Marker embedded in a body to prove it never reaches refusal diagnostics.
const SENTINEL: &str = "LEAKSENTINEL";

/// Verb under which a conformance case is dispatched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verb {
    Expression,
    Predicate,
}

/// Expected outcome for a conformance case.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Expect {
    /// `EvalFailure` with class `Arithmetic` whose detail contains this label.
    ArithmeticRefusal(&'static str),
    /// `LanguageError::TypeMismatch`.
    TypeMismatch,
    /// `EvalFailure` with exactly this class (never `Ok(false)`).
    PredicateError(ExpressionErrorClass),
    /// Any `Err` whose rendered raw and mapped text does not contain
    /// [`SENTINEL`].
    NoSentinelInError,
}

/// One seeded conformance case: audit repro id, dispatch verb, input body,
/// XPath expression, and the expectation that must hold.
struct ConformanceCase {
    id: &'static str,
    audit: &'static str,
    verb: Verb,
    body: &'static str,
    expr: &'static str,
    expect: Expect,
}

/// Seeded cases from the language-boundary audit. Order mirrors the matrix.
const CASES: [ConformanceCase; 8] = [
    ConformanceCase {
        id: "R2-expr-posinf",
        audit: "R2",
        verb: Verb::Expression,
        body: "<root/>",
        expr: "1 div 0",
        expect: Expect::ArithmeticRefusal("+Inf"),
    },
    ConformanceCase {
        id: "R2-pred-posinf",
        audit: "R2",
        verb: Verb::Predicate,
        body: "<root/>",
        expr: "1 div 0",
        expect: Expect::ArithmeticRefusal("+Inf"),
    },
    ConformanceCase {
        id: "R2-pred-neginf",
        audit: "R2",
        verb: Verb::Predicate,
        body: "<root/>",
        expr: "-1 div 0",
        expect: Expect::ArithmeticRefusal("-Inf"),
    },
    ConformanceCase {
        id: "R2-pred-nan",
        audit: "R2",
        verb: Verb::Predicate,
        body: "<root/>",
        expr: "0 div 0",
        expect: Expect::ArithmeticRefusal("NaN"),
    },
    ConformanceCase {
        id: "R1-pred-parse-error-propagates",
        audit: "R1",
        verb: Verb::Predicate,
        body: "LEAKSENTINEL-<broken",
        expr: "/root",
        expect: Expect::PredicateError(ExpressionErrorClass::Parse),
    },
    ConformanceCase {
        id: "R4-pred-number-is-typed-mismatch",
        audit: "R4",
        verb: Verb::Predicate,
        body: "<root><item/><item/></root>",
        expr: "count(/root/item)",
        expect: Expect::TypeMismatch,
    },
    ConformanceCase {
        id: "R4-pred-string-is-typed-mismatch",
        audit: "R4",
        verb: Verb::Predicate,
        body: "<root><name>x</name></root>",
        expr: "/root/name",
        expect: Expect::TypeMismatch,
    },
    ConformanceCase {
        id: "R5-refusal-excludes-document-content",
        audit: "R5",
        verb: Verb::Expression,
        body: "<root>LEAKSENTINEL</root>",
        expr: "1 div 0",
        expect: Expect::NoSentinelInError,
    },
];

async fn exchange_with_xml(xml: &str) -> Exchange {
    Exchange::new(Message::new(Body::Xml(xml.to_string())))
}

/// Dispatch a case through the public language API and return the error it
/// must produce. Any `Ok` result is a conformance failure.
async fn dispatch(lang: &XPathLanguage, case: &ConformanceCase, ex: &Exchange) -> LanguageError {
    match case.verb {
        Verb::Expression => {
            let expr = lang
                .create_expression(case.expr)
                .unwrap_or_else(|e| panic!("[{}] create_expression failed: {e}", case.id));
            match expr.evaluate(ex).await {
                Ok(value) => panic!("[{}] expected an error, got Ok({value:?})", case.id),
                Err(err) => err,
            }
        }
        Verb::Predicate => {
            let pred = lang
                .create_predicate(case.expr)
                .unwrap_or_else(|e| panic!("[{}] create_predicate failed: {e}", case.id));
            match pred.matches(ex).await {
                Ok(value) => panic!("[{}] expected an error, got Ok({value})", case.id),
                Err(err) => err,
            }
        }
    }
}

/// Assert the expected outcome against the observed error.
fn assert_expectation(case: &ConformanceCase, err: LanguageError) {
    let id = case.id;
    let audit = case.audit;
    match case.expect {
        Expect::ArithmeticRefusal(label) => match &err {
            LanguageError::EvalFailure { class, detail, .. } => {
                assert_eq!(
                    *class,
                    ExpressionErrorClass::Arithmetic,
                    "[{id} / {audit}] expected Arithmetic class"
                );
                let detail = detail.as_deref().unwrap_or("");
                assert!(
                    detail.contains(label),
                    "[{id} / {audit}] detail must contain `{label}`, was: {detail}"
                );
            }
            other => panic!("[{id} / {audit}] expected EvalFailure Arithmetic, got {other:?}"),
        },
        Expect::TypeMismatch => match &err {
            LanguageError::TypeMismatch { .. } => {}
            other => panic!("[{id} / {audit}] expected TypeMismatch, got {other:?}"),
        },
        Expect::PredicateError(expected_class) => match &err {
            LanguageError::EvalFailure { class, .. } => {
                assert_eq!(
                    *class, expected_class,
                    "[{id} / {audit}] expected class {expected_class:?}"
                );
            }
            other => {
                panic!("[{id} / {audit}] expected EvalFailure {expected_class:?}, got {other:?}")
            }
        },
        Expect::NoSentinelInError => {
            let rendered = err.to_string();
            assert!(
                !rendered.contains(SENTINEL),
                "[{id} / {audit}] raw error text leaked document content: {rendered}"
            );
            let (_, _, mapped) = mapped_class_and_cause(err);
            let mapped_text = mapped.to_string();
            assert!(
                !mapped_text.contains(SENTINEL),
                "[{id} / {audit}] mapped error text leaked document content: {mapped_text}"
            );
        }
    }
}

#[tokio::test]
async fn conformance_kit_runs_all_seeded_cases() {
    assert!(
        CASES.len() >= 8,
        "conformance table must not be silently emptied"
    );

    let lang = XPathLanguage::new();
    for case in &CASES {
        let ex = exchange_with_xml(case.body).await;
        let err = dispatch(&lang, case, &ex).await;
        assert_expectation(case, err);
    }
}
