# Plan Blessing Verdict: consolidate-uri-metadata

**Plan hash:** sha256:e031e4908bab22d16d8def26db83068e4efa7ab9d4805f5f35b1a54d753ad7e9
**Reviewer mode:** self-grill-proposals (advisory)
**Verdict:** **BLESS-WITH-FIXES**

The plan is structurally sound, spec-complete, and the r_glm C1/I1–I6 findings are
concretely resolved. Two new Critical issues surfaced during code cross-reference that
r_glm did not catch, plus minor gaps. None are architectural; all are localized and
cheap to fix in the plan text before implementation.

---

## Verification against the 6 blessing questions

1. **C1 wiring fix adequate?** YES. The "Migration delegation convention" preamble
   (tasks.md L3–11) is explicit, names the exact fn signature, and every migration task
   (1.6, 2.1–2.6, 3a.1–3a.4, 3b.1–3b.3) carries a "Wire … delegation" step. Verified
   against the real split: `Component::metadata()` default lives at
   `crates/components/camel-component-api/src/component.rs:21` returning
   `ComponentMetadata::minimal(self.scheme())`; config structs are separate types.
   The delegation model matches reality.

2. **I1–I6 resolved?** YES, all six concretely:
   - I1 (aliases ExprArray): Task 1.1 step 2 replaces `Punctuated<KeyValue>` with a
     custom loop + `AttrValue { Lit, Array }`. Verified current parser at
     `uri_config.rs:17-30` is exactly the `Lit`-only `KeyValue` that would reject arrays.
   - I2 (bare-flag): Task 1.1 step 2 accepts bare ident OR key=value. Correct.
   - I3 (trybuild): Task 1.3 adds `trybuild` dev-dep + `tests/ui/`. Confirmed trybuild
     is NOT yet present and `tests/` dir does NOT exist — this is genuinely new infra.
   - I4 (syn-based lint): Task 3b.4 uses `syn` AST walk skipping `#[cfg(test)]`, not
     ripgrep. Sound — a glob grep cannot exclude test modules reliably.
   - I5 (Option coherence): Task 1.2 test `infer_option_required_false` + spec
     Requirement "Required-flag coherence with Option types". Covered.
   - I6 (seda/direct skip_impl): Tasks 3a.1/3a.2 mandate `skip_impl` + regression tests
     for bespoke validation. Verified seda/direct use free-function `from_uri`
     (seda L84, direct L77) with no trait impl — skip_impl is the correct vehicle.

3. **Spec coverage — every Requirement/Scenario owned?** YES. Mapping:
   - Macro-derived uri_options → 1.3; OptionKind inference → 1.2; Semantic keys → 1.1;
     Secret+default reject → 1.3 step 5; Opt-in metadata → 1.4; Delegation → every
     migration task + preamble; ComponentMetadata builders → 1.5; Kind override
     validation → 1.2 step 3; Required/Option coherence → 1.2; Single source of truth →
     3b.4 lint; No-dup names → 2.7; List inference → 1.2; MODIFIED catalog visibility →
     1.6 + 2.7 + 3b.4. No orphan requirement.

4. **Phase-boundary coherence?** YES. tasks.md phases (P1 7, P2 7, P3a 4, P3b 5 = 23)
   match design.md §Phases exactly. Dependencies are linear and correct
   (P2 depends on P1 reference; P3b depends on P3a proving skip_impl).

5. **Autopilot feasibility?** MOSTLY. 22 of 23 tasks are single-worker-pass sized.
   Task 3b.1 (HttpEndpointConfig) is the riskiest — see Important I-b. Task 1.1+1.2+1.3
   form a tight coupled cluster in one file; acceptable but see Minor.

6. **trybuild right mechanism?** YES. trybuild `compile_fail` is the standard, correct
   tool for asserting proc-macro compile errors (secret+default, kind-typo, unknown-key,
   no-optin). A runtime `#[test]` cannot assert a compile failure. Approved.

---

## Findings

### CRITICAL

#### C-NEW-1: Generated metadata types are unreachable via the `crate` path

The macro resolves generated code through the `crate = ".."` override (default
`camel_endpoint`; component crates set `camel_component_api` —
`uri_config.rs:159-160`, documented in `lib.rs:21-22`). Task 1.3/1.4 generate code
referencing `UriOption`, `OptionKind`, `ComponentMetadata`, `ComponentCapabilities`.

