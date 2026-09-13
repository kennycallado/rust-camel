# Design: r0nongoals

## Approach

Add one declarative requirement to the `cli-compile` delta. The requirement records permanent v1 non-goals and cross-references the existing requirements that enforce the artifact argument and compile-time asset boundaries. The argument boundary preserves R4 signature verification as the single sanctioned future surface extension. Scenarios provide concrete operator-visible examples so the OpenSpec parser and future reviewers can validate the boundary.

Add one key-term entry to `CONTEXT-MAP.md` immediately after `Compiled artifact`. It states the sealed-unit scope and cites ADR-0075, while avoiding a duplicate `Compiled artifact` definition.

## Affected crates

- `camel-cli` specification: documents the compile command's permanent v1 scope; no source crate changes.
- `camel-dsl` specification context: referenced only for document discovery semantics; no source changes.

## Architecture boundaries

This is documentation-only. It preserves the existing control-plane contract: compilation embeds a self-contained document and manifest, while deployment resolves permitted runtime environment values. It does not add runtime discovery, mutate route execution, or change component, service, language, or function boundaries. ADR-0075 remains the authority for the compiled artifact format and trust boundary.

## Alternatives considered

- Modify each existing compile requirement: rejected because it duplicates enforcement prose and increases drift risk.
- Add a duplicate `Compiled artifact` key term: rejected because `CONTEXT-MAP.md` already defines that term; the missing information is the permanent scope boundary.
- Edit `openspec/specs/cli-compile/spec.md` directly: rejected because the change must be archived through OpenSpec.
