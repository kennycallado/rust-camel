# Design: xml-native-charset

## Context

The xml bridge is a Quarkus gRPC service (Saxon-HE 12.7, Xerces 2.12.2). The
transform path feeds raw document bytes to a Xerces `SAXSource` (the parser
honors the XML declaration's `encoding`) and serializes via Saxon
`StreamResult` (honors `xsl:output @encoding`). In a GraalVM native image only
the standard charsets are registered unless the build opts in. Xerces has
built-in readers for UTF-8, ASCII, Latin-1 and UTF-16 — which is why
ISO-8859-1 and UTF-8 work while ISO-8859-15 fails. For any other declared
encoding Xerces asks the JDK charset registry; in the image that raises
`UnsupportedEncodingException` (an `IOException`), which Saxon wraps as
`I/O error reported by XML parser processing ` + systemId, and systemId is
null for a byte source. This matches camel-112 exactly, including why
ASCII-only content fails: the declaration alone triggers charset resolution.

## Pre-flight ruling (e_opus, GO-WITH-CAVEATS)

1. Probe via the existing Rust harness — `camel-test` xml bridge XSLT
   integration tests, gated by
   `support::xml_bridge::require_worktree_native_bridge()` and driven with
   `CAMEL_XML_BRIDGE_BINARY_PATH=bridges/xml/build/native/xml-bridge`.
   Documents travel as `Body::Bytes` raw (`into_bytes()` untranscoded), so
   Latin-9 bytes reach the bridge untouched. Failure text is captured in
   `CamelError::ProcessorError(e.to_string())`. grpcurl was rejected: not
   installed, and the port is mTLS-gated. JVM runs prove nothing (JVM has all
   charsets).
2. Failure is on the input side first; output side (`xsl:output encoding`)
   is a separate serializer charset need. The probe must keep the sides
   separate: 8859-15-in→UTF-8-out (assert `E2 82 AC` — proves U+20AC decode,
   not Latin-1 `¤`), UTF-8-in→8859-15-out (assert single byte `0xA4`),
   8859-15-in→8859-15-out, plus a pure-ASCII declaration case and the
   ISO-8859-1 / UTF-8 controls through the same paths.
3. Fix form (CORRECTED 2026-10-24 after build #2 disproof): declare
   `export QUARKUS_NATIVE_ADD_ALL_CHARSETS="true"` in
   `NATIVE_BUILD_ARGS`... (env file `native-build-args.env`). The original
   form — a `-H:+AddAllCharsets` token appended to
   `NATIVE_BUILD_ARGS_NEUTRAL` — is CANCELLED by Quarkus 3.39.1:
   `NativeImageBuildStep` appends `-H:-AddAllCharsets` AFTER the
   additional-build-args list (build #2: +0 bytes, probe unchanged).
   The Quarkus build-time property `quarkus.native.add-all-charsets`
   replaces the cancelling flag instead of fighting it (verified in
   quarkus-core-deployment-3.39.1 bytecode). The env-var spelling maps
   1:1 to the property via SmallRye env-config rules; it is a plain
   boolean, so the mislabeled-static YAML-precedence bug does not apply
   (and YAML must still not set it). build-native.sh sources the whole
   env file, so the export line needs zero script changes on linux/docker;
   xtask's win/macos path parses the file and gains a fail-closed
   passthrough (task 2.1). Aliases register with their charset: on the
   OUTPUT side any JDK alias resolves via `Charset.forName`; on the INPUT
   side the name must also be Xerces-mapped (`EncodingMap` sits in front
   of the JDK registry).
4. Traps to guard: Saxon may silently fall back to UTF-8 for an unknown output
   encoding — assert the output declaration AND byte-exact `0xA4` (euro in
   Latin-9), never `E2 82 AC`; decoded value must be U+20AC (`0xA4` is `¤` in
   Latin-1, so a Latin-1 misdecode must not pass). A parser failure on the
   euro could still re-serialize as `&#8364;` — compare exact bytes. The
   harness must refuse stale/downloaded binaries
   (`require_worktree_native_bridge`, never the `resolve_*` silent-skip path).
   Xerces alias note (verified against xercesImpl-2.12.2 `EncodingMap`): the
   mapped IANA names are `LATIN-9`, `ISO_8859-15`, `ISO8859_15_FDIS` — use
   `LATIN-9` for the alias scenario (`latin9` without the hyphen is unmapped
   and would fail even on the JVM).

## Approach

Two native builds plus one disproof build, one probe harness, run in the
mission worktree only:

- **Build #1 (reproduce, current args)**: done — see evidence `## before`.
- **Build #2 (disproof, cancelled flag)**: done — the `-H:+AddAllCharsets`
  token was cancelled by Quarkus (evidence `## cancelled-flag attempt`).
  The probe-hypothesis itself was CONFIRMED by build #1; only the flag
  mechanism was disproved.
- **Build #3 (fix, Quarkus env-config)**: xtask passthrough (task 2.1) +
  env-file export line (task 2.2), rebuild, same probe — all cases green
  including byte-exact euro round trip. Mechanism check: build log shows
  `+AddAllCharsets` with no later `-H:-AddAllCharsets`. Record runner
  binary size (`stat -c%s`) and startup (median wall time to the ready
  line over 10 runs) before/after. Cost threshold in ABSOLUTE terms:
  exceeding +5 MB binary size or +50 ms median ready time is an OWNER
  ESCALATION (record the delta, report, stop) — not a phase failure and
  not a silent pass. The explicit-list
  alternative stays closed regardless (needs a Feature class); the threshold
  only decides whether the owner is consulted on the measured cost.
- **Evidence**: all probe evidence lives in
  `openspec/changes/xml-native-charset/evidence.md` with three sections:
  `before`, `after`, `deltas`. Each probe section records the test command
  line (`--features integration-tests`), the test names that ran, pass/fail
  counts, and the verbatim failure text per failing test.
- **Docs**: posture section in `docs/src/services/bridge.md` and
  `docs/src/components/xml-transform.md`, worded to describe the behavior the
  probes OBSERVE (standard JDK charsets registered, Xerces-mapped IANA names
  resolve on input / JDK aliases on output, unknown
  charset names surface as `TRANSFORM_ERROR:` bridge errors), not assumed
  behavior: all standard JDK charsets + aliases guaranteed in native for
  `xsl:output` serialization, Xerces-mapped IANA names for input parsing; an
  `encoding` naming a charset outside the JDK registry fails with a
  `TRANSFORM_ERROR:` bridge error;
  CJK/EBCDIC decoders ship as part of `AddAllCharsets` (accepted surface,
  noted). If Saxon silently falls back to UTF-8 for an unknown output
  encoding instead of failing, record that as a separate bd finding — never
  as a pass.

Cost statement: native builds are expensive (~10–20 min each, dockerized
GraalVM musl-static). Exactly two builds are planned; no build matrix. Heavy
commands run under
`systemd-run --user --unit=<name> --collect -p MemoryMax=12G -p MemorySwapMax=2G --setenv=TMPDIR=/home/shared/tmp`.
sccache flakiness: on exit 254 retry once with `RUSTC_WRAPPER=` empty.

## Constraints

- `bridges/xml/native-build-args.env` stays the ONLY place native-image args
  live; `application.yml` must NOT set `quarkus.native.additional-build-args`
  or `quarkus.native.add-all-charsets` (SmallRye drops the env-var comma form
  when YAML carries the indexed form — the mislabeled-static bug).
- No changes to `bridges/jms` (mission 383 in flight).
- No Rust-side encoding changes; if the before-probe disproves the charset
  registry hypothesis, STOP and report instead of expanding scope.
- No version bump; master bumps xml after merge.

## Phases

1. **Reproduce in native (before)** — probe cases in `camel-test`, native
   build #1 at current args, verbatim failure evidence, controls green.
   Exit criteria: evidence file committed, 8859-15 red / controls green.
2. **Fix + prove (after)** — env file entry, native build #2, probe fully
   green, size/startup deltas recorded. Exit criteria: evidence file updated,
   byte-exact euro round trip green, deltas recorded; over threshold → owner
   escalation, phase paused rather than failed.
3. **Docs + gates** — posture docs, full gate registry, Java bridge tests.
   Exit criteria: gates green (or N/A enumerated), docs merged into tree.
