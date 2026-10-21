## ADDED Requirements

### Requirement: Bridge versions have one flat registry and no Gradle literal

The project SHALL store bridge versions in `bridges/versions.toml` as the single
editable source of truth. The file SHALL contain exactly the flat keys `cxf`,
`jms`, and `xml`, each assigned a quoted semantic version. It SHALL contain no
table headers, arrays, unknown keys, duplicate keys, or non-flat values.

Each `bridges/{cxf,jms,xml}/build.gradle.kts` SHALL resolve its default version
by reading `rootDir.resolve("../versions.toml")` with a regex for that bridge's
key, and SHALL contain no literal bridge version. It SHALL resolve the override
with `providers.gradleProperty("version").orNull ?: registryDefault`, not an
unspecified `Project.version`/`findProperty` value that can deaden the
fallback. An explicit `-Pversion=<value>` SHALL take precedence; with no
override, `Project.version` SHALL be the registry value. A missing file,
missing key, or unreadable value SHALL fail Gradle configuration with an
actionable diagnostic naming the resolved path and key.

#### Scenario: Registry default feeds a bridge build

- **GIVEN** `versions.toml` has `cxf = "0.8.4"`, `jms = "0.8.4"`, and
  `xml = "0.8.1"`, and no `-Pversion` is supplied
- **WHEN** `./gradlew -q properties` runs in each bridge directory
- **THEN** the reported `version:` is the matching registry value for cxf, jms,
  and xml, and `grep -rn '0\.8\.' bridges/*/build.gradle.kts` is empty

#### Scenario: Gradle property override keeps precedence

- **GIVEN** each bridge has its registry default
- **WHEN** `./gradlew -q properties -Pversion=9.9.9` runs in each bridge
  directory
- **THEN** the reported `version:` is `9.9.9`, not the registry value

#### Scenario: Non-flat registry shape is rejected

- **GIVEN** `bridges/versions.toml` contains a table header, an unknown key, a
  duplicate key, or an unquoted value
- **WHEN** the bridge-version lint parses the registry
- **THEN** it exits non-zero and names the file, line, and offending shape

### Requirement: Exact positive registry is compared against Rust constants

The bridge-version lint SHALL use an exact positive registry, not a directory
sweep:

- `cxf` → `crates/components/camel-cxf/src/lib.rs`
- `jms` → `crates/components/camel-jms/src/lib.rs`
- `xml` → `crates/components/camel-xslt/src/lib.rs` and
  `crates/components/camel-validator/src/lib.rs`

For every entry, the lint SHALL read the single
`pub const BRIDGE_VERSION: &str = "<value>";` declaration and compare its value
with the corresponding `versions.toml` entry. A missing file, missing or
duplicate declaration, or value mismatch SHALL fail the lint; a mismatch SHALL
report the path, line, registry key, expected value, and actual value.

#### Scenario: Clean registry passes

- **GIVEN** `versions.toml` and all four registered `BRIDGE_VERSION` constants
  hold their current values
- **WHEN** `cargo xtask lint-bridge-versions` runs
- **THEN** it exits 0

#### Scenario: Injected constant drift fails with exact evidence

- **GIVEN** a fixture workspace whose `camel-cxf/src/lib.rs` declares
  `BRIDGE_VERSION = "0.8.3"` while `versions.toml` says `cxf = "0.8.4"`
- **WHEN** the lint runs against that fixture
- **THEN** it exits non-zero and reports the CXF lib path, line, key `cxf`,
  expected `0.8.4`, and actual `0.8.3`

#### Scenario: Both XML component constants are checked

- **GIVEN** the XSLT constant matches but the Validator constant differs from
  the `xml` registry entry
- **WHEN** the lint runs
- **THEN** it exits non-zero naming the Validator lib path and the mismatch

### Requirement: No bridge version appears in the live doc set

