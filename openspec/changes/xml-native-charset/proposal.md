# Proposal: xml-native-charset

## Why

camel-112 reports that the xml bridge native image fails with
`I/O error reported by XML parser processing null` when a transform document or
stylesheet output declares `encoding="ISO-8859-15"` — even for pure-ASCII
content. ISO-8859-1 and UTF-8 stay green. The regression exists only in the
native image: GraalVM registers only the standard charsets by default, and
`bridges/xml/native-build-args.env` (the single source of truth for
platform-neutral native-image args) has no charset entry. Users currently strip
the encoding declaration as a workaround, which we should not require.

bd: rc-fnfm8 (P3 bug). Pre-flight expert verdict: GO-WITH-CAVEATS
(e_opus, 2026-10-23).

## What Changes

- Add native probe coverage to the existing xml bridge XSLT integration test
  harness (`camel-test`), gated on a worktree-built native binary, covering:
  ISO-8859-15 input declaration (ASCII-only and Latin-9 euro payload),
  ISO-8859-15 output encoding (byte-exact `0xA4` euro assertion), and
  ISO-8859-1 / UTF-8 controls — before and after the build-args change.
- Append `-H:+AddAllCharsets` to `NATIVE_BUILD_ARGS_NEUTRAL` in
  `bridges/xml/native-build-args.env` — the only place the arg may live.
  An explicit charset list is ruled out: it requires a custom GraalVM Feature
  class, which breaks the env-only single source of truth.
- Document the supported-encodings posture in the xml bridge docs
  (`docs/src/services/bridge.md`, `docs/src/components/xml-transform.md`):
  all standard JDK charsets (plus aliases) are guaranteed in native for
  `xsl:output` serialization, Xerces-mapped IANA names (e.g. `LATIN-9`,
  `ISO_8859-15`) for input parsing; an `encoding` naming a charset outside
  the JDK registry fails with a `TRANSFORM_ERROR:` bridge error.

Explicitly excluded (non-goals): Rust-side encoding handling changes (probe
must prove the bug is there instead), JVM-only fixes, version bumps
(master bumps xml afterwards).

## Acceptance criteria

- Native before-probe (current build args): input-side ISO-8859-15 cases fail
  with the exact reported parser failure text recorded verbatim; the
  output-only case's red output is recorded verbatim as-is; ISO-8859-1 and
  UTF-8 controls pass. Evidence committed in the change dir
  (`evidence.md`, sections before/after/deltas).
- Native after-probe (`-H:+AddAllCharsets`): all cases pass including the
  Latin-9 euro round trip through XSLT output as single byte `0xA4`; image size
  and startup deltas recorded (+5 MB binary size or +50 ms median ready time →
  owner escalation: report and stop).
- `native-build-args.env` remains the only file carrying native-image args.
- Docs state the supported-encodings posture.

## Risk budget

Acceptable: +1–3 MB native image growth from CJK/EBCDIC decoders (security
surface note recorded in docs). Out of bounds: changes to `bridges/jms`
(mission 383 in flight), charset handling in Rust crates, YAML-side
`quarkus.native.add-all-charsets` (would create a second source of truth).