**But these four types are NOT re-exported by `camel_component_api`.** Verified
`camel-component-api/src/lib.rs:50-58` re-exports only `camel_api::{CamelError, Exchange,
…}` and `camel_endpoint::{UriComponents, UriConfig, parse_uri}` — none of the metadata
types. Components today import them via a *direct* path:
`camel_api::component_metadata::{ComponentCapabilities, ComponentMetadata, OptionKind,
UriOption}` (verified `camel-timer/src/lib.rs:20-22`).

Generated `Self::uri_options()` / `Self::metadata()` bodies emitting
`#endpoint_crate::UriOption::new(..)` will therefore **fail to compile** in every
component crate, because `camel_component_api::UriOption` does not exist.

**Required fix (add as a new Task 1.0 or fold into 1.5, before any generation task):**
add re-exports to `camel-component-api/src/lib.rs`:
`pub use camel_api::component_metadata::{ComponentCapabilities, ComponentMetadata,
OptionKind, UriOption};` — OR have the macro emit fully-qualified
`::camel_api::component_metadata::…` paths for metadata types (independent of the
`crate` override, which governs only `UriConfig`/`UriComponents`/`CamelError`). The plan
must pick one and state it; today it silently assumes the types are reachable via
`#endpoint_crate`, which is false. This is the exact class of "config-vs-component wiring
gap" C1, one layer deeper (config-vs-crate-path).

#### C-NEW-2: `parse_nested_meta` cannot parse the `metadata(..)` sub-list as specified

Task 1.4 step 1 says parse `#[uri_config(metadata(scheme = "sql", description = "..",
producer, consumer, polling_consumer, streaming))]` "via `parse_nested_meta`". The
current `parse_uri_config_attr` (`uri_config.rs:134`) uses
`attr.parse_nested_meta(|meta| …)` with `meta.path.is_ident("skip_impl")` /
`"crate"`. A nested `metadata(...)` group requires calling `meta.parse_nested_meta(..)`
*recursively* on the inner list AND mixing bare-flag idents (`producer`) with
`key = value` (`scheme = ".."`) inside it — the same bare-vs-kv duality that Task 1.1
explicitly restructures the *field* parser to handle. Task 1.4 does not state that the
struct-attr parser needs the same treatment; a naive `meta.value()?` on `producer` (no
`=`) will error.

**Required fix:** Task 1.4 must specify the nested-meta handling explicitly: on
`meta.path.is_ident("metadata")`, call `meta.parse_nested_meta` for the inner group, and
inside it branch on `meta.value().is_ok()` (kv) vs bare ident (capability flag), mirroring
Task 1.1's loop. Add a unit test `metadata_mixed_flags_and_kv` and a trybuild
`metadata_unknown_key_fail.rs`.

### IMPORTANT

#### I-a: `#[uri_scheme]` already required — skip_impl adoption tasks must add it

Tasks 3a.1–3a.4 and 3b.1–3b.3 add `#[derive(UriConfig)]` + `#[uri_config(skip_impl)]`.
`impl_uri_config` calls `extract_scheme(&input.attrs)` (`uri_config.rs`, referenced
L71) which **requires** `#[uri_scheme = ".."]` and errors if absent. Tasks 3a.1/3a.2/3a.3
list `#[uri_scheme = "…"]`, good — but Task 3a.4 (mock) branch "otherwise add derive"
and Tasks 3b.2/3b.3 must also carry `#[uri_scheme]`. 3b.1/3b.2/3b.3 do include it.
Confirm mock's derive branch includes `#[uri_scheme = "mock"]` (currently only shown in
the "otherwise" clause). Minor wording tightening; flagged Important because a missing
scheme is a hard compile error.

#### I-b: Task 3b.1 blast radius — split recommended for autopilot

`HttpEndpointConfig` has bespoke `from_components` (L189), an auth-required-param
matrix (L364–375), 39 parse tests + `from_uri_with_defaults` at ~21 sites. Annotating
every field with `#[uri_param]` matching manual parsing, while keeping the manual impl,
is the single largest cognitive step. `skip_impl` generates `parse_uri_components` +
`uri_options()` but the manual `impl UriConfig` already defines `from_components`; the
task must confirm **no symbol collision** between generated `parse_uri_components` and the
manual impl (skip_impl generates an *inherent* `parse_uri_components` — verified
`uri_config.rs:686` emits `pub fn parse_uri_components` in an inherent impl, while the
manual trait impl provides `from_components`; these coexist). Recommend the plan add an
explicit acceptance line: "generated inherent `parse_uri_components` does not clash with
manual `impl UriConfig`; `uri_options()` is the only consumed output." Consider allowing
a worker-escalation checkpoint after 3b.1 before 3b.2/3b.3.