The lint SHALL scan the live doc set: every `README.md` recursively under
`crates/`, every Markdown file under `docs/src/**`, text sources, configs, and
docs under `examples/**`, and `bridges/**/README.md`. It SHALL reject any
bridge-related semantic version in that set, including current pins, historical
references, and minimum-version prose. The lint SHALL NOT scan or fail on
`docs/adr/**`, `openspec/changes/archive/**`, `docs/archived/**`,
`docs/reviews/**`, or `CHANGELOG*`.

Detection SHALL be contextual, not a blanket semver ban. Token matching SHALL
use the `regex` crate without lookaround: match
`\d+\.\d+\.\d+(?:-[0-9A-Za-z]+(?:[.-][0-9A-Za-z]+)*)?`, then reject the match when the preceding
character is an ASCII digit or `.`, the following character is an ASCII digit,
or the following character is `.` and the next is an ASCII digit or identifier
character. A sentence-final period SHALL remain allowed: `since bridge 0.8.0.`
SHALL yield token `0.8.0`, while `127.0.0.1` and `0.0.0.0:9090` SHALL yield no
token.

Bridge subjects SHALL be matched case-insensitively: whole-word `bridge`
(excluding the compound crate `camel-bridge`), `cxf-bridge`, `jms-bridge`,
`xml-bridge`, `cxf-bridge-v`, `jms-bridge-v`, `xml-bridge-v`, `BRIDGE_VERSION`,
and `bridge_version`. Context words SHALL be matched case-insensitively.

Sentence scanning SHALL treat consecutive nonblank Markdown lines as one
paragraph and a table row as one sentence. A sentence ends at `[.!?;]` followed
by whitespace or paragraph end; a dot inside a version token SHALL never end a
sentence.

The token SHALL be bridge-related when any of these holds:

1. `version = "<token>"` is inside `[components.cxf]`, `[components.jms]`, or
   `[components.xml*]`.
2. A `bridge_version` field is assigned the token with `:` or `=`.
3. A bridge subject lies within 32 characters on either side of the token, or
   both a bridge subject and a version-context word occur in the token's
   sentence (`version`, `since`, `breaking`, `needs`, `requires`, `supports`,
   `or newer`, `or later`, `minimum`, `pinned`, `release pin`, `>=`).
4. The file is a bridge-topic doc and the sentence contains a version-context
   word. Bridge-topic docs are the `camel-{cxf,jms,xslt,validator,xj}` READMEs,
   `docs/src/components/{ws-soap,jms,xml-transform,validator}.md`,
   `docs/src/services/bridge.md`, and `bridges/**/README.md`.

The only product exception SHALL be a product name within 32 characters
**before** the token: `Quarkus`, `GraalVM`, `ActiveMQ`, `Artemis`, `MQTT`,
`OpenAPI`, `cargo`, `rustc`, `JDK`, `Saxon`, `Xerces`, or `camel-bridge`. The
exception SHALL NOT override rules 1 or 2, and a product after the token SHALL
NOT suppress a preceding bridge subject. Thus JMS ActiveMQ/Artemis table rows
SHALL pass, while `the bridge 0.8.0 acks ActiveMQ deliveries` SHALL fail.
Unrelated crate history, dependency versions, broker/protocol versions, wasm
guest `Cargo.toml` package versions, and IP addresses SHALL remain legal. Every
finding SHALL report path, line, token, and a remedy naming
`camel_<x>::BRIDGE_VERSION` or the release page.

#### Scenario: Component-block pin is rejected

- **GIVEN** `examples/cxf-example/Camel.toml`, the CXF README, or the ws-soap
  guide contains `[components.cxf]` followed by `version = "0.8.4"`
- **WHEN** the lint runs
- **THEN** it exits non-zero naming that file and line

#### Scenario: Bridge history and minimum prose are rejected

- **GIVEN** a live bridge doc says `since bridge 0.8.0`,
  `Breaking in 0.8.0`, or `needs 0.8.4 or newer`
- **WHEN** the lint runs
- **THEN** each is reported; historical and minimum bridge prose is not exempt

#### Scenario: Sentence-final version and IP boundaries are handled

- **GIVEN** a live doc contains `Since bridge 0.8.0.`, `127.0.0.1:8080`, and
  `0.0.0.0:9090`
