# Tasks: guide-foundation-concepts-and-drift-contract

<!--
  Single-phase change. No `## Phase N` heading: one coherent deliverable
  (the foundation + drift contract that later changes inherit). The absence
  of phase headings matches design.md's single-phase declaration.
  Task order is deliberate: linters (1.1) are built first with inline
  fixtures, so the concept-page and glossary tasks (1.3-1.5) can be verified
  by running the linters against the real pages.
-->

## xtask linters

### Task 1.1: Add lint-glossary, lint-slop, lint-adr-cite advisory linters

**Files:**
- `scripts/xtask/src/main.rs` (modified)

**Steps:**
1. Add three `pub fn` linters following the existing `lint_unwrap`/`lint_non_exhaustive` pattern. `lint_glossary(workspace_root: &Path) -> Result<Vec<Violation>, String>` scans the fixed glossary file (no path filter). `lint_slop(workspace_root: &Path, paths: &[PathBuf]) -> Result<Vec<Violation>, String>` and `lint_adr_cite(workspace_root: &Path, paths: &[PathBuf]) -> Result<Vec<Violation>, String>` take an explicit path list (empty = walk all of `docs/src/`). Each has a `--deny` mode (non-zero exit when violations exist) vs default advisory (print, exit 0).
2. **`lint_glossary`**: read `docs/src/concepts/glossary.md`. Extract bold canonical terms that BEGIN a glossary entry line — for each line matching `^\s*(?:-\s*)?\*\*([A-Z][A-Za-z]+)\*\*`, capture group 1. (Line-anchoring prevents false positives on inline labels like `**Authority**:` or `**Note**:`.) Read `CONTEXT-MAP.md` and extract Key Term first-components ONLY from the `## Key Terms` section — from the `## Key Terms` heading line up to (not including) the next `## ` level-2 heading — matching `^- \*\*([^*/]+?)[ */]`. A bold term is a violation if its value is not among the captured first-components. (Non-bold primitives are not matched, so skipped automatically. The glossary writes the SHORT canonical form, e.g. `**Degraded**` not `**Degraded / Unhealthy**`, so the letters-only regex captures it; the definition prose may spell out both states.)
3. **`lint_slop`**: accept optional path args (one or more file or directory paths, passed after the subcommand); paths may be files or directories (directories recurse over `*.md`); no paths = walk all of `docs/src/`. For each file, scan line-by-line. A fence line is one that, ignoring up to 3 leading spaces, begins with ```` ``` ```` or `~~~` (this matches info-string fences like ```` ```rust,ignore ````); toggle in/out of fence mode on fence lines, and skip slop detection while inside a fence. Flag hits for: em-dash `—`; whole-word `leverage`, `utilize`, `facilitate`, `prior to`; whole-word `seamless`, `robust`, `powerful`. (`ensure` is NOT flagged.) Return one Violation per hit with file + line.
4. **`lint_adr_cite`**: accept optional path args (same file/directory semantics as `lint-slop`; no paths = walk all of `docs/src/`). Find `ADR-00\d\d` tokens. For each, glob `docs/adr/00NN-*.md`. If no file: violation "unresolved". If file exists, parse status by checking (in order) for a line matching `^Status:\s*(\w+)`, `^\*\*Status:\*\*\s*(\w+)`, `^\*\*Status\*\*:\s*(\w+)`, `^-\s*\*\*Status:\*\*\s*(\w+)` (list-item bold form), or a `## Status` heading followed by a status word. If no status line found: treat as active (not a violation), log a note. If status word (case-insensitive) is `retired` or `superseded`: violation "retired/superseded".
5. Wire three subcommand arms in `main()` (`LintGlossary`, `LintSlop`, `LintAdrCite`) parsing `--deny`; `LintSlop` and `LintAdrCite` also parse trailing path args (optional path filters), while `LintGlossary` scans the fixed glossary file. Mirror the existing subcommand dispatch.
6. Add inline `#[test]` blocks for each linter (match the existing inline-test style in `main.rs`): `lint_glossary` (exact match passes, compound first-component match passes, non-matching term violates, inline `**Authority**:` label not matched); `lint_slop` (em-dash outside fence violates, em-dash inside a ```` ``` ```` fence exempt, em-dash inside a ```` ```rust,ignore ```` fence exempt, banned verb violates); `lint_adr_cite` (existing+active passes, retired violates, missing-file violates, statusless passes).

