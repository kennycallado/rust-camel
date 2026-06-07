//! YAML/JSON DSL for defining Camel routes — declarative route parsing and compilation to canonical specs.
//!
//! Main types: `DeclarativeRoute`, `DeclarativeStep`, `YamlRoute`, `YamlStep`.
//! Main modules: `canonical`, `compile`, `contract`, `discovery`, `json`, `model`, `yaml`, `yaml_ast`.

pub mod canonical;
pub mod compile;
pub mod contract;
pub mod discovery;
pub mod env_interpolation;
pub mod json;
pub mod model;
pub mod template;
pub mod yaml;
pub mod yaml_ast;

pub use canonical::{parse_canonical_json, parse_canonical_route};
pub use compile::{
    compile_declarative_route, compile_declarative_route_to_canonical,
    compile_declarative_route_with_stream_cache_threshold, compile_declarative_step,
};
pub use contract::{DeclarativeStepKind, is_rust_only_kind, mandatory_declarative_step_kinds};
pub use discovery::{
    DiscoveryError, discover_routes, discover_routes_with_threshold,
    discover_routes_with_threshold_and_security,
};
pub use json::{
    parse_json, parse_json_to_canonical, parse_json_to_declarative, parse_json_with_threshold,
    parse_json_with_threshold_and_security,
};
pub use model::{
    AggregateStepDef, AggregateStrategyDef, BodyTypeDef, ChoiceStepDef, DeclarativeCircuitBreaker,
    DeclarativeConcurrency, DeclarativeErrorHandler, DeclarativeRedeliveryPolicy, DeclarativeRoute,
    DeclarativeSecurityPolicy, DeclarativeStep, EnrichStepDef, FilterStepDef,
    LanguageExpressionDef, LogLevelDef, LogStepDef, MulticastAggregationDef, MulticastStepDef,
    ScriptStepDef, SecurityCompileContext, SetBodyStepDef, SetHeaderStepDef, SetPropertyStepDef,
    SplitAggregationDef, SplitExpressionDef, SplitStepDef, StreamCacheStepDef, ToStepDef,
    ValueSourceDef, WhenStepDef, WireTapStepDef,
};
pub use yaml::{
    YamlRoute, YamlRoutes, YamlStep, load_from_file, parse_yaml, parse_yaml_to_canonical,
    parse_yaml_to_declarative, parse_yaml_with_threshold, parse_yaml_with_threshold_and_security,
};

pub use template::json::{parse_json_templated_routes, parse_json_templates};
pub use template::materializer::{
    CompiledMaterializationResult, materialize_and_compile, materialize_template, resolve_params,
    substitute_strings_in_json,
};
pub use template::yaml::{parse_yaml_templated_routes, parse_yaml_templates};
