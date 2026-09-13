# Proposal: r0nongoals

## Why

The compiled-artifact boundary must remain explicit as the compile roadmap expands. Without a permanent non-goals statement, later changes could reintroduce development-loop features or ambient configuration into sealed artifacts. This change records the boundary from bd `rc-ws8sd` and the compile-roadmap ruling.

## What Changes

- Add a `Permanent v1 non-goals` requirement to the `cli-compile` delta specification.
- Enumerate the five permanent exclusions: watch/hot-reload, runtime file discovery/globbing, ambient `Camel.toml`, a wide artifact argument surface beyond `--report`, `--help`, `--version`, and `--manifest`, and compile-time `CAMEL_*` overrides. Preserve R4 signature verification as the single sanctioned future surface extension.
- Add a `Compiled artifact` scope-boundary note to `CONTEXT-MAP.md`.

No Rust code, CLI behavior, ADR, or canonical spec is edited directly. OpenSpec archive will canonicalize the delta after merge.

## Acceptance criteria

- `openspec validate r0nongoals --type change --json` passes with no delta-structure errors.
- The delta contains one requirement with scenarios that name all five permanent non-goals and preserve the existing argument and verification boundary.
- `CONTEXT-MAP.md` identifies the same non-goals and cites ADR-0075.
- Context citation lint and the required documentation build pass.

## Risk budget

Risk is limited to specification and context prose drift. No implementation behavior or public API changes are in scope. The wording must not contradict existing `cli-compile` requirements for unsupported assets or artifact arguments.
