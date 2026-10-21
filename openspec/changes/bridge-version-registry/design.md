# Design: bridge-version-registry

## Approach

**1. Single source.** `bridges/versions.toml` has flat `cxf`, `jms`, `xml`
entries. The parser allows blank/`#` lines; every other line matches
`^(cxf|jms|xml)\s*=\s*"<semver>"$`; duplicates, unknown keys, non-flat shapes,
missing keys, and trailing content fail.

**2. Gradle default.** Each `build.gradle.kts` reads
`rootDir.resolve("../versions.toml")`, extracts its key, and fails with path/key
when absent. Resolve the override with
`providers.gradleProperty("version").orNull ?: registryDefault`; do not key off
an unspecified `Project.version`/`findProperty`, which can deaden the fallback.
No literal remains.

**3. Positive registry.** Exact cxf/jms/xml mapping to the four component
files. One `pub const BRIDGE_VERSION: &str = "<ver>";` each; missing
file/constant or mismatch reports path, line, key, expected, actual.

**4. Detection.** Live set: `README.md` recursively under `crates/`, Markdown
under `docs/src/**`, text files under `examples/**`,
`bridges/**/README.md`. Exclude history dirs/CHANGELOG,
`target/`, `Cargo.lock`, and wasm guest `Cargo.toml`.

Regex matching is lookaround-free:
`\d+\.\d+\.\d+(?:-[0-9A-Za-z]+(?:[.-][0-9A-Za-z]+)*)?`. Reject a match when the previous char is a
digit or `.`, the next char is a digit, or the next char is `.` and the one after
is a digit or identifier char. Sentence-final period allowed:
`since bridge 0.8.0.` catches `0.8.0`; `127.0.0.1` and `0.0.0.0:9090` yield
none.

Subjects (case-insensitive): whole-word `bridge` excluding `camel-bridge`,
`cxf-bridge`, `jms-bridge`, `xml-bridge`, `<b>-bridge-v`, `BRIDGE_VERSION`,
`bridge_version`. Context words: case-insensitive. A token is
bridge-related when:

1. `version =` is inside `[components.cxf]`, `[components.jms]`, or
   `[components.xml*]`.
2. `bridge_version` is assigned the token.
3. A subject lies within 32 characters on either side of the token, or the
   token's sentence contains both a subject and a version-context word
   (`version`, `since`, `breaking`, `needs`, `requires`, `supports`, `or newer`,
   `or later`, `minimum`, `pinned`, `release pin`, `>=`).
4. A bridge-topic doc (bridge component READMEs, bridge guides,
   `docs/src/services/bridge.md`, `bridges/**/README.md`) has a context word in
   the sentence.

Scan Markdown paragraphs as consecutive nonblank lines; a table row is one
sentence. A sentence ends at `[.!?;]` followed by whitespace or paragraph end; a
dot inside a token never ends it. The product exception
is a name within 32 characters **before** the token
(`Quarkus`, `GraalVM`, `ActiveMQ`, `Artemis`, `MQTT`, `OpenAPI`, `cargo`,
`rustc`, `JDK`, `Saxon`, `Xerces`, `camel-bridge`). It never overrides rules 1/2; a product after the token never suppresses a preceding subject. Thus
JMS ActiveMQ/Artemis table rows pass, while
`the bridge 0.8.0 acks ActiveMQ deliveries` fails. Crate history, dependencies, broker/protocol, wasm package versions, and IPs stay legal.
Findings report path, line, token, and a remedy naming
`camel_<x>::BRIDGE_VERSION` or the release page.

**5. Tag mode.** `--tag` requires `cxf-bridge-v<b>`, `jms-bridge-v<b>`, or
`xml-bridge-v<b>`. Unknown prefixes list accepted forms; a mismatch prints:

```
tag xml-bridge-v0.8.2 != bridges/versions.toml xml=0.8.1 — run 'cargo xtask bump-bridge xml 0.8.2', commit, delete and re-push the tag
```

Tag mode also checks registry/live set.

**6. Atomic bump.** `bump-bridge` validates inputs, snapshots targets, writes the
registry and that bridge's constants (both for `xml`) through temp-file + rename,
then runs the lint in-process. Any failure restores every snapshot and exits
non-zero with the lint error and rollback notice; a rollback failure reports
both. Success lists changes; no other bridge changes.

**7. Rewrites.** Remove `version =` from the CXF example, CXF README block, and
ws-soap block. Rewrite ws-soap pins/minimums as present-tense capabilities
naming `camel_cxf::BRIDGE_VERSION` or the release page. In the CXF bridge
README, state that `cargo xtask build-cxf-bridge` without `--version` produces
a `dev`-labelled binary via `build-native.sh`; point to `BRIDGE_VERSION`/release
page. Do not claim registry resolution or change build behavior. Rewrite
JMS README, rabbitmq, and JMS-container history as present-tense behavior.
Replace the XJ literal with `camel_xslt::BRIDGE_VERSION.to_string()`. Delete the
JMS-only test; register the xtask command, CI gate, and `AGENTS.md`; add `guard` to every release workflow with `build: needs: guard`.

## Affected crates

- `xtask`: registry parser, live-set lint, tag mode, atomic bump.
- Bridge components: lint-managed constants; JMS test deleted; no behavior change.
- `bridges/*`, `examples/*`, `docs/src`, CI/AGENTS: prose, defaults, gates.

## Architecture boundaries

Release/build tooling only. No public API or runtime boundary crossing. Applies
ADR-0041's single-source principle to build metadata and the xtask gate
convention.

## Verification (proof, not assertion)

- Fixtures prove each rule and negative near-misses: JMS ActiveMQ/Artemis
  table rows, `since 0.16.0`, examples/wasm `Cargo.toml` versions, and IPs.
- A `#[cfg(test)]` post-write failure hook proves bump restores every fixture
  byte; success proves only requested targets change.
- Lint and `--tag cxf-bridge-v0.8.4` exit 0; `--tag cxf-bridge-v9.9.9` exits
  non-zero with the exact message; the live set has zero findings.
- `grep -rn '0\.8\.' bridges/*/build.gradle.kts` is empty; per bridge,
  `./gradlew -q properties` reports the registry version and
  `-Pversion=9.9.9` reports `9.9.9`.
- YAML parsing asserts `guard` and `build.needs: guard` in each workflow.
- Gates: fmt, xtask clippy with `-D warnings`, xtask tests, this repo's lint,
  `schema --check`, and `mdbook test`.

## Alternatives considered

- Keep Gradle literals/extend the JMS test: multiple sources.
- Generate Gradle files or require `-Pversion=`: extra surface or broken
  offline builds.
- One version for all bridges: XML trails.
- Tag-only guard: misses prose/constant drift.
- Ban every semver: rejects unrelated dependencies, brokers, protocols, and
  packages; contextual detection is required.