- **WHEN** the lint runs
- **THEN** it reports `0.8.0` from the first line, and reports neither IP

#### Scenario: Consecutive nonblank lines form one paragraph

- **GIVEN** a bridge paragraph spans consecutive nonblank lines with
  `The bridge` on the first line and `requires 0.8.4.` on the next
- **WHEN** the lint scans the paragraph
- **THEN** `0.8.4` is reported because both the subject and context word occur
  in the token's sentence

#### Scenario: Sentence punctuation does not split a token

- **GIVEN** a live bridge paragraph contains `Since bridge 0.8.0.` and another
  sentence ends with `0.8.4;`
- **WHEN** the lint scans sentences
- **THEN** the sentence boundary is after the final period/semicolon, and
  tokens `0.8.0` and `0.8.4` are both reported

#### Scenario: Product exception is position-bound

- **GIVEN** JMS table rows list `| ActiveMQ Classic 5.18.3 | ... | bridge cap |`
  and `| ActiveMQ Artemis 2.36.0 | ... |`, and another live sentence says
  `the bridge 0.8.0 acks ActiveMQ deliveries`
- **WHEN** the lint runs
- **THEN** `5.18.3` and `2.36.0` pass, while `0.8.0` is reported because the
  product name follows the token

#### Scenario: Product name never overrides structured rules

- **GIVEN** a `version = "0.8.4"` line in a `[components.cxf]` block, or a
  `bridge_version: "1.0.0"` field, on a line where a product name precedes the
  token
- **WHEN** the lint runs
- **THEN** both remain violations

#### Scenario: Explicit bridge binary and tag terms are recognized

- **GIVEN** a live doc contains `cxf-bridge-v0.8.4` or `jms-bridge 0.8.0`
- **WHEN** the lint runs
- **THEN** both are reported

#### Scenario: Bridge subjects are case-insensitive whole words

- **GIVEN** fixtures contain `Since bridge 0.8.0` and `**Breaking in 0.8.0**`,
  and another line contains `camel-bridge v0.59.0`
- **WHEN** the lint runs
- **THEN** the first two are reported, and `camel-bridge` is not treated as a
  bridge subject

#### Scenario: Bridge version field is rejected

- **GIVEN** `crates/components/camel-xj/README.md` assigns
  `bridge_version: "1.0.0"`
- **WHEN** the lint runs
- **THEN** it exits non-zero naming the file, line, and token

#### Scenario: Bridge README numeric versions are rejected

- **GIVEN** `bridges/cxf/README.md` contains a pinned, release-pin, minimum, or
  historical bridge version such as `pinned 0.8.1` or `release pin is 0.8.1`
- **WHEN** the lint runs
- **THEN** both occurrences are reported

#### Scenario: JMS container comments are no longer exempt

- **GIVEN** `examples/jms-container-example/src/main.rs` contains
  `Since bridge 0.8.0` or `(bridge 0.8.0)`
- **WHEN** the lint runs
- **THEN** those comment lines are reported

#### Scenario: Bridge-topic history without the word bridge is rejected

- **GIVEN** `crates/components/camel-jms/README.md` contains
  `**Breaking in 0.8.0:**` in a bridge-topic component README
- **WHEN** the lint runs
- **THEN** the line is reported despite the absence of the word `bridge`

#### Scenario: Unrelated versions remain legal

- **GIVEN** the live set contains `since 0.16.0` in a non-bridge crate README,
  `minijinja/2.21.0`, `ActiveMQ 5.18.3`, `MQTT 3.1.1`, OpenAPI `3.0.3`, an
  examples `Cargo.toml` package/dependency version, a wasm guest `version =
  "0.8.1"`, and `127.0.0.1:8080`
- **WHEN** the lint runs
- **THEN** none is reported

#### Scenario: History directories stay exempt

- **GIVEN** `docs/adr/**`, `openspec/changes/archive/**`, `docs/archived/**`,
  `docs/reviews/**`, and `CHANGELOG*` contain old bridge versions
- **WHEN** the lint runs
- **THEN** none is reported

### Requirement: Live bridge prose states capabilities in the present tense

