# Proposal: guide-foundation-concepts-and-drift-contract

## Why

The mdBook guide skeleton exists but the `concepts/` section is a single bullet-list stub. An evaluator (persona P1/P4) who gets a route running cannot map "Exchange / Route / Component / data-plane vs control-plane" because there is no navigable mental model, and the guide has no drift contract to keep its prose honest as the framework evolves.

The prerequisite change (`context-map-guide-refresh-clause`, committed in this worktree) settled the authority-model refresh trigger. This change builds the first real guide content on that settled model: a navigable concept structure, a curated glossary that cites CONTEXT-MAP, and a mechanically-checkable drift contract that prevents the guide from inventing vocabulary or citing retired ADRs.

A recon of the post-rebase state corrected the original scope: the getting-started anchors are already complete (`hello-world/src/main.rs` and `config-basic/routes/hello.yaml` both carry `ANCHOR: first-route`, and the include pages resolve), and `contributing.md` already states the include-first rule. Those are NOT in scope here. This change does the work that is genuinely missing.

## What Changes

**In:**
- `docs/src/SUMMARY.md` — split the single `Core concepts` entry into an index plus 5 sub-pages (Exchange & Message, Routes & pipelines, Components & endpoints, Data plane vs control plane, Glossary).
- `docs/src/concepts/` — refactor `index.md` into a navigation hub; author `exchange-message.md`, `routes-pipelines.md`, `components-endpoints.md`, `planes.md`, `glossary.md`. Each concept page follows the two-source rule (form via `{{#include}}` from a compiled example, why via ADR citation).
- `docs/src/introduction.md` — append a one-paragraph "coming from Apache Camel?" divergence note citing ADR-0046.
- `docs/src/contributing.md` — add the "Show the form, cite the decision" authoring rule (the two-source rule) to the existing Documentation workflow section.
- `scripts/xtask/src/` — three advisory linters: `lint-glossary` (every bold canonical term in the guide glossary exists in CONTEXT-MAP Key Terms), `lint-slop` (mechanical slop markers), `lint-adr-cite` (every `ADR-00NN` reference in the guide resolves to a non-RETIRED file in `docs/adr/`). All warn-only.

**Out:**
- getting-started page content and anchors (already complete post-rebase).
- `eip/`, `components/`, `yaml-dsl/` recipe content (Phase 2 / change #2).
- `operations/`, `extending/`, `architecture/` content (Phase 3 / change #3).
- Turning any advisory linter into a blocking CI gate (deferred to 1.0).
- Any change to `CONTEXT-MAP.md` Key Terms (the guide is a pure consumer of the term-landing rule).

## Acceptance criteria

- `mdbook build docs` is clean and `mdbook test docs` passes (defers to CI when mdbook is absent locally; `cargo check -p hello-world -p config-basic` and `cargo test -p camel-dsl --test documentation_examples` pass locally).
- Every code/YAML block on the new/edited concept pages is an `{{#include}}` from a compiled example — zero inline code fences that duplicate example content.
- Every bold canonical term in `concepts/glossary.md` matches a CONTEXT-MAP Key Term — exactly or as the first component of a compound Key Term (`cargo xtask lint-glossary --deny` reports zero violations).
- No `ADR-00NN` reference on the new/edited pages points at a missing or RETIRED ADR (`cargo xtask lint-adr-cite --deny` reports zero violations; in particular ADR-0048 is never cited).
- `cargo xtask lint-slop --deny docs/src/concepts docs/src/introduction.md docs/src/contributing.md` reports zero violations (path-filtered to changed files; pre-existing pages like `api-reference.md` are out of scope, whole-tree `--deny` is a 1.0 concern).
- `cargo fmt --check`, `cargo clippy -p xtask -- -D warnings`, and `cargo test -p xtask` pass (the new linters are Rust code with inline unit tests).
- `openspec validate guide-foundation-concepts-and-drift-contract --type change --json` reports no delta-structure errors.

## Risk budget

Acceptable: new xtask linters that scan markdown (mechanical, testable); concept pages that paraphrase CONTEXT-MAP terms (wording may drift, vocabulary is linted); STE judgment left to authors (advisory, not blocking).

Out of bounds: changing the authority order or any ADR; editing CONTEXT-MAP.md Key Terms; turning advisory linters blocking; adding guide sections beyond `concepts/`, `introduction.md`, `contributing.md`.
