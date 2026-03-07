use crate::Value;

/// A language expression/predicate reference resolved by the runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageExpressionDef {
    pub language: String,
    pub source: String,
}

/// A declarative value source for set_header / set_body.
#[derive(Debug, Clone, PartialEq)]
pub enum ValueSourceDef {
    Literal(Value),
    Expression(LanguageExpressionDef),
}