Live bridge documentation SHALL state what a bridge supports, not a version
number. It SHALL describe behavior in the present tense and SHALL point readers
to `camel_<x>::BRIDGE_VERSION` or the bridge release page when a current version
is needed. Migration and history notes SHALL belong in an ADR or generated
release notes, not a README.

The informative sentences in the four originally listed files SHALL be retained
as capability prose, not simply deleted:

- `examples/cxf-example/Camel.toml` and the CXF README SHALL drop only the
  literal `version =` pin from their configuration example.
- `docs/src/components/ws-soap.md` SHALL replace its release-pin/minimum
  paragraph with present-tense capability statements, preserving the
  `allow_expired_pinned_certificate`, one-way, and
  `CAMEL_CXF_BRIDGE_BINARY_PATH` guidance without versions.
- `bridges/cxf/README.md` SHALL state that `cargo xtask build-cxf-bridge`
  without `--version` produces a `dev`-labelled binary via `build-native.sh`,
  and SHALL point to `camel_cxf::BRIDGE_VERSION` or the release page for the
  current version. It SHALL NOT claim a registry-resolved build, and build
  behavior SHALL NOT change.

The JMS README, rabbitmq guide, and JMS container comments SHALL similarly
replace their bridge-version history with present-tense behavior. The XJ README
SHALL replace its `bridge_version` literal with
`camel_xslt::BRIDGE_VERSION.to_string()`.

#### Scenario: ws-soap capability rewrite keeps guidance

- **GIVEN** the ws-soap paragraph that previously listed pins and minimums
- **WHEN** it is rewritten
- **THEN** it names per-profile consumer dispatch,
  `allow_expired_pinned_certificate`, one-way operations, and the binary-path
  override as current capabilities, with no bridge version literal

#### Scenario: CXF bridge README capability rewrite

- **GIVEN** the native-acceptance paragraph that previously pinned `0.8.1`
- **WHEN** it is rewritten
- **THEN** it states that one-way behavior is accepted against a source-built
  bridge, gives `cargo xtask build-cxf-bridge` without a version argument,
  labels the result `dev`, and points to `camel_cxf::BRIDGE_VERSION` or the
  release page

#### Scenario: Informative JMS prose survives

- **GIVEN** `crates/components/camel-jms/README.md` and the rabbitmq guide
- **WHEN** their bridge-version history sentences are rewritten
- **THEN** the acknowledgement, redelivery, header-forwarding, and synchronous
  broker-acknowledgement guidance remains, stated in the present tense with no
  bridge version number

#### Scenario: XJ README uses the constant

- **GIVEN** the XJ README configuration example
- **WHEN** its `bridge_version` value is rewritten
- **THEN** it uses `camel_xslt::BRIDGE_VERSION.to_string()` and contains no
  semantic version literal

### Requirement: Tag mode fails with an actionable exact diagnostic

The lint SHALL accept `--tag <name>` and SHALL derive the bridge from the
`<bridge>-bridge-v` prefix. It SHALL require the stripped tag value to equal the
corresponding `versions.toml` entry. An unknown tag prefix SHALL fail with the
accepted forms. A mismatch SHALL print exactly:

```
tag xml-bridge-v0.8.2 != bridges/versions.toml xml=0.8.1 — run 'cargo xtask bump-bridge xml 0.8.2', commit, delete and re-push the tag
```

Tag mode SHALL also run the registry and live-set checks; a matching tag
SHALL not bypass a drifted constant or a live bridge-version finding.

#### Scenario: Matching tag passes

- **GIVEN** `versions.toml` has `cxf = "0.8.4"`
- **WHEN** the lint runs with `--tag cxf-bridge-v0.8.4`
- **THEN** it exits 0

#### Scenario: Mismatched tag fails actionably

- **GIVEN** `versions.toml` has `xml = "0.8.1"`
- **WHEN** the lint runs with `--tag xml-bridge-v0.8.2`
- **THEN** it exits non-zero and prints the exact diagnostic above naming the
  bump command, commit, tag deletion, and re-push

#### Scenario: Unknown tag prefix is rejected

