# Proposal: bridge-version-registry

## Why

Bridge versions live in three Gradle fallbacks, four Rust `BRIDGE_VERSION`
constants, and live Camel.toml/documentation examples. Every release repeats the
manual edit; one missed pin creates a crate/tag skew that appears only during a
release build or when a component requests an unpublished bridge. The JMS-only
drift test compares Rust to a Gradle literal and preserves the duplication.

Bd: rc-0fdrp (supersedes rc-hujeg).

## What Changes

- Add `bridges/versions.toml` as the single source of truth, with exactly three
  flat entries: `cxf = "0.8.4"`, `jms = "0.8.4"`, `xml = "0.8.1"`.
- Make each `build.gradle.kts` read its default from `../versions.toml` with a
  regex; no literal bridge version remains in Gradle. `-Pversion=` keeps
  precedence.
- Add `cargo xtask lint-bridge-versions`: validate the registry shape, compare
  the exact positive registry (CXF, JMS, XSLT, Validator), reject any
  bridge-related semantic version in the live doc set, and support `--tag` with
  an actionable diagnostic. The live set is `README.md` files recursively under
  `crates/`, all Markdown under `docs/src/**`, text sources/configs/docs under
  `examples/**`, and `bridges/**/README.md`. Detection is contextual, so
  unrelated crate, dependency, package, broker, protocol, and IP-address
  versions stay legal; wasm guest `Cargo.toml` versions are exempt.
- Add `cargo xtask bump-bridge <bridge> <version>`: rewrite the registry and
  registered constants, run the lint, and restore every original byte on
  post-write failure.
- Rewrite live pins as capability statements in the present tense, pointing at
  `camel_<x>::BRIDGE_VERSION` or the release page. Keep the informative
  sentences in the CXF example, CXF README, ws-soap guide, and CXF bridge
  README; remove only literal pins. Rewrite the JMS README, rabbitmq guide, and
  JMS container comments likewise, and replace the XJ `bridge_version` literal
  with `camel_xslt::BRIDGE_VERSION.to_string()`; delete the JMS-only drift test.
- Wire the lint into xtask, CI gates, `AGENTS.md`, and a `guard` job in each
  bridge release workflow, with `build` depending on it.

Excluded: any bridge version bump, bridge/engine behavior change, tag, or push.
History directories remain exempt (`docs/adr/**`,
`openspec/changes/archive/**`, `docs/archived/**`, `docs/reviews/**`,
`CHANGELOG*`); live historical and minimum bridge prose is NOT exempt.

## Acceptance criteria

- `grep -rn '0\.8\.' bridges/*/build.gradle.kts` returns nothing.
- `cargo xtask lint-bridge-versions` exits 0; the live doc set contains no
  bridge version literal.
- `--tag cxf-bridge-v0.8.4` exits 0; `--tag cxf-bridge-v9.9.9` exits non-zero
  with the actionable bump/commit/re-push message.
- Fixture tests prove const drift detection with file, line, expected, and
  actual values, prove bump rollback restores all bytes, and prove unrelated
  semver near-misses stay legal.
- Per bridge, `./gradlew -q properties` reports its registry default; with
  `-Pversion=9.9.9` it reports `9.9.9`.
- The four originally listed files retain their informative content as
  configuration examples or capability prose, and `bridges/**/README.md` retain
  no bridge numeric current, minimum, or history version.
- The release workflows parse, and their `build` jobs declare `needs: guard`.

## Risk budget

Acceptable: a strict registry parser and contextual bridge-version detection
that fixtures prove catches pins, histories, minimums, `bridge_version` fields,
and component-block `version =` lines. Out of bounds: banning unrelated
semvers (crate history, dependencies, brokers, protocols, wasm guest package
versions), changing runtime bridge selection, bumping a version, rewriting
ADRs, or tagging/pushing.