#### I-c: Duplicate-name invariant runs too late

Task 2.7 (`no_duplicate_option_names`) covers Phase-2 schemes only. Phase 3a/3b add
direct/seda/log/http where hand-mapping `#[uri_param(name=..)]` to legacy param names is
error-prone (e.g. http auth trio). The final gate 3b.4 asserts catalog presence but NOT
per-scheme name uniqueness. **Fix:** extend 3b.4's `all_components_in_catalog` (or add a
sibling test) to re-assert `no_duplicate_option_names` across all 12 schemes.

### MINOR

- **M-1 (Task 1.6):** sql config has enum fields (`SqlOutputType`, `TransactionMode`
  with `FromStr`, verified `config.rs:31-72`). Per the inference rule these infer
  `String` (correct, guardrail G1). The task should note these fields explicitly so the
  worker does not reach for `kind = "enum:.."` unprompted — the reference migration sets
  the pattern for all others.

- **M-2 (Task 1.1/1.2/1.3):** three tasks mutate the same `uri_config.rs` in sequence.
  Fine, but 1.1 and 1.2 both touch the parse/inference boundary; recommend 1.1→1.2→1.3
  land as one worker session (or one commit chain) to avoid an intermediate
  non-compiling state where `uri_options()` references not-yet-added `UriParamAttr`
  fields.

- **M-3 (Task 3b.5):** acceptance uses `rg -c 'uri_options|skip_impl|inference|
  delegation'` — an OR count that passes if any one term appears. Tighten to require all
  four terms (`&&` of per-term counts) so the ADR amendment is genuinely complete.

- **M-4 (Task 3a.4 mock):** "Empty `uri_options` is legitimate" conflicts with 3b.4's
  `all_components_in_catalog` note "mock may have empty uri_options; rest non-empty" —
  self-consistent, good, but ensure the delegation still sets scheme/description so
  `get_metadata("mock")` is non-`None`.

---

## Self-grill records

### Proposal DP-1: "C1 delegation convention closes the config-vs-component gap"

**Questions:**
1. [glossary] Does "delegation" as used in the preamble match how `Component::metadata()`
   actually resolves in the catalog?
2. [sharpen] Is "the config struct's inherent `metadata()`" a single precise target, or
   are there two generation modes (opt-in vs compose) that a worker could confuse?
3. [scenario] If a worker adds the `#[uri_config(metadata(..))]` attr but forgets the
   delegation line, does any test catch it?
4. [cross-ref] Does the code confirm the default `Component::metadata()` returns
   `minimal` (empty options), making delegation strictly necessary?

**Answers:**
1. Match. `component.rs:21` default returns `minimal(self.scheme())`; catalog harvests
   `Component::metadata(&self)`. Delegation to config inherent `metadata()` is the only
   way options reach the catalog. (`camel-component-api/src/component.rs:21-22`)
2. Two modes exist (preamble L8 full delegate; L10 compose-with-uri_options). A worker
   could pick compose when the config has opt-in metadata and lose capabilities/
   description. Preamble says "or, if the component has no opt-in" — acceptable, but M-1
   reference task should model the full-delegate path. (`tasks.md:8-10`)
3. Yes: each migration task's `*_metadata_nonempty` test asserts
   `get_metadata(scheme).uri_options` non-empty — a forgotten delegation yields empty
   options and fails. Good coverage. (`tasks.md:173, 220, 243, …`)
4. Confirmed. (`camel-component-api/src/component.rs:21-22`)

**Outcome:** confirm (delegation model correct) — but surfaced C-NEW-1 during cross-ref:
delegation compiles only if the generated types resolve, which they do not.

### Proposal DP-2: "Task 1.4 metadata(..) opt-in generates the Component override"

**Questions:**
1. [glossary] Is `#[uri_config(metadata(..))]` a new sub-attribute the current parser
   understands?
2. [sharpen] Does "generate `fn metadata()` on the config struct" mean inherent fn or a
   `Component` impl? The spec text at Requirement "Opt-in metadata" says "generate a
   `Component::metadata()` override" — is that consistent with tasks.md?
