use camel_api::{LanguageExpressionDef, ValueSourceDef};

#[test]
fn declarative_types_are_available_from_camel_api() {
    let expression = LanguageExpressionDef {
        language: "simple".to_string(),
        source: "${body}".to_string(),
    };

    let literal = ValueSourceDef::Literal(serde_json::Value::String("hello".to_string()));
    let evaluated = ValueSourceDef::Expression(expression.clone());

    assert!(matches!(
        literal,
        ValueSourceDef::Literal(serde_json::Value::String(ref text)) if text == "hello"
    ));

    assert!(matches!(
        evaluated,
        ValueSourceDef::Expression(LanguageExpressionDef { ref language, ref source })
            if language == "simple" && source == "${body}"
    ));
}
