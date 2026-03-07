use camel_core::route::BuilderStep;

use camel_dsl::{
    ChoiceStepDef, DeclarativeConcurrency, DeclarativeRoute, DeclarativeStep, FilterStepDef,
    LanguageExpressionDef, LogStepDef, ScriptStepDef, SetBodyStepDef, SetHeaderStepDef, ToStepDef,
    ValueSourceDef, WhenStepDef, compile_declarative_route,
};

#[test]
fn declarative_route_compiles_literal_steps_and_route_metadata() {
    let route = DeclarativeRoute {
        from: "timer:tick".into(),
        route_id: "dsl-route".into(),
        auto_startup: false,
        startup_order: 42,
        concurrency: Some(DeclarativeConcurrency::Sequential),
        error_handler: None,
        circuit_breaker: None,
        steps: vec![
            DeclarativeStep::Log(LogStepDef::info("hello")),
            DeclarativeStep::To(ToStepDef::new("mock:out")),
        ],
    };

    let def = compile_declarative_route(route).unwrap();
    assert_eq!(def.route_id(), "dsl-route");
    assert!(!def.auto_startup());
    assert_eq!(def.startup_order(), 42);
    assert!(matches!(&def.steps()[1], BuilderStep::To(uri) if uri == "mock:out"));
    assert!(def.concurrency_override().is_some());
}

#[test]
fn declarative_script_compiles_to_dedicated_runtime_step() {
    let route = DeclarativeRoute {
        from: "timer:tick".into(),
        route_id: "dsl-script".into(),
        auto_startup: true,
        startup_order: 1,
        concurrency: None,
        error_handler: None,
        circuit_breaker: None,
        steps: vec![DeclarativeStep::Script(ScriptStepDef {
            expression: LanguageExpressionDef {
                language: "custom-lang".into(),
                source: "do something".into(),
            },
        })],
    };

    let def = compile_declarative_route(route).unwrap();
    assert!(matches!(
        &def.steps()[0],
        BuilderStep::DeclarativeScript { expression }
            if expression.language == "custom-lang" && expression.source == "do something"
    ));
}

#[test]
fn declarative_language_steps_are_deferred_to_runtime_registry() {
    let route = DeclarativeRoute {
        from: "timer:tick".into(),
        route_id: "dsl-langs".into(),
        auto_startup: true,
        startup_order: 1,
        concurrency: None,
        error_handler: None,
        circuit_breaker: None,
        steps: vec![
            DeclarativeStep::SetHeader(SetHeaderStepDef {
                key: "k".into(),
                value: ValueSourceDef::Expression(LanguageExpressionDef {
                    language: "custom-lang".into(),
                    source: "header expr".into(),
                }),
            }),
            DeclarativeStep::SetBody(SetBodyStepDef {
                value: ValueSourceDef::Expression(LanguageExpressionDef {
                    language: "custom-lang".into(),
                    source: "body expr".into(),
                }),
            }),
            DeclarativeStep::Filter(FilterStepDef {
                predicate: LanguageExpressionDef {
                    language: "custom-lang".into(),
                    source: "pred expr".into(),
                },
                steps: vec![DeclarativeStep::To(ToStepDef::new("mock:filtered"))],
            }),
            DeclarativeStep::Choice(ChoiceStepDef {
                whens: vec![WhenStepDef {
                    predicate: LanguageExpressionDef {
                        language: "custom-lang".into(),
                        source: "when expr".into(),
                    },
                    steps: vec![DeclarativeStep::To(ToStepDef::new("mock:when"))],
                }],
                otherwise: Some(vec![DeclarativeStep::To(ToStepDef::new("mock:otherwise"))]),
            }),
        ],
    };

    let def = compile_declarative_route(route).unwrap();
    assert!(matches!(
        &def.steps()[0],
        BuilderStep::DeclarativeSetHeader {
            key,
            value: ValueSourceDef::Expression(expression),
        } if key == "k" && expression.language == "custom-lang" && expression.source == "header expr"
    ));
    assert!(matches!(
        &def.steps()[1],
        BuilderStep::DeclarativeSetBody {
            value: ValueSourceDef::Expression(expression),
        } if expression.language == "custom-lang" && expression.source == "body expr"
    ));
    assert!(matches!(
        &def.steps()[2],
        BuilderStep::DeclarativeFilter { predicate, .. }
            if predicate.language == "custom-lang" && predicate.source == "pred expr"
    ));
    assert!(matches!(
        &def.steps()[3],
        BuilderStep::DeclarativeChoice { whens, .. }
            if whens.len() == 1
                && whens[0].predicate.language == "custom-lang"
                && whens[0].predicate.source == "when expr"
                && matches!(whens[0].steps.as_slice(), [BuilderStep::To(uri)] if uri == "mock:when")
    ));
}