**Tests:** (Rust unit tests, inline in main.rs, named fixtures per case)
- `lint_glossary_compound_alias_matches`: glossary fixture line `**ErrorHandler** — def` + CONTEXT-MAP fixture (inside `## Key Terms`) `- **ErrorHandler / ErrorHandlerConfig / ExceptionPolicy** — wraps error policy` → 0 violations.
- `lint_glossary_inline_label_not_matched`: glossary fixture with an inline `**Authority**:` on a non-entry line → the inline label is NOT extracted → 0 violations.
- `lint_glossary_non_matching_violates`: glossary entry `**FooBar**` + CONTEXT-MAP `## Key Terms` with no FooBar → 1 violation.
- `lint_glossary_section_bounded`: a term that appears in CONTEXT-MAP `## Relationships` but NOT in `## Key Terms` → still a violation (extraction must not cross section boundary).
- `lint_slop_emdash_inside_infostring_fence_exempt`: fixture with a ```` ```rust,ignore ```` opener, an em-dash inside, then a closer → 0 violations.
- `lint_slop_emdash_outside_fence_violates`: em-dash in prose → 1 violation.
- `lint_slop_banned_verb`: fixture `leverage` outside a fence → 1 violation.
- `lint_adr_cite_active_each_format`: five fixtures citing an active ADR whose file uses respectively `Status: Accepted`, `**Status:** Accepted`, `**Status**: Accepted`, `- **Status:** Accepted`, and a `## Status` heading followed by a blank line then `Accepted` (matching on-disk ADR-0036) → 0 violations each.
- `lint_adr_cite_retired`: fixture citing `ADR-0048` + a `docs/adr/0048-*.md` with `**Status**: Retired` → 1 violation.
- `lint_adr_cite_statusless`: fixture citing an ADR whose file has no status line → 0 violations, note logged.
- `lint_adr_cite_missing`: fixture citing `ADR-0099` with no matching file → 1 violation "unresolved".
- `lint_slop_path_filter`: two files, one with a hit; call with `paths` selecting only the clean file → 0 violations; select the dirty file → 1 violation; select a parent directory containing both → recursively scans both files' `*.md` and reports the dirty one's hit.
- `lint_adr_cite_path_filter`: two pages, one citing a retired ADR; call with `paths` selecting only the clean page → 0 violations; select the dirty page → 1 violation; select a parent directory → recursively scans both and reports the dirty page's violation.

**Acceptance:**
- `cargo fmt --check`, `cargo clippy -p xtask -- -D warnings`, `cargo test -p xtask` all pass.
- `cargo xtask lint-glossary`, `cargo xtask lint-slop`, `cargo xtask lint-adr-cite` each run without panic (advisory exit 0 even with hits).

- [x] 1.1

## docs/concepts structure

### Task 1.2: Split SUMMARY concepts entry + refactor concepts/index.md to navigation hub

**Files:**
- `docs/src/SUMMARY.md` (modified)
- `docs/src/concepts/index.md` (modified)

**Steps:**
1. In `docs/src/SUMMARY.md`, replace the single `- [Core concepts](concepts/index.md)` line with an index plus five sub-page entries:
   ```
   - [Core concepts](concepts/index.md)
     - [Exchange & Message](concepts/exchange-message.md)
     - [Routes & pipelines](concepts/routes-pipelines.md)
     - [Components & endpoints](concepts/components-endpoints.md)
     - [Data plane vs control plane](concepts/planes.md)
     - [Glossary](concepts/glossary.md)
   ```
2. Rewrite `docs/src/concepts/index.md` as a short navigation hub: one paragraph framing the mental model (route = source → pipeline → sink; exchange flows through it; components own URI schemes), then a bulleted list linking the five sub-pages with a one-line "what you learn" each. Remove the existing 5-bullet definition list (those definitions move into the sub-pages/glossary). No form-claims that need includes on the index page — it is navigation only.

**Tests:** (verification — shell-based)
- `summary-lists-subpages`: `rg -c 'concepts/(exchange-message|routes-pipelines|components-endpoints|planes|glossary)\.md' docs/src/SUMMARY.md` → 5.
- `index-is-navigation`: `rg -c 'concepts/(exchange-message|routes-pipelines|components-endpoints|planes|glossary)' docs/src/concepts/index.md` → at least 5 (the hub links all sub-pages).
- `lint-slop-clean`: `cargo xtask lint-slop --deny docs/src/concepts/index.md` reports 0 violations.
- `validate-clean`: `openspec validate guide-foundation-concepts-and-drift-contract --type change --json` → `"valid": true`.

