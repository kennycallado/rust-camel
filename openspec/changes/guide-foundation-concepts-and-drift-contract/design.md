# Design: guide-foundation-concepts-and-drift-contract

## Approach

Fill the `concepts/` section into a navigable mental model and install the drift contract that keeps the guide honest. Every concept page follows one template, derived from the prerequisite's authority model:

> **Show the form, cite the decision.** A page demonstrates current *form* with an anchored `{{#include}}` from a compiled example (tier-1-backed, `mdbook test` + `cargo check` fail on drift), then links the governing ADR for the *why* with a one-sentence paraphrase. The page's own prose is connective narrative only.

The same template is written into `contributing.md` as the project's authoring rule.

### Concept page structure

`concepts/index.md` becomes a navigation hub (one paragraph + links). Five sub-pages:

| Page | Form (include source) | Why (ADR citation) |
|------|----------------------|--------------------|
| `exchange-message.md` | `hello-world` `set_header` + the exchange flow | ADR-0024 (PipelineOutcome); Message≠Exchange per CONTEXT-MAP |
| `routes-pipelines.md` | `hello-world` `RouteBuilder::from(...).to(...)` | ADR-0001 (Tower data plane); ADR-0024 (Stop = control flow) |
| `components-endpoints.md` | `hello-world` `register_component(...)` + URI `timer:tick?period=` | ADR-0015 (PollingConsumer vs event-driven Consumer) |
| `planes.md` | the `hello-world` pipeline include (the data plane in action) | ADR-0001 (planes split); ADR-0045 (bounded context) |
| `glossary.md` | none (curated prose, not form) | each entry cites CONTEXT-MAP Key Terms + defining ADR |

`planes.md` demonstrates the data plane through the SAME `hello-world` pipeline include used by `routes-pipelines.md` (the route pipeline IS the data plane) and cites ADR-0001 for the *why* of the two-plane split. It does NOT assert the `Service<Exchange>` trait signature as a form-claim — that deeper contract belongs in Phase-3 `architecture/`, and asserting it here would require an include no compiled example currently provides. By limiting planes.md to a form-claim the existing include already satisfies (the pipeline shape) plus ADR citation for the rationale, the page honors the two-source rule with no new example and no escape hatch.

### Glossary

`concepts/glossary.md` has two tiers, matching CONTEXT-MAP's own split between cross-cutting Key Terms and crate-local terms:

- **Cross-cutting terms (bold, lint-checked):** the user-facing subset of CONTEXT-MAP Key Terms — Message, ErrorHandler, CircuitBreaker, ExceptionDisposition, SecurityPolicy, Degraded, PollingConsumer, PipelineOutcome. The glossary writes the SHORT canonical form as the bold term (e.g. `**Degraded**`, `**ErrorHandler**`); CONTEXT-MAP Key Terms are sometimes compound labels (`ErrorHandler / ErrorHandlerConfig / ExceptionPolicy`, `Degraded / Unhealthy`), and `lint-glossary` matches a bold term if it equals the FIRST component of a compound Key Term, so `**Degraded**` matches the `Degraded / Unhealthy` entry. The definition prose may spell out the full compound. Bold terms are line-anchored (begin a glossary entry line) so inline labels are not matched. Each is a `**Term**` bold entry; vocabulary cannot drift, wording may.
- **Foundational primitives (cited, not lint-checked):** Route, Exchange, Endpoint, Component, Processor, EIP, Consumer, Producer. These are crate-local terms owned by the contract/runtime crates. Correct owners: `Exchange`/`Processor` contract → `crates/camel-api/CONTEXT.md`; `Component`/`Endpoint`/`Consumer`/`Producer` → `crates/components/CONTEXT.md` (the parent; `camel-component-api/CONTEXT.md` only covers their SPI view); `Route` → `crates/camel-core/CONTEXT.md`; `EIP` (pattern implementations) → `crates/camel-processor/CONTEXT.md`. Each entry links its owning `CONTEXT.md`; the worker verifies the exact file during implementation. They are NOT bold (so `lint-glossary` skips them); their vocabulary is Camel-standard and low-drift.

Every entry gives a one-sentence user-facing definition and cites its authority. Implementer-internal terms (OutcomeSegment, synchronous-projection CQRS, module-discipline ceiling, …) stay OUT of the guide glossary; they surface only in Phase-3 `architecture/`.

### Camel-divergence note

`introduction.md` gains one paragraph: rust-camel is Apache-Camel-inspired (ADR-0046), not drop-in; EIP vocabulary is the compat surface. A dedicated `coming-from-camel.md` page is Phase 2.

### The three advisory linters

