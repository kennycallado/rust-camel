# Design: jobtyped

## Approach

All work lives in `crates/camel-cli/src/commands/job/` (plus its test
modules); the compile artifact path reuses the same parser seam and needs
no new logic of its own.

1. **Type model** — `JobArgType` enum (`String | Int | Bool | Enum(Vec<String>)`)
   on `JobArgumentDeclaration` as `arg_type: JobArgType` defaulting to
   `String` when `type:` is omitted. Parsing happens in
   `job_argument_declaration` (document.rs), which admits the new `type`
   key with a string-scalar value and rejects everything else through the
   existing declaration-error class (`InvalidArgumentDeclaration` /
   new `InvalidArgumentType` wording): unknown type words, non-string
   scalars, malformed enum grammar (`enum[]`, empty-after-trim member,
   duplicate-after-trim member, member containing `,`/`[`/`]`/CR/LF).
   Members are trimmed; membership is exact and case-sensitive.

2. **Canonical forms** — `coerce_argument(value, &JobArgType) ->
   Option<String>`: `int` = strict `i64::from_str` (optional sign,
   digits only, no whitespace, overflow rejected), canonical
   `i64::to_string` (`007`→`7`, `+5`→`5`); `bool` = case-insensitive
   `true`/`false` (NOT `1`/`0`), canonical lowercase; `enum` = exact
   member match, canonical = member verbatim; `string` = verbatim.
   `JobArgType` owns ONE renderer (`fn render(&self) -> String`:
   `string`/`int`/`bool`/`enum[a,b,c]`) shared by the coercion
   diagnostics and the help column, so the two surfaces cannot drift.
   `JobDocError::ArgumentCoercion` carries `expected: JobArgType`.

3. **Where coercion runs** — two seams, mirroring the A2 split between
   declaration checks (parse-time, shared with `--help`) and resolution
   (run-time):
   - Declaration time (`normalize_job_args`): a typed argument WITH a
     `default` whose default fails coercion is rejected at load. `--help`
     inherits the check through `parse_job_document_for_help` (it already
     runs `normalize_job_args`).
   - Resolution time (`resolve_job_args`): after the existing
     unknown-name check, last-wins overwrite, missing-required check, and
     default fill, every typed argument's resolved value is coerced and
     the CANONICAL string replaces the raw one in the resolved map. Order
     guarantees precedence: unknown-name > missing-required > coercion
     (first failure wins, deterministic via BTreeMap iteration).
   - The resolved map still feeds `interpolate_declared_fields` verbatim
     in shape (`BTreeMap<String, String>`), so `${arg:NAME}` substitutes
     the canonical form through the EXISTING `camel_dsl::interpolate_with_args`
     seam with zero camel-dsl change. Coercion strictly precedes
     interpolation, which precedes field validation.

4. **Errors** — new `JobDocError` variants, all exit 2:
   `InvalidArgumentType { argument, raw }` (malformed `type` value) and
   `ArgumentCoercion { name, expected, raw }` where `expected` renders as
   `int` / `bool` / `enum[a,b,c]` — the enum rendering lists the allowed
   members, satisfying the AC "lists allowed values". Diagnostics follow
   the existing loud single-line style.

5. **Help render** — `render_argument_row` (help.rs) replaces the
   hardcoded `"  string  "` with the declared type's rendering
   (`string`/`int`/`bool`/`enum[a,b,c]` comma-joined). The type column
   pads to the widest rendered type in the job, the same strategy as the
   name column. CRLF flattening applies to enum members like any other
   rendered text (grammar already forbids CR/LF in members).

6. **Compiled artifacts** — two seams, correcting the landed A2 posture
   only for argument declarations:
   - Compile time: `run_compile` (compile.rs) never parsed job documents
     (A2 relied on artifact-startup rejection for required-without-
     default). A4 adds a NARROW compile-time check: when the trailer kind
     is Job, run new `validate_job_declarations_for_compile(text)` in
     document.rs — UNTYPED YAML `Value` parse, extract ONLY the `args:`
     mapping, then `normalize_job_args` (which includes `type` grammar
     and typed-default coercion). On error, compile exits 2 and writes NO
     artifact. The untyped extraction keeps the seam exactly as wide as
     promised: NO document-structure checks (missing `execute:`, unknown
     top-level fields, scalar bodies stay compile-permitted exactly as
     today) and NO execution-value validation — documents that compiled
     before keep compiling unless their ARGUMENT DECLARATIONS are
     invalid. Since `type:` is new surface, no landed A2 document changes
     behavior except pre-existing malformed declarations, which now fail
     earlier in the same exit-2 class.
   - Artifact startup: resolves declared args through the same parser
     path with an empty pair list, so embedded typed defaults coerce
     identically; runtime coercion stays as defense-in-depth. The
     `--arg` surface stays closed on artifacts.

7. **Pre-flight verification note** (e_glm GO): workers must first grep
   for any duplicate strict per-arg schema copy (only
   `job_argument_declaration` is known); a second copy would deny `type:`.

## Affected crates

- camel-cli: `commands/job/document.rs` (type model, declaration parsing,
  declaration-time default check, resolution-time coercion, error
  variants, `validate_job_declarations_for_compile`), `commands/job/
  help.rs` (type column via the shared `JobArgType` renderer),
  `commands/compile.rs` (Job-kind declaration validation before
  write), test modules `document_tests.rs`, `help_tests.rs`,
  `tests.rs` (incl. the batch-mode contract test), and
  `tests/compiled_artifact_test.rs`. No other crate changes.
- Docs (repo root): `CONTEXT-MAP.md` glossary entry `Declared job
  arguments (args:)` admits the optional `type` key (grammar, canonical
  coercion, artifact `--arg` closure unchanged) — keeps
  `lint-context-citations` and holistic docs alignment clean.

## Architecture boundaries

CLI parse/validate layer only (Languages-adjacent console-command surface
per RULING-camel-job-orientation.md: declaration stays DATA in the YAML
schema — no Rust trait, no registry). The DSL interpolation seam is
consumed, not modified (camel-dsl BUSY). No Runtime, Components, or
Services code is touched; exit-code taxonomy stays 2 > 1 > 0.

## Alternatives considered

- `type: enum` + separate `values:` key — rejected: two-key coupling, and
  the bd AC spells `enum[a,b,c]` as one scalar.
- Coercion inside the camel-dsl scanner — rejected: wrong layer (scanner
  is namespace-agnostic), and camel-dsl is BUSY this mission.
- CLI-value validation at declaration parse — impossible: CLI values
  arrive only at resolution; defaults ARE known at parse and are checked
  there (fail-fast, help-honest).
- serde untagged enum for `type` — rejected: manual parsing keeps the
  argument-specific loud diagnostics A2 established.