**Acceptance:**
- `rg -c 'concepts/(exchange-message|routes-pipelines|components-endpoints|planes|glossary)\.md' docs/src/SUMMARY.md` returns 5.
- The five sub-page files exist (created by tasks 1.3-1.5; this task only edits SUMMARY + index).

- [x] 1.2

## docs/concepts pages

### Task 1.3: Author exchange-message.md, routes-pipelines.md, components-endpoints.md

**Files:**
- `docs/src/concepts/exchange-message.md` (new)
- `docs/src/concepts/routes-pipelines.md` (new)
- `docs/src/concepts/components-endpoints.md` (new)

**Steps:**
1. Author each page following the two-source rule: demonstrate current FORM via an `{{#include ../../../examples/hello-world/src/main.rs:first-route}}` include (the existing anchor), then cite the governing ADR for the WHY with a one-sentence paraphrase. Prose is connective narrative only; it must not define domain terms (glossary owns those) or restate ADR reasoning.
2. **`exchange-message.md`**: explain an Exchange carries input+output Message, headers, properties, and error state through a pipeline; show the `.set_header("source", Value::String("timer".into()))` line (from the include) as the form-claim of header manipulation. Cite ADR-0024 (PipelineOutcome) for why an exchange resolves to Completed/Stopped/Failed. Link the glossary for Message vs Exchange.
3. **`routes-pipelines.md`**: explain a Route is `from:` (source) + ordered `steps:`; show the `RouteBuilder::from("timer:tick?period=1000&repeatCount=5")` source and the `.to("log:info?showHeaders=true&showCorrelationId=true")` sink from the include. Cite ADR-0001 (Tower data plane) for why steps are `Service<Exchange>` and ADR-0024 (Stop is successful control flow, not an error).
4. **`components-endpoints.md`**: explain a Component owns a URI scheme and creates Endpoints/Consumers/Producers; show `ctx.register_component(TimerComponent::new())` + the `timer:tick?period=` URI from the include. Cite ADR-0015 for the PollingConsumer vs event-driven Consumer distinction.
5. Each page: ≤ ~1 screen, no inline code fence duplicating example content (every code block is the include), no `docs/ARCHITECT.md` reference, every ADR-NNNN reference resolves to an existing non-retired ADR.

