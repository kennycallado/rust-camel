# Design: validate-partner-self-declare

## Approach

The load-bearing element is not the cross-check — it is the **wired
reference feeding `bind_partners` + `fill_bind_vars` + the env fold**.
A partner validate target that self-declares must reach the same wiring
path a `send`/`receive` ref takes. Three code sites encode the
send/receive-only rule today; all three gain a gated Partner arm:

1. `crates/camel-cli/src/commands/test/scenario.rs`
   `wire_endpoint_refs()`: add an arm collecting
   `Validate { target: Partner(ep) }` when `ep.provisioning ==
   Some(Harness)`. The existing `seen` endpoint-string dedup prevents
   double-wiring when a send/receive also names the URI. This single
   change routes the ref into `bind_partners` (scripted via the
   `partners:` entry, else permissive), the `harness_provisioned` env
   fold (`bindVar -> http://bound-addr`), and `fill_bind_vars`.
2. `crates/camel-integration-test/src/document.rs` cross-check (i)
   (~:793): the collector gains a `Validate { Partner }` arm that
   pushes the URI into `harness_uris` **iff the ref is object-form
   `provisioning: harness` AND a `partners:` entry names the URI**
   (both doc-visible facts; the parser already accepts the object form
   on `http:` refs — `endpoint_from_raw` — so no parse change). The
   post-loop check stays: any partner URI not in `harness_uris` still
   fails `doc-validation`, exit 2, naming the URI. Object-form refs
   WITHOUT a `partners:` entry fall back to the string-equality rule;
   their error message teaches every escape (add a `partners:` entry,
   add `provisioning: harness`, or declare via send/receive).
3. `crates/camel-integration-test/src/document.rs` `bindings()`
   (~:182): `ScenarioTarget::Partner(ep) => endpoint_bindings(ep)`
   gated on `provisioning: harness` (plain-string targets stay inert —
   byte-identical for existing docs). Sole caller is the
   reserved-env-key check; a validate `bindVar` SHOULD reserve its key.

No runner, adapter, poll, or deadline changes: once the adapter exists,
`recorded_requests(key)` reads it; ordering is already safe
(`bind_partners` -> env fold -> `boot_scenario` -> `fill_bind_vars`),
and the route reaches the partner through the env tier set at boot.

Canonical spec amendment (`integration-tier/spec.md`, requirement
"Partner request verification"): the declaration sentence admits the
object-form self-declaration; the "undeclared URI" scenario narrows to
the still-failing shapes; a new scenario codifies the pilot repro
(validate-only reference, loads, binds, fills bindVar, runs green).

## Affected crates

- camel-integration-test: `document.rs` — cross-check (i) relaxation
  with `partners:`-key guard; `bindings()` Partner arm. Parse tests.
- camel-cli: `scenario.rs` — `wire_endpoint_refs` Partner arm;
  driver/boot test reproducing the pilot shape end-to-end.

## Architecture boundaries

Testtooling only. The parse/validate contract lives in
camel-integration-test (library truth); the CLI driver owns wiring and
env fold — no Runtime, DSL, Components, Services, Languages, or
Functions crate is touched. The rule "wired harness `http` refs are the
only keys a `partners:` entry may name" is preserved (the
reverse-check composes: a `partners:` key naming a self-declared
validate URI passes because the URI is wired). Hexagonal boundaries
unchanged.

Single-phase change (no `## Phases` section).
