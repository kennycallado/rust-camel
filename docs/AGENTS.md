# Documentation Authoring Rules

## CRITICAL

- **Load the `ste-writing` skill** before editing any `.md` file in this directory. The skill defines ASD-STE100 Simplified Technical English rules. Apply them to all prose. Do NOT rely on mechanical linters as a substitute for the skill.
- **Two-source rule**: every durable claim must cite `crates/<crate>/CONTEXT.md` (crate authority) or an ADR in `docs/adr/`. Uncited claims are speculation.
- **Include-driven pages**: use `{{#include ../../../examples/<dir>/<file>:<anchor>}}` to pull code from compiled examples. Do NOT write inline code snippets for patterns that have runnable examples. Anchor the example source with `// ANCHOR: <id>` / `// ANCHOR_END: <id>` (Rust) or `# ANCHOR: <id>` / `# ANCHOR_END: <id>` (YAML).
- **Never add content inside fenced code blocks that changes the included code**. The include directive replaces the block at build time.

## Structural linters (deterministic, run these)

- `cargo run -p xtask -- lint-adr-cite --deny docs/src/` — verifies every ADR citation points to a file that exists in `docs/adr/`.
- `cargo run -p xtask -- lint-glossary` — verifies the glossary is consistent with CONTEXT-MAP Key Terms.

These are link/structure validators. They are deterministic and valuable. Run them before committing.

## Prose quality

There is no mechanical prose linter. Prose quality depends on:
1. The `ste-writing` skill (primary)
2. Expert review (ste-writing or code-review skill via a capable agent)
3. Human judgment

Do NOT add a regex-based prose linter. Regex cannot assess sentence complexity, voice consistency, information density, or verbosity. A denylist of banned words gives false confidence.

## Page template

Every EIP pattern page follows this structure:
1. `# <EIP name>` heading
2. One sentence naming the EIP and its Hohpe/Woolf category
3. `{{#include}}` directive pulling the route code
4. 2-4 paragraphs of prose (STE-compliant)
5. Link to `../../../crates/camel-processor/CONTEXT.md` for contract details
6. ADR citation where the page states architectural rationale
7. Example source link to GitHub

Section hub pages (index.md) group pages by family with one-line descriptions and relative links.

## SUMMARY wiring

`docs/src/SUMMARY.md` is the mdBook table of contents. Structure rules:

- Each major section has one parent page (e.g. `[EIP patterns](eip/index.md)`) with entries indented underneath.
- When a section exceeds ~10 sibling entries, split them into family sub-groups with their own hub page (e.g. `[Routing](eip/routing.md)` indented under the section parent, with pattern pages indented further). Family hub names MUST match the headings in the section's `index.md`.
- Sections under the threshold stay flat under their parent.
- Never add a fourth nesting level. If a family has more than ~12 entries, the grouping is wrong — revisit the taxonomy.

## Build verification

Before committing documentation changes:
```bash
nix shell nixpkgs#mdbook -c mdbook build docs    # must exit 0
cargo run -p xtask -- lint-adr-cite --deny docs/src/   # must be clean
```