**Tests:** (verification — shell-based; linters from task 1.1)
- `pages-use-include`: each of the 3 files contains `{{#include ../../../examples/hello-world/src/main.rs:first-route}}` → `rg -F -c '{{#include' <file>` ≥ 1 for each.
- `no-inline-dup-code`: no ```` ```rust ```` / ```` ```yaml ```` fence in the 3 files whose content duplicates the hello-world example (the only Rust shown is the include) → reviewer-verifiable; `! rg -n '^```rust$|^```yaml$' <file>` succeeds (the include uses ```` ```rust,ignore ````).
- `pages-cite-adrs`: each file cites at least one `ADR-00NN` → `rg -c 'ADR-00' <file>` ≥ 1 for each.
- `lint-adr-cite-clean`: `cargo xtask lint-adr-cite --deny docs/src/concepts/exchange-message.md docs/src/concepts/routes-pipelines.md docs/src/concepts/components-endpoints.md` reports 0 violations.
- `lint-slop-clean`: `cargo xtask lint-slop --deny docs/src/concepts/exchange-message.md docs/src/concepts/routes-pipelines.md docs/src/concepts/components-endpoints.md` reports 0 violations.

**Acceptance:**
- All 4 verification tests pass for each file.
- `cargo check -p hello-world` still passes (the include source is unchanged).

- [x] 1.3

### Task 1.4: Author planes.md and glossary.md

**Files:**
- `docs/src/concepts/planes.md` (new)
- `docs/src/concepts/glossary.md` (new)

**Steps:**
1. **`planes.md`**: demonstrate the data plane through the SAME `{{#include ../../../examples/hello-world/src/main.rs:first-route}}` include used by routes-pipelines.md (the route pipeline IS the data plane in action). Explain the two-plane split: data plane = Tower `Service<Exchange>` pipeline; control plane = trait hierarchy for route lifecycle/supervision/hot-reload. Cite ADR-0001 (the split) and ADR-0045 (bounded context). Do NOT assert the `Service<Exchange>` trait signature as a form-claim (no compiled example provides it; that deeper contract is Phase-3 `architecture/`). The page's form-claim is the pipeline shape, which the include satisfies.
2. **`glossary.md`**: two tiers. Bold ONLY the canonical terms at the start of a glossary entry line (so lint-glossary's line-anchored regex matches them and inline labels like `**Authority**:` do not).
   - Cross-cutting bold terms (each `**Term**` beginning its entry line), one-sentence user-facing definition + `Authority: CONTEXT-MAP.md#key-terms` and the defining ADR where applicable: **Message**, **ErrorHandler**, **CircuitBreaker**, **ExceptionDisposition**, **SecurityPolicy**, **Degraded** (write the short canonical `**Degraded**`; the definition may spell out both Degraded and Unhealthy states), **PollingConsumer**, **PipelineOutcome**.
   - Foundational primitives (NOT bold, so lint-glossary skips them), one-sentence definition + link to the owning CONTEXT.md: Route → `crates/camel-core/CONTEXT.md`; Exchange, Processor → `crates/camel-api/CONTEXT.md`; Component, Endpoint, Consumer, Producer → `crates/components/CONTEXT.md`; EIP → `crates/camel-processor/CONTEXT.md`. (Message is NOT listed here — it is a CONTEXT-MAP Key Term, so it gets ONE bold cross-cutting entry citing CONTEXT-MAP; do not duplicate it as a primitive.)
3. Both pages: ≤ ~1 screen each, every ADR-NNNN resolves to a non-retired ADR, no `docs/ARCHITECT.md` reference.

**Tests:** (verification — linters from task 1.1)
- `planes-uses-include`: `rg -F -c '{{#include' docs/src/concepts/planes.md` ≥ 1.
- `planes-no-trait-signature-claim`: `! rg -n 'Service<Exchange>' docs/src/concepts/planes.md` succeeds (the page does not assert the trait signature as form; if it mentions the phrase it must be as ADR-cited rationale, not a form-claim — reviewer-verifiable).
- `glossary-bold-terms-valid`: `cargo xtask lint-glossary --deny` reports 0 violations.
- `lint-adr-cite-clean`: `cargo xtask lint-adr-cite --deny docs/src/concepts/planes.md docs/src/concepts/glossary.md` reports 0 violations.
- `lint-slop-clean`: `cargo xtask lint-slop --deny docs/src/concepts/planes.md docs/src/concepts/glossary.md` reports 0 violations.

**Acceptance:**
- All verification tests pass.
- `cargo xtask lint-glossary --deny` exits 0.

- [x] 1.4

## docs prose additions

### Task 1.5: Add Camel-divergence note to introduction.md + two-source rule to contributing.md

**Files:**
- `docs/src/introduction.md` (modified)
- `docs/src/contributing.md` (modified)

**Steps:**
1. **`introduction.md`**: append one paragraph after the existing paragraph beginning "This guide is the home for concepts, tutorials, recipes, and operational guidance": state rust-camel is Apache-Camel-inspired, not a drop-in implementation; EIP vocabulary is the compatibility surface; link ADR-0046 for the consultation-protocol rationale. STE-flavored prose.
2. **`contributing.md`**: in the existing "Documentation workflow" section, add a subsection "Show the form, cite the decision" stating the two-source authoring rule verbatim: each concept page demonstrates current form via an anchored `{{#include}}` from a compiled example and cites the governing ADR for the why with a one-sentence paraphrase; the guide defines no domain terms (CONTEXT-MAP owns definitions) and restates no ADR reasoning. Place it before the existing paragraph beginning "Use named `ANCHOR` regions" so the rule frames the include mechanics.
3. Both edits: no `docs/ARCHITECT.md` reference, every ADR-NNNN resolves to a non-retired ADR.

**Tests:** (verification — linters from task 1.1)
- `intro-has-camel-note`: `rg -c 'ADR-0046' docs/src/introduction.md` ≥ 1.
- `contributing-has-two-source-rule`: `rg -c 'Show the form, cite the decision' docs/src/contributing.md` = 1.
- `lint-adr-cite-clean`: `cargo xtask lint-adr-cite --deny docs/src/introduction.md docs/src/contributing.md` reports 0 violations.
- `lint-slop-clean`: `cargo xtask lint-slop --deny docs/src/introduction.md docs/src/contributing.md` reports 0 violations.

**Acceptance:**
- All verification tests pass.

- [x] 1.5
