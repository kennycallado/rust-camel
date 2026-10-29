# Design: jsonschema-format-enforcement

## Approach

`jsonschema` 0.58 exposes `jsonschema::options()`, a `ValidationOptions`
builder whose `draft` field defaults to `None` — the schema's `$schema` key
still selects the draft (2020-12 when absent), exactly like
`validator_for`. `.should_validate_formats(true)` overrides the
draft-dependent default (enforced on Draft 4/6/7, annotation-only on
2019-09/2020-12) and forces enforcement on every draft. The workspace
dependency (`default-features = false, features = ["resolve-file"]`)
gates only resolvers; format checks (email, uri, date-time, ...) are
built in. Unknown formats stay annotations: the builder default is
`ignore_unknown_formats: true`, which is the crate's documented policy and
is kept.

Both compile sites replace `jsonschema::validator_for(schema)` with
`jsonschema::options().should_validate_formats(true).build(schema)`
(mapped to the existing error types). The two crates (`camel-processor`,
`camel-validator`) share no crate that could host a helper without a new
dependency edge, so each carries a private `compile_json_schema` helper
with a comment cross-referencing the other; `lint-component-deps` is
respected.

Validator opt-out: `ValidatorConfig` gains `validate_formats: bool`
(default `DEFAULT_VALIDATE_FORMATS = true`). A new `ValidatorBundle`
implements `ComponentBundle` (`config_key() == "validator"`) in
`camel-validator`, mirroring `TemplateBundle`: `from_toml` accepts only an
exact bool for `validate-formats`; any other value fails config load with
an error naming `components.validator.validate-formats`. `camel-bundles`
already depends on `camel-component-validator` (XSD bridge wiring), so
registering the bundle in `camel-bundles` boot adds no edge. The resolved
flag is carried by the validator component and injected into
`ValidatorConfig` at endpoint creation, before `CompiledValidator::compile`
consumes it.

REST `request_schema`: the DSL lowers `request_schema` into an
`UnmarshalStep.schema` consumed by `JsonSchemaValidateService`; there is no
per-operation config carrier on that path, so format enforcement is
always-on there. Documented as such; no DSL key is invented.

## Affected crates

- `camel-processor`: `JsonSchemaValidateService::new` compiles with
  formats enabled. Tests in `tests/json_schema_validate.rs`.
- `camel-validator`: `compiled.rs` (JSON + YAML compile paths) honors
  `validate_formats`; new `bundle.rs` (`ValidatorBundle`) + config default;
  component registration carries the flag.
- `camel-bundles`: registers `ValidatorBundle` alongside `TemplateBundle`.
- `camel-dsl`: no code change; route-level test added in
  `tests/rest_schema_e2e.rs`.
- Docs: `docs/src/components/validator.md`,
  `docs/src/yaml-dsl/top-level-blocks.md`.

## Architecture boundaries

Components boundary: the opt-out is component-owned configuration flowing
through the existing `ComponentBundle` registry (`[components.<key>]`
tables, same mechanism as `components.template`); the component API
surface is unchanged. Runtime/Services boundary: `JsonSchemaValidateService`
is a data-plane Tower service; format enforcement changes only its
compiled predicate, not the exchange contract. DSL boundary: `rest:` YAML
shape is unchanged — `request_schema` stays a plain JSON Schema object.
Respects ADR-0017 (snake_case DSL keys — no new keys introduced) and the
component config conventions established by the template bundle.

## Alternatives considered

- Per-endpoint URI parameter (`validator:...?validateFormats=false`):
  rejected — the order names `[components.validator] validate-formats`,
  and the bundle table gives one app-wide default consistent with the
  template precedent.
- Shared helper crate for the two compile sites: rejected — would create a
  new dependency edge the order forbids.
- REST opt-out DSL key: rejected — no per-operation carrier exists; order
  says keep always-on and say so rather than invent a key.
- `with_draft(Draft::Draft7)` pinning: rejected — would drop `$schema`
  auto-detection and change draft semantics, not just formats.

Bd: rc-giuvl
