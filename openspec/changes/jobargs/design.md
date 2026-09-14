# Design: jobargs

## Approach

Keep arguments as job-document data. Extend the strict raw serde model with an
optional top-level `args` map and a strict per-argument declaration. Normalize
CLI pairs into a deterministic map after parsing. When `args` is present,
validate names, reject unknown names, require values for declarations marked
`required` without defaults, and apply defaults. When absent, retain the
current ordered header injection and emit one deprecation diagnostic.

Interpolation uses the existing camel-dsl scanner and tree-walk stage. Extend
its namespace dispatch so `env:` and `arg:` share one scanner, escaping,
sanitization, and lookup path. Argument declaration names and `${arg:NAME}`
names use `[A-Za-z_][A-Za-z0-9_]*`; `${arg:NAME:-fallback}` is unsupported.
The scanner dispatches namespace before lookup: `env:` consults ambient
environment, while `arg:` consults resolved declarations only; an argument
name never falls through to environment. Normal and
embedded documents enter this same runtime resolver; artifacts continue to
embed pre-interpolation text.

Compiled artifacts remain fail-closed and keep their frozen argument surface:
they accept no new `--arg` flag. The embedded declaration travels in the
existing document trailer; defaults resolve at startup, while a required
argument without a default fails with exit 2. This gives compiled artifacts
the same interpolation stage without widening their CLI contract.

## Affected crates

- `camel-cli`: job document schema, argument validation, execution wiring,
  compiled-artifact argument parsing, and end-to-end tests.
- `camel-dsl`: shared `${env:}`/`${arg:}` interpolation scanner and tree walk.
- `camel-lint`: synchronized scanner mirror and parity tests.
- `openspec/specs/cli-jobs`: canonical job argument contract.

## Architecture boundaries

Arguments stay operator configuration at the CLI/DSL boundary. They do not
become runtime headers unless using the legacy no-declaration path, and they do
not create a runtime registry or trait. Route loading still follows the DSL
discovery and embedded seams. This preserves the DSL-to-runtime boundary and
the compiled-artifact contract in ADR-0075; reserved document naming and
interpolation behavior follow ADR-0062 and the existing camel-dsl context.

## Phases

### Phase 1: Declare and parse string arguments
- **Goal:** Add strict top-level and per-argument schema types.
- **Dependencies:** Existing job document parser and serde deny-unknown policy.
- **Externally-visible types/interfaces:** Job YAML `args:` declaration.
- **Deliverable:** Parser implementation and unit coverage.
- **Exit-criteria:** Valid declarations parse; unknown keys fail with exit 2.

### Phase 2: Generalize shared interpolation
- **Goal:** Resolve `env:` and `arg:` through one scanner and tree-walk stage.
- **Dependencies:** Phase 1 argument names and lookup representation.
- **Externally-visible types/interfaces:** `${arg:NAME}` interpolation syntax.
- **Deliverable:** camel-dsl scanner, camel-lint mirror, parity tests.
- **Exit-criteria:** Escapes, embedded tokens, and unresolved names behave identically.

### Phase 3: Validate and execute arguments
- **Goal:** Apply validation/defaults and wire resolved values into job and artifact runs.
- **Dependencies:** Phases 1 and 2; current setup_booted_job teardown shape.
- **Externally-visible types/interfaces:** `camel job --arg` validation and exit-2 diagnostics.
- **Deliverable:** CLI and compiled-artifact behavior with back-compat note.
- **Exit-criteria:** Unknown/missing errors exit 2; defaults and interpolation work in all four fields.

### Phase 4: Contract and regression coverage
- **Goal:** Lock the cross-path behavior and update project context/spec documentation.
- **Dependencies:** Phases 1–3.
- **Externally-visible types/interfaces:** Documented job argument contract.
- **Deliverable:** E2E tests, delta spec, context citations.
- **Exit-criteria:** Job and compiled artifact resolve the same values; quality gates pass.

## Alternatives considered

Using a runtime argument registry or typed argument enum was rejected by the
ruling: arguments are document data and typed arguments belong to A4. Keeping
`${arg:}` as a second interpolation implementation was rejected because it
would violate the same-stage requirement and risk divergence between jobs and
compiled artifacts.