All in `scripts/xtask/src/main.rs`, following the existing `pub fn lint_<x>(workspace_root: &Path) -> Result<Vec<Violation>, String>` pattern, wired as `cargo xtask lint-glossary|lint-slop|lint-adr-cite` subcommands. All warn-only (print violations, exit 0 unless a `--deny` flag is passed; default advisory).

- **`lint-glossary`**: parse `docs/src/concepts/glossary.md` for bold canonical terms (`**Term**`). For each, match the term against the Key Terms section of `CONTEXT-MAP.md`: a bold term matches if it equals the FIRST component of a Key Term entry (`- **Term** —` or `- **Term / …** —`), so compound labels (`ErrorHandler / …`) match `**ErrorHandler**`. Violation = bold term whose first-component is not any Key Term's first component. Foundational primitives (Route, Exchange, …) are intentionally NOT bold, so they are skipped. Unit-test fixtures: exact match, compound-label first-component match, and a non-matching term.
- **`lint-slop`**: walk `docs/src/**/*.md`, optionally filtered by path arguments (`cargo xtask lint-slop [PATH...]` — paths may be files or directories; directories recurse over `*.md`; when paths given, only those are scanned; no paths = whole tree). Flag the mechanical slop markers: em-dash (`—`), banned verbs (`leverage`, `utilize`, `facilitate`, `prior to`), buzzwords (`seamless`, `robust`, `powerful`). (`ensure` is NOT on the list.) Content inside ANY fenced code block (```` ``` ```` or `~~~`) is exempt. The acceptance gate for this change runs `cargo xtask lint-slop --deny docs/src/concepts docs/src/introduction.md docs/src/contributing.md` (path-filtered to changed files), NOT whole-tree, because pre-existing pages (`api-reference.md` has em-dash hits) are out of scope here.
- **`lint-adr-cite`**: walk `docs/src/**/*.md`, optionally filtered by path arguments (same file/directory semantics as `lint-slop`; no paths = whole tree), for `ADR-00NN` tokens. For each, confirm `docs/adr/00NN-*.md` exists. Status parsing handles all on-disk formats: bare `Status:` lines, `**Status:**` (colon inside bold), `**Status**:` (colon outside bold), `- **Status:**` (list-item bold), `## Status` heading lines, and statusless legacy files (ADR-0001 has no status line). A statusless ADR is treated as active (non-violation) with a logged note; a `Retired`/`Superseded` status is a violation (would catch an accidental ADR-0048 citation). Unit-test fixtures cover one of each format plus the statusless case.

## Affected crates

- `scripts/xtask` (modified): three new lint functions + three new subcommand arms in `main()` + unit tests for each (the existing linters carry inline `#[test]` blocks — match that style).
- No runtime crate touched. No `Cargo.toml` dependency changes (linters use std + the existing walkdir/regex already in xtask, if present; otherwise std `fs` + manual matching).

## Architecture boundaries

Respects every boundary by construction — no Runtime, DSL, Components, Services, Languages, or Functions code changes. The linters are build tooling in `xtask`, which already owns the lint surface. The guide content is documentation under `docs/src/` (git-tracked via the `.gitignore` un-ignore of `docs/src/`).

Relevant ADRs (cited by the guide, not modified): ADR-0001 (planes), ADR-0015 (PollingConsumer), ADR-0024 (PipelineOutcome/Stop), ADR-0045 (bounded context), ADR-0046 (Camel inspiration), ADR-0049 (`#[non_exhaustive]` — motivates the refresh clause already landed).

## mdbook availability

`mdbook` is not installed in this environment. `mdbook build docs` / `mdbook test docs` defer to CI. The local gates that DO run: `cargo check -p hello-world -p config-basic` (the included examples compile), `cargo test -p camel-dsl --test documentation_examples` (include resolution + parse), and the new xtask linters. The include-resolution correctness is partially covered by `documentation_examples.rs` (which already parses the `{{#include}}` targets).

## Phases

Single-phase. All five work items (SUMMARY, concept pages, Camel note, contributing rule, linters) form one coherent deliverable: the foundation + drift contract that later changes inherit. No `## Phase N` headings.

## Alternatives considered

1. **Multi-phase split** (concepts / linters as separate phases). Rejected: the drift contract (linters + contributing rule) and the concept content are mutually load-bearing — the glossary needs `lint-glossary` to be safe, and the linters need the glossary to have something to check. Splitting forces one to land without its guard.
2. **Generate glossary from CONTEXT-MAP.** Rejected (per architect Rec 2): no stable machine schema for ~40 terms pre-1.0; curation is the needed work.
3. **Blocking linters now.** Rejected: advisory-first protects contributor velocity pre-release; flip to `--deny`/CI-blocking at 1.0.