- **GIVEN** a tag name such as `foo-bridge-v1.2.3`
- **WHEN** the lint runs in tag mode
- **THEN** it exits non-zero and lists the accepted
  `cxf-bridge-v*`, `jms-bridge-v*`, and `xml-bridge-v*` forms

### Requirement: bump-bridge is atomic and lint-verified

`cargo xtask bump-bridge <bridge> <version>` SHALL validate that `<bridge>` is
one of `cxf`, `jms`, or `xml` and that `<version>` is a semantic version. It
SHALL rewrite the registry entry and every positive-registry constant for that
bridge (`xml` updates both the XSLT and Validator constants), then run the
bridge-version lint in process.

The command SHALL behave atomically: it SHALL snapshot every target file before
writing, and if any write or the post-write lint fails, it SHALL restore every
snapshot byte-for-byte and exit non-zero. The result SHALL be either all pins
updated or none. A restore failure SHALL report both the original failure and
the rollback failure. Success SHALL print the changed files.

#### Scenario: Successful bump changes only the requested bridge

- **GIVEN** a fixture workspace with all registered files matching the current
  registry
- **WHEN** `bump-bridge xml 0.8.2` runs
- **THEN** `versions.toml`, `camel-xslt/src/lib.rs`, and
  `camel-validator/src/lib.rs` contain `0.8.2`, the CXF and JMS values are
  unchanged, and the lint passes

#### Scenario: Post-write lint failure rolls back every file

- **GIVEN** a temp workspace fixture and a test-only hook that makes the
  post-write lint fail after all files are written
- **WHEN** `bump-bridge cxf 9.9.9` runs
- **THEN** it exits non-zero, and every target file is byte-identical to its
  pre-command snapshot

#### Scenario: Invalid input performs no writes

- **GIVEN** an unknown bridge name or a non-semver version
- **WHEN** `bump-bridge` runs
- **THEN** it exits non-zero before writing any file

### Requirement: Release workflows guard tags and gates register the lint

Each of `.github/workflows/{cxf,jms,xml}-bridge-release.yml` SHALL define a
`guard` job that checks out the repository, installs Rust, and runs
`cargo xtask lint-bridge-versions --tag "$GITHUB_REF_NAME"`. The `build` job
SHALL declare `needs: guard`, so a mismatched tag blocks every platform build.

The lint SHALL be registered as a `cargo xtask` subcommand, in the CI quality
gates, and in the `AGENTS.md` gate list. The release workflow files SHALL parse
as valid YAML with the `needs: guard` edge present.

#### Scenario: Mismatched release tag blocks the build

- **GIVEN** a pushed `jms-bridge-v9.9.9` tag while `versions.toml` has
  `jms = "0.8.4"`
- **WHEN** the JMS bridge release workflow starts
- **THEN** `guard` fails with the actionable tag diagnostic and `build` does
  not run

#### Scenario: Workflow YAML and gate registration are verifiable

- **GIVEN** the three edited workflow files
- **WHEN** they are parsed with a YAML parser and inspected
- **THEN** each parses successfully, declares `guard`, and its `build` job has
  `needs: guard`; `ci.yml` and `AGENTS.md` list the new lint

### Requirement: No bridge behavior or version changes

The change SHALL NOT alter bridge build outputs, bridge selection, runtime
component behavior, or engine behavior. It SHALL NOT bump any bridge version,
edit any ADR or archived OpenSpec change, or create, delete, or push a tag. The
only Gradle behavior change SHALL be the source of the default version.

#### Scenario: Diff scope is registry tooling and bridge-doc literals only

- **GIVEN** the completed change
- **WHEN** its diff is inspected
- **THEN** no bridge version value in `versions.toml` or a registered constant
  differs from the pre-change value, and no file under `docs/adr/**` or
  `openspec/changes/archive/**` is modified

#### Scenario: Bridge property build remains unchanged

- **GIVEN** the edited Gradle configuration
- **WHEN** a bridge build runs with an explicit `-Pversion=`
- **THEN** the supplied property is the project version and no other build
  behavior changes