3. [scenario] Can `parse_nested_meta` parse `metadata(scheme="x", producer)` mixing kv
   and bare flag as-is?
4. [cross-ref] Does the existing parser handle nested groups?

**Answers:**
1. No — current parser only knows `skip_impl` and `crate`
   (`uri_config.rs:135, 140`). New branch required (task acknowledges this).
2. **Inconsistency.** spec.md:93-96 says "generate a `Component::metadata()` override";
   tasks.md:117 + design.md:9-11 say generate an *inherent* `fn metadata()` on the
   *config* struct, then the component delegates. The inherent-fn model is the coherent
   one (config ≠ component type). The spec wording "Component::metadata() override" is
   loose but the delegation Requirement (spec.md:110-121) reconciles it. Not a blocker;
   note for ADR precision (M-3).
3. No — see C-NEW-2. Bare `producer` has no `= value`; naive `meta.value()?` errors.
4. Only flat `is_ident` checks; no recursive `parse_nested_meta`. Confirms C-NEW-2.
   (`uri_config.rs:134-146`)

**Outcome:** refine — raise C-NEW-2 (parser must handle nested mixed group) and note the
spec/tasks wording reconciliation.

### Proposal DP-3: "skip_impl adoption for seda/direct/log/http preserves parse"

**Questions:**
1. [glossary] Is `skip_impl` semantics ("generate parsing helper, keep manual trait
   impl") accurate?
2. [sharpen] For free-function `from_uri` (seda/direct), is there a `trait UriConfig`
   impl at all to keep, or is skip_impl generating an inherent helper only?
3. [scenario] Does generated `parse_uri_components` collide with anything in http's
   manual impl?
4. [cross-ref] Do seda/direct actually lack a trait impl (justifying "adopt from
   scratch")?

**Answers:**
1. Accurate. `skip_impl` at `uri_config.rs:680` emits inherent `pub fn
   parse_uri_components` + a `from_uri` that calls it; NOT the full trait derivation.
2. seda/direct have free-function `from_uri` (seda L84, direct L77), no trait impl.
   skip_impl generates inherent helpers; the free fn stays or becomes trait impl. Tasks
   3a.1/3a.2 say "retain free-function logic as a manual `impl UriConfig`" — workable.
3. http manual impl provides `from_components` (trait method, L189); generated inherent
   `parse_uri_components` is a different name — no collision. See I-b for explicit
   acceptance line.
4. Confirmed via grep (seda L71/L84, direct L66/L77 — struct + free fn, no `impl
   UriConfig`).

**Outcome:** confirm — with I-b acceptance-line addition.

### Proposal DP-4: "trybuild is the right compile-fail mechanism"

**Questions:**
1. [glossary] Is trybuild the project's established pattern for compile-fail?
2. [sharpen] Does adding a dev-dep to one proc-macro crate affect workspace
   `cargo-audit`/lints?
3. [scenario] Will trybuild `.stderr` snapshots be brittle across rustc versions?
4. [cross-ref] Does any existing crate already use trybuild (pattern to copy)?

**Answers:**
1. It is the ecosystem standard; no runtime alternative can assert compile failure.
2. New dev-dep only; cargo-audit runs workspace-wide — trybuild is well-audited, low
   risk. No lint impact (dev-dep).
3. Real risk: `.stderr` files are rustc-version-sensitive. **Mitigation the plan should
   state:** commit `.stderr` with `TRYBUILD=overwrite` on the CI toolchain, or assert
   only that compilation fails (trybuild does fail-without-stderr-match if no `.stderr`
   present). Recommend the latter for the three cases to avoid snapshot churn. (Minor —
   fold into Task 1.3.)
4. None found (`grep trybuild` empty). This is first use — Task 1.3 correctly treats it
   as new infra.

**Outcome:** confirm — add the `.stderr` brittleness mitigation note to Task 1.3.

---

## Summary

**BLESS-WITH-FIXES.** Resolve **C-NEW-1** (re-export or fully-qualify metadata types —
without it Phase 1 does not compile in any component crate) and **C-NEW-2** (nested
mixed-meta parsing for `metadata(..)`) in the plan before autopilot starts. Address
Important I-a/I-b/I-c and Minors M-1..M-4 as plan-text tightening. The C1/I1–I6
remediation from the prior cycle is genuinely complete; spec coverage and phase coherence
are clean.
