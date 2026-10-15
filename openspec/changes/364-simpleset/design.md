# Design: 364-simpleset

## Approach

### Value-source mappings

Keep `SetBodyData::Config` before `Literal`. In shared lowering in
`crates/camel-dsl/src/yaml.rs`, both `set_body` and `transform` call one literal
conversion helper. Reject `Value::Object` before `bare_string_value_source`.
Do not reject in a custom deserializer: the outer untagged `RouteDslStep` would
hide actionable errors. Valid configs still use existing `parse_value_source`.

For rejected objects, find keys outside `value`, `language`, `source`, `simple`,
`rhai`, `jsonpath`, `xpath`; sort them lexicographically. Unknown keys take
precedence over wrong-type diagnostics. Otherwise identify invalid non-string,
non-null expression fields in fixed declaration order. Report only key names,
never serialized values or serde error details. Any unexpected object case must
still fail closed. The valid alternatives text is always:
`` `value`, `language`+`source`, `simple`, `rhai`, `jsonpath`, `xpath` ``.
The mixed-key acceptance condition refers to offending-key names, not that list.

Under the schema feature, provide a `JsonSchema` representation for `SetBodyData`
with `anyOf: [SetBodyConfig, {type: [string, number, boolean, array, null]}]`.
Regenerate `schemas/dsl/route-schema.json` and
`crates/camel-lint/schema/route-schema.json` using `cargo xtask schema`.
Do not use `not`; the lint schema walker does not support it. Keep the broad TS
binding and document JSON schema as the enforcement artifact. Confirm the lint
container-key derivation is unchanged.

### Script capability

Introduce one `compile_script` helper in
`crates/camel-core/src/lifecycle/adapters/step_compilers/core.rs`, called by
`BuilderStep::Script` and `BuilderStep::DeclarativeScript`. Resolve the language
from the registry; call `create_mutating_expression`; produce `ScriptMutator`
with existing eval metadata on success. `NotSupported` produces the shared
`RouteError` in the delta spec. Delete the `SetBody` fallback. Preserve existing
unknown-language and other creation-error handling. Do not list built-in
languages in diagnostics: custom registered languages define their capability.

## Affected crates

- `camel-dsl`: lowering, schema representation, YAML/JSON regression coverage,
  README, AST comments and CONTEXT glossary.
- `camel-core`: shared compiler, constructor/runtime tests, route-definition
  comments and surviving redaction fixture seed.
- `camel-lint`: generated schema copy and schema/container-key verification.
- `camel-integration-test`: `direct_reply_test.rs` wrapped object fixture.
- `camel-test`: four `cxf_test.rs` inline route fixtures; mutating-script checks.

## Architecture boundaries

DSL stays independent of the Languages registry. Capability checks belong to
Runtime compilation before a Pipeline executes; Components, Services and
Functions do not change. Respect ADR-0001's data/control plane split and
ADR-0032's trust boundary. Preserve existing keys under ADR-0046, snake_case
under ADR-0017, and YAML/JSON parity under ADR-0026. ADR-0006's typed runtime
error propagation remains unchanged. References are indexed by `CONTEXT-MAP.md`.

## Phases

### Phase 1: Reject invalid value-source mappings

Deliver DSL lowering, schema/tests and object migration in independent commit
`fix(dsl)!: reject bare maps in set_body/transform`, with its `BREAKING CHANGE:`
footer and `Bd: rc-me8oe`. Exit: YAML/JSON negative and positive coverage passes,
schema copies agree, container keys are stable and object fixtures are migrated.

### Phase 2: Require mutating scripts

Deliver shared Runtime compiler, script test/doc migrations and complete
verification in independent commit
`fix(core)!: fail script with non-mutating language`, with its migration footer
and `Bd: rc-i7cfx`. Exit: all surfaces reject read-only scripts identically,
mutating scripts work, redaction seed survives, inventory sweep and gates pass.
Both phases are planned together after this spec blessing; no incremental bless.

## Migration and verification

Inventory: `/home/shared/tmp/364-simpleset/inventory.md`, 338 classified hits.
Migrate B1-B3, S1, T1-T4, four CXF maps (:264,526,580,643), and README :419.
CXF `constant` maps become `value: {constant: ...}` to preserve current object
semantics. Migrate parse-only script examples to valid mutating languages too.
Foreign Apache Camel `setBody` syntax and historical archives remain untouched.
Update basic/transformation migration callouts, OpenSearch/minijinja examples,
EIP transform prose, comments and the BREAKING object-literal-wrapper glossary.
The required migration text is in proposal.md. Keep a per-site ledger for a
post-change DSL parser sweep of route files and extracted examples; record
partial-document exclusions and every unrelated failure without claiming zero
global failures. CXF infra tests deferred to CI must be reported.

## Risks and open questions

No design choices remain open under decision.md. Generic shape diagnostics are
separate follow-up rc-tyv0r. No new dependencies, key aliases, TS narrowing or
static lint checks. User directive supersedes old model routing: `r_gpt` only;
fresh `e_opus` spec and plan blessing sessions, one attempt each.
