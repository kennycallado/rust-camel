# Design: permsrc-lint-exactly-one

## Approach

Two coordinated edits, one regeneration, tests at three layers.

### 1. Schema contract (camel-dsl `route_ast.rs`)

`RouteDslPermissionValueSource` keeps its Rust shape (three `Option<String>`
fields, `deny_unknown_fields`, serde defaults) — runtime deserialization is
untouched. A schemars 1.x annotation injects a `oneOf` constraint into the
generated `$defs.RouteDslPermissionValueSource` node:

```json
"oneOf": [
  { "properties": { "literal": { "type": "string" },
                    "header":  { "type": "null" },
                    "property":{ "type": "null" } },
    "required": ["literal"] },
  { "properties": { "literal": { "type": "null" },
                    "header":  { "type": "string" },
                    "property":{ "type": "null" } },
    "required": ["header"] },
  { "properties": { "literal": { "type": "null" },
                    "header":  { "type": "null" },
                    "property":{ "type": "string" } },
    "required": ["property"] }
]
```

Why `oneOf` branches instead of `minProperties`/`maxProperties`: JSON Schema
`properties` only constrains keys that are PRESENT, and each branch types
the sibling keys as `null` — so `{literal: "x", header: null}` passes branch
1, exactly matching serde (explicit `null` deserializes to `None`). A pure
property-count constraint would over-reject null-sibling forms the runtime
accepts. Exactly-one-branch-matches holds for every input shape:

- `{}` — fails all three (`required`) → zero-source error
- `{literal: "a", header: "b"}` — fails all three (sibling type) →
  multi-source error
- `{literal: "a"}` / `{literal: "a", header: null}` — matches exactly
  branch 1 → clean

Implementation note (PINNED): use schemars 1.x `transform` — a small
`#[cfg(feature = "schema")] fn permission_value_source_oneof(schema:
&mut schemars::Schema)` in `route_ast.rs` that inserts the oneOf JSON
(via `serde_json::json!` into the schema map under key `"oneOf"`),
wired via `#[cfg_attr(feature = "schema",
schemars(transform = permission_value_source_oneof))]`. The transform
path is deterministic for arbitrary nested JSON; `extend` attribute
literals are NOT used. The path must round-trip `cargo xtask schema
--check`. The struct doc comment gains a line stating the mirror
contract and its runtime anchor (`yaml_source_to_value_source`).

### 2. Diagnostic rendering (camel-lint `rules/rschema.rs`)

jsonschema 0.52 exposes two distinct kinds for oneOf failures:
`OneOfNotValid` (no branch matched — the zero-source and multi-source
case) and `OneOfMultipleValid` (several branches matched — reachable only
for malformed inputs under looser schemas). `OneOfMultipleValid` (and
every non-permission failure) keeps today's generic diagnostic shape
byte-identically.

**Where the error surfaces.** `security_policy`, `permission`, and
`resource`/`action` are ALL schemars `Option` fields (`anyOf: [$ref,
null]` each), so a zero/multi-source value produces a NESTED error tree:
the top-level reported error is a collapsed `AnyOf` at the
`security_policy` node, containing an `AnyOf` at `permission`, containing
an `AnyOf` at `resource`/`action`, containing the `OneOfNotValid` from
`RouteDslPermissionValueSource`'s oneOf (empirically confirmed against
jsonschema 0.52.1 with the proposed schema). Detection therefore uses a
small RECURSIVE WALKER in the existing AnyOf arm:
`fn collect_permission_oneof_paths(err: &ValidationError<'_>, out:
&mut Vec<String>)` — descend `AnyOf`/`OneOfNotValid` contexts to any
depth (both kinds carry `context: Vec<Vec<ValidationError<'static>>>`)
and collect, as OWNED instance-path strings, every `OneOfNotValid` whose
`schema_path()` contains the marker substring
`RouteDslPermissionValueSource/oneOf` (probe-confirmed verbatim form).
For EACH match (deduped by instance path — a two-field route yields two
sibling matches under one collapsed AnyOf), the field name, span anchor,
and found-set value are derived from THAT match's own `instance_path()`
(which ends in `/resource` or `/action` and points at the value mapping;
the validation instance is navigated via `serde_json` pointer), and one
diagnostic is emitted per match. No standalone `OneOfNotValid` arm is
added: the def's only two ref-sites are the anyOf-wrapped `resource`/
`action`, so every permission oneOf failure reaches the walker; top-level
oneOf failures (e.g. `credential_sources` array items — plain `$ref`, no
Option wrapper) keep the generic `_ =>` behavior, pinned by a regression
test. Walker `continue` note: when a matched permission oneOf is found
under a collapsed AnyOf, the remaining nested non-permission errors of
that same AnyOf (e.g. a sibling `AdditionalProperties`) are intentionally
subsumed — first-error-wins, mirroring the arm's existing documented
de-collapse limitation.

