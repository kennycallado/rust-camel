//! Numeric boundary regressions for XPath number projection.
//!
//! These pin the non-finite refusal taxonomy and the integer/float boundary
//! behavior of [`crate::number_to_json`] as observed through both language
//! verbs.

use super::XPathLanguage;
use super::test_support::mapped_class_and_cause;
use camel_api::ExpressionErrorClass;
use camel_language_api::Body;
use camel_language_api::Exchange;
use camel_language_api::Language;
use camel_language_api::LanguageError;
use camel_language_api::Message;
use serde_json::Value as JsonValue;

async fn exchange_with_xml(xml: &str) -> Exchange {
    Exchange::new(Message::new(Body::Xml(xml.to_string())))
}

#[tokio::test]
async fn expression_count_function() {
    let lang = XPathLanguage::new();
    let expr = lang.create_expression("count(/root/item)").unwrap();
    let ex = exchange_with_xml("<root><item>a</item><item>b</item></root>").await;
    let result = expr.evaluate(&ex).await.unwrap();
    assert_eq!(result, JsonValue::Number(serde_json::Number::from(2i64)));
    assert_eq!(result.as_i64(), Some(2));
}

#[tokio::test]
async fn r2_regression_positive_infinity_refused() {
    let lang = XPathLanguage::new();
    let expr = lang.create_expression("1 div 0").unwrap();
    let ex = exchange_with_xml("<root/>").await;
    let err = expr
        .evaluate(&ex)
        .await
        .expect_err("1 div 0 must be refused");
    match &err {
        LanguageError::EvalFailure { class, detail, .. } => {
            assert_eq!(*class, ExpressionErrorClass::Arithmetic);
            let detail = detail.as_deref().expect("detail must be present");
            assert!(detail.contains("+Inf"), "detail was: {detail}");
        }
        other => panic!("expected EvalFailure Arithmetic, got {other:?}"),
    }
    let (class, cause, _) = mapped_class_and_cause(err);
    assert_eq!(class, ExpressionErrorClass::Arithmetic);
    assert!(cause.contains("+Inf"), "cause was: {cause}");
}

#[tokio::test]
async fn r2_regression_negative_infinity_refused() {
    let lang = XPathLanguage::new();
    let expr = lang.create_expression("-1 div 0").unwrap();
    let ex = exchange_with_xml("<root/>").await;
    let err = expr
        .evaluate(&ex)
        .await
        .expect_err("-1 div 0 must be refused");
    match &err {
        LanguageError::EvalFailure { class, detail, .. } => {
            assert_eq!(*class, ExpressionErrorClass::Arithmetic);
            let detail = detail.as_deref().expect("detail must be present");
            assert!(detail.contains("-Inf"), "detail was: {detail}");
        }
        other => panic!("expected EvalFailure Arithmetic, got {other:?}"),
    }
    let (class, cause, _) = mapped_class_and_cause(err);
    assert_eq!(class, ExpressionErrorClass::Arithmetic);
    assert!(cause.contains("-Inf"), "cause was: {cause}");
}

#[tokio::test]
async fn r2_regression_nan_refused() {
    let lang = XPathLanguage::new();
    let expr = lang.create_expression("0 div 0").unwrap();
    let ex = exchange_with_xml("<root/>").await;
    let err = expr
        .evaluate(&ex)
        .await
        .expect_err("0 div 0 must be refused");
    match &err {
        LanguageError::EvalFailure { class, detail, .. } => {
            assert_eq!(*class, ExpressionErrorClass::Arithmetic);
            let detail = detail.as_deref().expect("detail must be present");
            assert!(detail.contains("NaN"), "detail was: {detail}");
        }
        other => panic!("expected EvalFailure Arithmetic, got {other:?}"),
    }
    let (class, cause, _) = mapped_class_and_cause(err);
    assert_eq!(class, ExpressionErrorClass::Arithmetic);
    assert!(cause.contains("NaN"), "cause was: {cause}");
}

#[tokio::test]
async fn r2_regression_predicate_verb_refuses_infinity() {
    let lang = XPathLanguage::new();
    let pred = lang.create_predicate("1 div 0").unwrap();
    let ex = exchange_with_xml("<root/>").await;
    let err = pred
        .matches(&ex)
        .await
        .expect_err("1 div 0 must be refused, not coerced to bool");
    match &err {
        LanguageError::EvalFailure { class, .. } => {
            assert_eq!(*class, ExpressionErrorClass::Arithmetic);
        }
        other => panic!("expected EvalFailure Arithmetic, got {other:?}"),
    }
}

#[tokio::test]
async fn string_of_infinity_is_a_string_not_refused() {
    let lang = XPathLanguage::new();
    let expr = lang.create_expression("string(1 div 0)").unwrap();
    let ex = exchange_with_xml("<root/>").await;
    let result = expr.evaluate(&ex).await.unwrap();
    assert_eq!(result, JsonValue::String("Infinity".to_string()));
}

#[tokio::test]
async fn fractional_number_stays_float() {
    let lang = XPathLanguage::new();
    let expr = lang.create_expression("2.5").unwrap();
    let ex = exchange_with_xml("<root/>").await;
    let result = expr.evaluate(&ex).await.unwrap();
    assert_eq!(result.as_f64(), Some(2.5));
    assert!(!result.is_i64() && !result.is_u64());
}

#[tokio::test]
async fn integral_two_pow63_yields_u64_integer_form() {
    let lang = XPathLanguage::new();
    let expr = lang.create_expression("9223372036854775808 div 1").unwrap();
    let ex = exchange_with_xml("<root/>").await;
    let result = expr.evaluate(&ex).await.unwrap();
    assert_eq!(result.as_u64(), Some(9_223_372_036_854_775_808));
}

#[tokio::test]
async fn integral_two_pow63_negative_yields_i64_min() {
    let lang = XPathLanguage::new();
    let expr = lang
        .create_expression("-9223372036854775808 div 1")
        .unwrap();
    let ex = exchange_with_xml("<root/>").await;
    let result = expr.evaluate(&ex).await.unwrap();
    assert_eq!(result.as_i64(), Some(i64::MIN));
}

#[tokio::test]
async fn integral_two_pow64_stays_float_no_saturation() {
    let lang = XPathLanguage::new();
    let expr = lang
        .create_expression("18446744073709551616 div 1")
        .unwrap();
    let ex = exchange_with_xml("<root/>").await;
    let result = expr.evaluate(&ex).await.unwrap();
    assert!(!result.is_i64() && !result.is_u64());
    assert_eq!(result.as_f64(), Some(18_446_744_073_709_551_616.0));
}

#[tokio::test]
async fn negative_integral_two_pow64_stays_float_no_zero_saturation() {
    let lang = XPathLanguage::new();
    let expr = lang
        .create_expression("-18446744073709551616 div 1")
        .unwrap();
    let ex = exchange_with_xml("<root/>").await;
    let result = expr.evaluate(&ex).await.unwrap();
    assert!(!result.is_i64() && !result.is_u64());
    assert_eq!(result.as_f64(), Some(-18_446_744_073_709_551_616.0));
}

#[tokio::test]
async fn negative_zero_projects_to_zero_integer_form() {
    let lang = XPathLanguage::new();
    let expr = lang.create_expression("0 div -3").unwrap();
    let ex = exchange_with_xml("<root/>").await;
    let result = expr.evaluate(&ex).await.unwrap();
    assert_eq!(result.as_i64(), Some(0));
}
