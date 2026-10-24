# xml-bridge-charset-support Specification

## Purpose
TBD - created by archiving change xml-native-charset. Update Purpose after archive.
## Requirements
### Requirement: Native image charset registry opt-in

The xml bridge native image SHALL register all standard JDK charsets (and
their aliases) by declaring `QUARKUS_NATIVE_ADD_ALL_CHARSETS="true"` in
`bridges/xml/native-build-args.env` (an `export` line, so the shell
consumer picks it up directly), which maps to the Quarkus build-time
property `quarkus.native.add-all-charsets` and makes Quarkus emit
`-H:+AddAllCharsets` itself, so that documents whose XML declaration
names any Xerces-mapped IANA encoding (Xerces' `EncodingMap` sits in front of
the JDK registry on the input side) parse correctly and stylesheets whose
`xsl:output @encoding` names any standard JDK charset or alias serialize
correctly.

Reason for mechanism (observed 2026-10-24): Quarkus 3.39.1's
`NativeImageBuildStep` unconditionally appends `-H:-AddAllCharsets` AFTER
the `additional-build-args` list, so a `-H:+AddAllCharsets` token placed
there is always cancelled (build #2: +0 bytes, probe unchanged). The
Quarkus build-time property replaces the cancelling flag instead of
fighting it (verified in quarkus-core-deployment-3.39.1 bytecode).

#### Scenario: both native build consumers propagate the charset opt-in

- **GIVEN** `native-build-args.env` declares
  `export QUARKUS_NATIVE_ADD_ALL_CHARSETS="true"`
- **WHEN** the linux/docker build runs (`build-native.sh` sources the
  whole file) AND the windows/macos build runs (xtask
  `build_bridge_native` parses the file)
- **THEN** both gradle invocations see `QUARKUS_NATIVE_ADD_ALL_CHARSETS=true`,
  and the xtask path fails closed on a MALFORMED declaration (non-boolean
  value, or the unexported assignment form that a shell would keep as a
  local and never hand to gradle); an ABSENT declaration is valid for the
  other bridges (jms/cxf share the parser) and the xml bridge's own
  declaration is pinned by a real-file unit test asserting `Some("true")`

#### Scenario: ISO-8859-15 declaration with ASCII-only content parses in native

- **GIVEN** the xml bridge native runner built with
  `QUARKUS_NATIVE_ADD_ALL_CHARSETS=true` declared in
  `native-build-args.env`
- **WHEN** a transform request carries a document whose declaration is
  `<?xml version="1.0" encoding="ISO-8859-15"?>` with pure-ASCII content
- **THEN** the transform succeeds AND the output bytes equal the exact
  expected serialized document (non-empty, byte-compared)

#### Scenario: ISO-8859-15 input side decoded as Latin-9 not Latin-1

- **GIVEN** the xml bridge native runner built with
  `QUARKUS_NATIVE_ADD_ALL_CHARSETS=true` declared in
  `native-build-args.env`
- **WHEN** a document declared `encoding="ISO-8859-15"` contains byte `0xA4`
  (euro, U+20AC, in Latin-9) and the stylesheet output encoding is UTF-8
- **THEN** the output bytes contain the euro as the UTF-8 sequence
  `E2 82 AC` (proving the parser decoded U+20AC, not Latin-1 `¤` / `C2 A4`)

#### Scenario: ISO-8859-15 output side serialized byte-exact

- **GIVEN** the xml bridge native runner built with
  `QUARKUS_NATIVE_ADD_ALL_CHARSETS=true` declared in
  `native-build-args.env`
- **WHEN** a UTF-8 document containing the euro (`E2 82 AC`) is transformed by
  a stylesheet whose `xsl:output` declares `encoding="ISO-8859-15"`
- **THEN** the output bytes contain `encoding="ISO-8859-15"` AND the euro as
  the single byte `0xA4` (never the UTF-8 sequence `E2 82 AC`, never the
  entity form `&#8364;`)

#### Scenario: ISO-8859-15 round trip in and out

- **GIVEN** the xml bridge native runner built with
  `QUARKUS_NATIVE_ADD_ALL_CHARSETS=true` declared in
  `native-build-args.env`
- **WHEN** a document declared `encoding="ISO-8859-15"` containing byte
  `0xA4` is transformed by a stylesheet whose `xsl:output` also declares
  `encoding="ISO-8859-15"`
- **THEN** the output bytes contain the declaration
  `encoding="ISO-8859-15"` AND the euro as the single byte `0xA4`

#### Scenario: controls stay green

- **GIVEN** the xml bridge native runner built with
  `QUARKUS_NATIVE_ADD_ALL_CHARSETS=true` declared in
  `native-build-args.env`
- **WHEN** equivalent requests are sent with ISO-8859-1 and UTF-8 documents
  (declaration-side and output-side variants; ISO-8859-1 `0xA4` is `¤` U+00A4
  and must round trip as `C2 A4` in UTF-8 output, never `0xA4`-as-euro)
- **THEN** all control transforms succeed with byte-exact expected output

#### Scenario: charset alias resolution

- **GIVEN** the xml bridge native runner built with
  `QUARKUS_NATIVE_ADD_ALL_CHARSETS=true` declared in
  `native-build-args.env`
- **WHEN** a document declared with the Xerces-mapped alias `LATIN-9`
  contains byte `0xA4` (euro, U+20AC) and the stylesheet output encoding is
  UTF-8
- **THEN** the transform succeeds AND the output bytes contain the euro as
  the UTF-8 sequence `E2 82 AC` (a Latin-1 `¤` misdecode would produce
  `C2 A4` and must not pass)

#### Scenario: native-only evidence

- **GIVEN** the probe cases are gated by
  `require_worktree_native_bridge()` in the camel-test harness
- **WHEN** only a JVM bridge or a stale/downloaded binary is available
- **THEN** the probe fails loudly (panic from the require gate); it is never
  silently skipped and never substituted with a JVM run

### Requirement: Native fix evidence

The mission SHALL produce committed evidence from the SAME probe test names
run against two native builds: the unfixed build (current args, before the
charset entry) and the fixed build (`QUARKUS_NATIVE_ADD_ALL_CHARSETS=true`), plus measured
build cost deltas.

#### Scenario: before-build reproduces the reported failure

- **GIVEN** the native runner built with `native-build-args.env` NOT containing
  any charset entry
- **WHEN** the probe suite runs with `--features integration-tests` against
  that runner
- **THEN** every pure input-side ISO-8859-15 case (ASCII, euro-in,
  `LATIN-9` alias) fails with the verbatim reported failure text
  (`I/O error reported by XML parser processing null`), the output-only
  case AND the combined in-and-out round-trip case fail (error or byte
  assertion) with their
  verbatim output recorded as-is (observed on build #1: the
  serializer's encoder check precedes input parsing, so these surface
  `Unknown encoding requested: ISO-8859-15`; parser text is NOT asserted
  for them), the ISO-8859-1 and
  UTF-8 controls AND the unknown-charset cases pass, and the evidence file
  records the failing test names, the exact assertion output, the
  `--features integration-tests` command line, and a non-zero pass count for
  the controls. Evidence lives in
  `openspec/changes/xml-native-charset/evidence.md`, section `before`
  [Correction 2026-10-24, phase-1 review: round-trip moved from the
  parser-text group to the recorded-as-is group after build #1 observed
  the serializer encoder check firing first; matches tasks.md
  29d0ee357.]

#### Scenario: after-build proves the fix

- **GIVEN** the native runner built with
  `QUARKUS_NATIVE_ADD_ALL_CHARSETS=true` declared in
  `native-build-args.env`
- **WHEN** the SAME probe test names run against that runner
- **THEN** all cases including the Latin-9 euro round trip pass, and
  `openspec/changes/xml-native-charset/evidence.md` (section `after`) records
  the test names and pass counts

#### Scenario: build cost deltas recorded

- **GIVEN** both native builds completed in the mission worktree
- **WHEN** the runner binary size (`stat -c%s`) and startup time (median wall
  time to the ready line over 10 runs) are compared before vs after
- **THEN** the deltas are recorded in
  `openspec/changes/xml-native-charset/evidence.md` (section `deltas`); a
  delta exceeding +5 MB binary size or +50 ms median ready time is recorded
  as an owner escalation (report and stop), not a silent pass

### Requirement: Charset posture documentation

The unknown-charset scenarios below are expected to return `TRANSFORM_ERROR:`
in BOTH builds (with and without the charset opt-in) — they count as
controls, not as ISO-8859-15 failure cases.

The xml bridge documentation SHALL state the supported-encodings posture as
observed by the probe: which charsets are guaranteed in the native image (the
standard JDK charsets registered via the `QUARKUS_NATIVE_ADD_ALL_CHARSETS` opt-in in
`native-build-args.env`), that Xerces-mapped aliases such as `LATIN-9` and
`ISO_8859-15` resolve on the input side (JDK aliases resolve on the output
side), and the
observed failure behavior when an `encoding` names a charset the JDK registry
does not know.

#### Scenario: posture documented in bridge docs

- **GIVEN** the xml bridge docs (`docs/src/services/bridge.md`,
  `docs/src/components/xml-transform.md`)
- **WHEN** a reader looks up supported encodings for native deployments
- **THEN** the docs name the all-charsets opt-in posture
  (`QUARKUS_NATIVE_ADD_ALL_CHARSETS=true` in `native-build-args.env`, which
  makes Quarkus emit `-H:+AddAllCharsets`), the alias resolution
  (`LATIN-9`, `ISO_8859-15`), and the observed `TRANSFORM_ERROR:` failure
  behavior for unknown charset names

#### Scenario: unknown input charset fails with a transform error

- **GIVEN** the xml bridge native runner built with `native-build-args.env`
  (with or without the charset opt-in)
- **WHEN** a document declares an encoding name absent from the JDK registry
  (e.g. `encoding="X-BOGUS-CHARSET"`)
- **THEN** the response carries a `BridgeError` whose message starts with
  `TRANSFORM_ERROR:`

#### Scenario: unknown output charset fails with a transform error

- **GIVEN** the xml bridge native runner built with `native-build-args.env`
  (with or without the charset opt-in)
- **WHEN** a stylesheet's `xsl:output` declares an encoding name absent from
  the JDK registry
- **THEN** the response carries a `BridgeError` whose message starts with
  `TRANSFORM_ERROR:` (Saxon silently falling back to UTF-8 instead would be
  recorded as a separate finding, never counted as a pass)

### Requirement: Native build args single source of truth

The xml bridge native-image build args SHALL live only in
`bridges/xml/native-build-args.env`; neither `application.yml` nor
`build.gradle.kts` SHALL carry charset-related native-image args.

#### Scenario: no second source of truth introduced

- **GIVEN** the change adds the charset opt-in
- **WHEN** the xml bridge sources (`bridges/xml/**`, excluding
  `native-build-args.env`) are inspected for YAML/Gradle property KEYS
  `quarkus.native.add-all-charsets` or `quarkus.native.additional-build-args`
  (comment lines excluded), and for the strings `AddAllCharsets` and
  `ADD_ALL_CHARSETS` (the env-var spelling — case-sensitive grep, so a
  duplicate declaration elsewhere cannot hide behind the spelling change)
- **THEN** no property-key occurrence exists, AND neither `AddAllCharsets`
  nor `ADD_ALL_CHARSETS` appears in any NON-COMMENT line under
  `bridges/xml/**` except `native-build-args.env` (comment lines are
  excluded from the string check too — they are not declarations; docs
  under `docs/src/**` and this change dir are out of scope entirely)