Detection of the permission value-source position is DOUBLE-guarded:

1. **Schema-path identity** (primary): the (possibly nested) error's
   `schema_path()` must resolve into the `RouteDslPermissionValueSource`
   oneOf — concretely, contain the segments
   `RouteDslPermissionValueSource` followed by `oneOf` (exact segment form
   — `$defs` ref or inlined — verified against the regenerated schema
   during implementation via a debug print of a failing fixture's errors).
   This is collision-proof: `CredentialSourceDsl`, `ExceptionDisposition`,
   and `RouteDslRestBinding` oneOf failures have different schema paths and
   keep their collapsed diagnostics.
2. **Instance-path tail** (field context): last segment is `resource` or
   `action` (the only parents referencing this def, under
   `RouteDslSecurityPolicy.permission` in both `routes` and `rest`
   envelopes; prefix-agnostic because only the tail is inspected).

For a doubly-detected node, synthesize ONE diagnostic with the exact
runtime error wording (camel-dsl yaml.rs:380-388, including the `none set`
empty-set rendering):

```
security_policy permission {field} must specify exactly one of: literal,
header, or property (set: {found})
```

`{field}` comes from the path tail; `{found}` is computed from the
validation instance value at that path — the source keys whose values are
non-null, in canonical `literal, header, property` order; empty set renders
`none set`. Span: `value_span_for` on the resource/action mapping (same
anchoring as the default arm). Any error failing either guard falls through
to today's AnyOf/pattern-de-collapse behavior byte-identically.

### 3. Regeneration

`cargo xtask schema` rewrites `schemas/dsl/route-schema.json`, syncs the
byte-equal `crates/camel-lint/schema/route-schema.json` copy, and
regenerates TS artifacts (expected content-unchanged; ts_rs ignores
schemars annotations). The `schema --check` gate enforces all of this.

### 4. Tests

- `rschema/tests.rs` (camel-lint): zero-source → one R-SCHEMA Error, message
  contains field name + `(set: none set)`; multi-source (literal+header) →
  message contains `(set: literal, header)`; all-three-non-null → canonical
  ordering `(set: literal, header, property)`; `action` field context; exactly
  one → no R-SCHEMA diagnostics; one string + null siblings → no R-SCHEMA
  diagnostics; all-null → one Error with `(set: none set)`; valid source plus
  unknown key → the generic collapsed-AnyOf diagnostic, no targeted
  exactly-one diagnostic; unknown key only (`{bogus: x}`) → targeted
  exactly-one Error fires (subsumes the nested AdditionalProperties signal);
  malformed scalar source → generic diagnostics only; exact
  one-diagnostic-per-value-spec count for zero and multi shapes;
  non-permission oneOf outputs (CredentialSourceDsl top-level,
  ExceptionDisposition, RouteDslRestBinding) unchanged.
- Corpus fixtures (camel-cli `tests/fixtures/lint-corpus/`):
  `permission-value-source-zero.yaml`, `permission-value-source-multi.yaml`
  (negative — baseline entries with justification comments),
  `permission-value-source-exactly-one.yaml` (positive — no baseline entry).
  Each wraps a minimal otherwise-valid route so the only diagnostics are the
  intended ones.

## Affected crates

- camel-dsl: schemars annotation on `RouteDslPermissionValueSource` (+ doc
  comment). No behavior change — the `schema` feature is derive-only.
- camel-lint: OneOf arm in `rschema.rs` + unit tests in `rschema/tests.rs`.
- camel-cli: 3 corpus fixtures + 2 baseline entries in
  `lint-corpus-baseline.ron`.
- scripts/xtask: no code change (regeneration only).

## Architecture boundaries

Data/control plane boundary respected: the change touches only the DSL
authoring surface (schema contract) and the runtime-free lint consumer
(R-SCHEMA rule). No runtime processor, component, or service code changes.
The schema remains the single source of the authoring contract
(`route-schema.json` generated from route_ast derives, byte-checked by the
xtask gate per openspec/specs/route-lint "Schema asset is embedded and kept
byte-equal"), so SDK consumers and `camel lint` see the same constraint
without a second hand-maintained copy.

Relevant decisions: rc-gddb2 (REJECT-ON-AMBIGUITY for permission value
sources — this change makes lint enforce the same decision), retro520
finding (lint/schema drift vs runtime on exactly-one), bd rc-lkbqi.
