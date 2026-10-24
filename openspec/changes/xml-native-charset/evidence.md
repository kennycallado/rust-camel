# Evidence: xml-native-charset

## before

Pre-change `bridges/xml/native-build-args.env` (env file UNMODIFIED):

```
NATIVE_BUILD_ARGS_NEUTRAL="-H:+AllowVMInspection,-Djdk.xml.entityExpansionLimit=64,-Djdk.xml.totalEntitySizeLimit=5000000,-Djdk.xml.maxGeneralEntitySizeLimit=100000,-Djdk.xml.elementAttributeLimit=10000,--initialize-at-run-time=net.sf.saxon.functions.hof.RandomNumberGenerator,--initialize-at-run-time=org.apache.hc.client5.http.impl.auth.NTLMEngineImpl,--initialize-at-run-time=org.xmlresolver"
```

### Build #1 (pre-change args)

Command actually run (the plan's command plus `PATH`/`RUSTC_WRAPPER`
because the systemd user manager environment on this NixOS host lacks the
Nix rust/sccache wrapper entries; `systemd-run` also has no `--unsetenv`
on systemd 260, so `env -u` is used where needed):

```
systemd-run --user --unit=xml388-build-before --wait --pipe --same-dir --collect \
  -p MemoryMax=12G -p MemorySwapMax=2G \
  --setenv=TMPDIR=/home/shared/tmp --setenv=RUSTC_WRAPPER= --setenv=PATH="$PATH" \
  cargo xtask build-xml-bridge
```

- Exit code: `0` (first attempts exited `101`: `sccache`/`rustc` not on the
  systemd unit PATH; resolved by the `PATH`/`RUSTC_WRAPPER` adaptation)
- Wall time: `280s` (service runtime `4min 40.024s`)
- Binary: `/home/shared/rust-camel-worktrees/388-xml-native-charset/bridges/xml/build/native/xml-bridge`
- Binary size: `95028304` bytes (`file`: statically linked, stripped)
- SHA256: `46dd0efe3b3ab893ed10686f685da344c41790dee018f11ac35942bfdc757474`

### Stale-binary gate proof

```
systemd-run --user --unit=xml388-gateproof --wait --pipe --same-dir --collect \
  -p MemoryMax=12G -p MemorySwapMax=2G \
  --setenv=TMPDIR=/home/shared/tmp \
  --setenv=CAMEL_XML_BRIDGE_BINARY_PATH=/bin/sh \
  --setenv=RUSTC_WRAPPER= --setenv=PATH="$PATH" \
  env -u XML_BRIDGE_PATH \
  cargo test -p camel-test --features integration-tests \
  --test xml_bridge_charset_probe_test \
  charset_8859_15_ascii_declaration_parses -- --nocapture
```

gate-proof: exit `101`; verbatim panic (support/xml_bridge.rs:97):

```
thread 'charset_8859_15_ascii_declaration_parses' (1474975) panicked at crates/camel-test/tests/support/xml_bridge.rs:97:13:
assertion `left == right` failed: CAMEL_XML_BRIDGE_BINARY_PATH must point at the worktree native build (/home/shared/rust-camel-worktrees/388-xml-native-charset/bridges/xml/build/native/xml-bridge), not a stale checkout artifact
```

### Startup measurement (standalone)

Exact invocation (the plan template verbatim — `cd`, `set -o pipefail`
included — plus the three runtime TLS env vars
the bridge requires — PortAnnouncer aborts without them; runs from the
worktree root; identical command to be reused for build #2 in `## deltas`):

```
cd $ABS_WT
set -o pipefail
TLS=$ABS_WT/bridges/xml/src/test/resources/tls
for i in $(seq 1 10); do
  t0=$(date +%s%N)
  QUARKUS_TLS_BRIDGE_KEY_STORE_PEM_0_CERT=$TLS/test-server.pem \
  QUARKUS_TLS_BRIDGE_KEY_STORE_PEM_0_KEY=$TLS/test-server.key \
  QUARKUS_TLS_BRIDGE_TRUST_STORE_PEM_CERTS=$TLS/test-ca.pem \
  timeout 60 ./bridges/xml/build/native/xml-bridge > target/startup-$i.log 2>&1 &
  pid=$!
  if ! timeout 60 sh -c "until grep -q '\"status\":\"ready\",\"port\":' target/startup-$i.log; do sleep 0.05; done"; then
    kill $pid 2>/dev/null; echo "startup-not-ready run $i" >&2; exit 1
  fi
  t1=$(date +%s%N)
  kill $pid 2>/dev/null; wait $pid 2>/dev/null
  echo $(( (t1 - t0) / 1000000 ))
done | sort -n | awk '{a[NR]=$1} END {print a[int((NR+1)/2)]}'
```

Result: 10/10 runs reached the ready line. Per-run values (sorted, ms):
`60 60 60 60 60 60 60 60 61 61` — median `60` ms. Ready-line logs kept at
`target/startup-1.log` … `target/startup-10.log`.

An earlier run without the TLS env aborted before ready
(`Bridge started with placeholder TLS certs — runtime env vars not set.
Aborting.`); that was an invocation gap, not a startup regression.

### Probe suite against build #1

Exact test command line:

```
cargo test -p camel-test --features integration-tests \
  --test xml_bridge_charset_probe_test -- --nocapture
```

Containment (as run; `env -u` replaces the unavailable
`--unsetenv=XML_BRIDGE_PATH`):

```
systemd-run --user --unit=xml388-probe-before --wait --pipe --same-dir --collect \
  -p MemoryMax=12G -p MemorySwapMax=2G \
  --setenv=TMPDIR=/home/shared/tmp \
  --setenv=CAMEL_XML_BRIDGE_BINARY_PATH=/home/shared/rust-camel-worktrees/388-xml-native-charset/bridges/xml/build/native/xml-bridge \
  --setenv=RUSTC_WRAPPER= --setenv=PATH="$PATH" \
  env -u XML_BRIDGE_PATH \
  cargo test -p camel-test --features integration-tests \
  --test xml_bridge_charset_probe_test -- --nocapture
```

Result: `test result: FAILED. 11 passed; 5 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.40s`
(`cargo test` exit `101`, expected while red cases remain).

Charset probe tests (8) and outcomes:

| test | outcome |
| --- | --- |
| `charset_8859_15_ascii_declaration_parses` | RED |
| `charset_8859_15_euro_input_decodes_as_latin9` | RED |
| `charset_8859_15_output_side_serialized_byte_exact` | RED |
| `charset_8859_15_round_trip_in_and_out` | RED |
| `charset_alias_latin9_decl_resolves` | RED |
| `charset_controls_iso8859_1_and_utf8` | GREEN |
| `charset_unknown_input_name_transform_error` | GREEN |
| `charset_unknown_output_name_transform_error` | GREEN |

Support tests also collected by the `mod support;` include (all GREEN,
8 pass): `support::jms_journal::tests::invalid_utf8_id_is_corrupt`,
`support::jms_journal::tests::empty_id_is_corrupt`,
`support::jms_journal::tests::two_reserves_with_rollback_replay_two`,
`support::jms_journal::tests::torn_trailing_frame_is_discarded`,
`support::jms_journal::tests::unsupported_record_type_is_corrupt`,
`support::jms_journal::tests::reserve_then_commit_replays_zero`,
`support::tests::direct_not_registered_matcher_is_specific`,
`support::tests::direct_not_registered_retry_eventually_succeeds`.

Verbatim failure text per failing test (from `target/probe-before.log`):

```
thread 'charset_8859_15_ascii_declaration_parses' (1477645) panicked at crates/camel-test/tests/xml_bridge_charset_probe_test.rs:192:10:
ISO-8859-15 declared ASCII document must transform: ProcessorError("transform failed: TRANSFORM_ERROR: I/O error reported by XML parser processing null")
```

```
thread 'charset_8859_15_euro_input_decodes_as_latin9' (1477646) panicked at crates/camel-test/tests/xml_bridge_charset_probe_test.rs:207:10:
ISO-8859-15 euro byte must decode as U+20AC: ProcessorError("transform failed: TRANSFORM_ERROR: I/O error reported by XML parser processing null")
```

```
thread 'charset_8859_15_output_side_serialized_byte_exact' (1477647) panicked at crates/camel-test/tests/xml_bridge_charset_probe_test.rs:224:10:
ISO-8859-15 output encoding must transform UTF-8 euro input: ProcessorError("transform failed: TRANSFORM_ERROR: Unknown encoding requested: ISO-8859-15")
```

```
thread 'charset_8859_15_round_trip_in_and_out' (1477648) panicked at crates/camel-test/tests/xml_bridge_charset_probe_test.rs:244:10:
ISO-8859-15 euro round trip must transform: ProcessorError("transform failed: TRANSFORM_ERROR: Unknown encoding requested: ISO-8859-15")
```

```
thread 'charset_alias_latin9_decl_resolves' (1477649) panicked at crates/camel-test/tests/xml_bridge_charset_probe_test.rs:306:10:
LATIN-9 alias must resolve to ISO-8859-15: ProcessorError("transform failed: TRANSFORM_ERROR: I/O error reported by XML parser processing null")
```

Associated Saxon stderr for the input-side failures (verbatim):

```
Error SXXP0003  I/O error reported by XML parser processing null. Caused by
  java.io.UnsupportedEncodingException: ISO8859_15_FDIS
```

### Expectation correction

expectation-note: round-trip case surfaces the serializer error (Unknown encoding requested: ISO-8859-15) because Saxon's encoder check precedes input parsing; plan corrected 2026-10-24 (commit 29d0ee357). The three pure input-side cases carry the verbatim camel-112 parser text.

Authorization: the plan/spec correction was made by the CONDUCTOR
(mission 388 session, commit 29d0ee357 lineage and the phase-1 review
follow-up) after first-hand verification of the verbatim probe output
above; under the mission's autopilot rules this is a recorded
conductor decision, owner-visible in the park report (deferrals ledger
entry: plan-expectation-correction).

Control pass count (explicit): charset controls + unknown-charset cases
GREEN = 3 (`charset_controls_iso8859_1_and_utf8`,
`charset_unknown_input_name_transform_error`,
`charset_unknown_output_name_transform_error`); the other 8 passes in
the aggregate `11 passed` are the `mod support;` include tests.

Alias sub-case note: `charset_alias_latin9_decl_resolves` sub-case (b)
(the `ISO_8859-15` underscore alias) was NOT evaluated in the before run
— sub-case (a) panicked first and Rust tests abort at the first panic.
Sub-case (a) carries the before-run proof for the alias family; an
after-green on (b) alone is not before→after proof for that sub-case.

Isolated round-trip re-run (deviation confirmation, kept from the first
evidence pass): full output at `target/probe-roundtrip.log` (single-test
run of `charset_8859_15_round_trip_in_and_out`, same failure text).

## cancelled-flag attempt (build #2)

outcome: cancelled — Quarkus appended -H:-AddAllCharsets after our flag; +0 bytes; probe unchanged (5 RED / 3 GREEN)


### Build #2 (post-change args)

`bridges/xml/native-build-args.env` diff is exactly one appended token on
the existing line:

```
...,--initialize-at-run-time=org.xmlresolver,-H:+AddAllCharsets"
```

`grep -c 'AddAllCharsets' bridges/xml/native-build-args.env` → `1`. No
other file changed (`application.yml`, `build.gradle.kts`,
`build-native.sh` untouched).

SSOT check (step 2) run verbatim against the worktree: `HITS` empty,
`test -z "$HITS"` exit `0`.

Build command (same containment/adaptations as build #1):

```
systemd-run --user --unit=xml388-build-after --wait --pipe --same-dir --collect \
  -p MemoryMax=12G -p MemorySwapMax=2G \
  --setenv=TMPDIR=/home/shared/tmp --setenv=RUSTC_WRAPPER= --setenv=PATH="$PATH" \
  cargo xtask build-xml-bridge
```

- Exit code: `0`
- Wall time: `135s` (service runtime `2min 15.014s`)
- Binary: `/home/shared/rust-camel-worktrees/388-xml-native-charset/bridges/xml/build/native/xml-bridge`
- Binary size: `95028304` bytes (identical to build #1)
- SHA256: `047dd322ccc954d7e53584d958e08a73001a5ffc889772102bbeba7ee954dfee`

### Startup measurement (standalone)

Same template as `## before` (worktree root, `set -o pipefail`, TLS env,
10 runs, ready line, median). Measured twice:

- Run A (immediately after build #2): ready 10/10; sorted per-run ms
  `62 116 116 117 117 117 117 118 119 136`; median `117`.
  (Transient host-load outlier — see deltas note.)
- Run B (repeat, identical template): ready 10/10; sorted per-run ms
  `61 60 60 61 60 61 60 60 61 61`; median `60`.

Recorded startup median: `60` ms (run B; matches `## before` `60` ms).

### Probe suite against build #2

Exact invocation (probe template verbatim, unit `xml388-probe-after`):

```
systemd-run --user --unit=xml388-probe-after --wait --pipe --same-dir --collect \
  -p MemoryMax=12G -p MemorySwapMax=2G \
  --setenv=TMPDIR=/home/shared/tmp \
  --setenv=CAMEL_XML_BRIDGE_BINARY_PATH=/home/shared/rust-camel-worktrees/388-xml-native-charset/bridges/xml/build/native/xml-bridge \
  --setenv=PATH="$PATH" --setenv=RUSTC_WRAPPER= \
  env -u XML_BRIDGE_PATH \
  cargo test -p camel-test --features integration-tests \
  --test xml_bridge_charset_probe_test -- --nocapture
```

Result: `test result: FAILED. 11 passed; 5 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.37s`
(`cargo test` exit `101`). Full output: `target/probe-after.log`.

| test | outcome |
| --- | --- |
| `charset_8859_15_ascii_declaration_parses` | RED |
| `charset_8859_15_euro_input_decodes_as_latin9` | RED |
| `charset_8859_15_output_side_serialized_byte_exact` | RED |
| `charset_8859_15_round_trip_in_and_out` | RED |
| `charset_alias_latin9_decl_resolves` | RED |
| `charset_controls_iso8859_1_and_utf8` | GREEN |
| `charset_unknown_input_name_transform_error` | GREEN |
| `charset_unknown_output_name_transform_error` | GREEN |

Verbatim failures (identical to `## before`):

```
thread 'charset_8859_15_euro_input_decodes_as_latin9' panicked at crates/camel-test/tests/xml_bridge_charset_probe_test.rs:207:10:
ISO-8859-15 euro byte must decode as U+20AC: ProcessorError("transform failed: TRANSFORM_ERROR: I/O error reported by XML parser processing null")
```

```
thread 'charset_8859_15_output_side_serialized_byte_exact' panicked at crates/camel-test/tests/xml_bridge_charset_probe_test.rs:224:10:
ISO-8859-15 output encoding must transform UTF-8 euro input: ProcessorError("transform failed: TRANSFORM_ERROR: Unknown encoding requested: ISO-8859-15")
```

```
thread 'charset_8859_15_round_trip_in_and_out' panicked at crates/camel-test/tests/xml_bridge_charset_probe_test.rs:244:10:
ISO-8859-15 euro round trip must transform: ProcessorError("transform failed: TRANSFORM_ERROR: Unknown encoding requested: ISO-8859-15")
```

```
thread 'charset_8859_15_ascii_declaration_parses' panicked at crates/camel-test/tests/xml_bridge_charset_probe_test.rs:192:10:
ISO-8859-15 declared ASCII document must transform: ProcessorError("transform failed: TRANSFORM_ERROR: I/O error reported by XML parser processing null")
```

```
thread 'charset_alias_latin9_decl_resolves' panicked at crates/camel-test/tests/xml_bridge_charset_probe_test.rs:306:10:
LATIN-9 alias must resolve to ISO-8859-15: ProcessorError("transform failed: TRANSFORM_ERROR: I/O error reported by XML parser processing null")
```

### BLOCKED: Quarkus overrides the appended flag

The appended `-H:+AddAllCharsets` does not reach the effective native-image
command. Quarkus 3.39.1
(`io.quarkus.deployment.pkg.steps.NativeImageBuildStep$NativeImageInvokerInfo$Builder.build()`)
calls `handleAdditionalProperties` (which appends `quarkus.native.additional-build-args`,
bytecode offset 740) and then, based on `NativeConfig.addAllCharsets()`,
unconditionally appends `-H:-AddAllCharsets` when the property is false
(offsets 1104/1122). The last flag wins, so a `-H:+AddAllCharsets` in the
env var is a no-op.

Empirical confirmation:

- Build #2 binary size is byte-identical to build #1 (`95028304`) and all
  five charset probes fail with the exact same texts — the flag had no
  effect.
- Control on the same GraalVM CE 25.2.4 image: a tiny native image with
  `-H:+AddAllCharsets` reports `Charset.isSupported("ISO-8859-15") == true`;
  without it, `false`. The flag itself is valid and sufficient in GraalVM;
  Quarkus's trailing `-H:-AddAllCharsets` cancels it.
- The `entityExpansionLimit` args from the same env var do appear in the
  image, so env-var propagation and the build itself are fine.

Recommended plan amendment (NOT applied here — step 1 forbids other edits,
and the step-2 SSOT grep bans the property key in `bridges/xml`): enable
Quarkus's own switch, `quarkus.native.add-all-charsets=true` (e.g.
`QUARKUS_NATIVE_ADD_ALL_CHARSETS=true` in the build environment, or the
Gradle/native config). That makes Quarkus emit `-H:+AddAllCharsets` after
its own args.

## after

### Environment change (task 2.2)

`bridges/xml/native-build-args.env` now holds the platform-neutral comma
list PLUS one exported Quarkus env-config line:

```
NATIVE_BUILD_ARGS_NEUTRAL="...-H:+AllowVMInspection,...--initialize-at-run-time=org.xmlresolver"
export QUARKUS_NATIVE_ADD_ALL_CHARSETS="true"
```

The dead `,-H:+AddAllCharsets` token is gone: the `NATIVE_BUILD_ARGS_NEUTRAL`
line is byte-identical to its pre-change content (verified against
`git show 41a666067^:bridges/xml/native-build-args.env`). `application.yml`
and `build.gradle.kts` are untouched. The env-var form is mandatory:
YAML+env makes SmallRye drop the comma-form args (header rule).

Real-file pin added to `scripts/xtask` `#[cfg(test)] mod tests`:
`tests::xml_bridge_env_declares_add_all_charsets` reads the real
`bridges/xml/native-build-args.env` from the workspace root and asserts
`Ok(Some("true".to_string()))`. `cargo test -p xtask` (containment unit
`xml388-xtask-gate2` — plan said `xml388-xtask-gate`, the name was
suffixed `2` because the first unit was still registered; same flags):
exit `0`, `581 passed; 0 failed` in the main test binary. The seven
relevant charset-flag tests, all passing (final names after the phase-2
review fixes, commit a061ac91f):
`read_add_all_charsets_flag_parses_export_line`,
`read_add_all_charsets_flag_absent_is_none_for_other_bridges`,
`read_add_all_charsets_flag_rejects_non_boolean`,
`read_add_all_charsets_flag_rejects_unexported_assignment`,
`read_add_all_charsets_flag_rejects_duplicate_declarations`,
`add_all_charsets_flag_accepts_arbitrary_export_whitespace`,
`xml_bridge_env_declares_add_all_charsets` (the remainder are
pre-existing xtask tests).

SSOT check (step 2) run verbatim against the worktree: `HITS` empty,
`test -z "$HITS"` exit `0`.

### Build #3 (post-change args)

Command (same containment/adaptations as build #1):

```
systemd-run --user --unit=xml388-build-after3 --wait --pipe --same-dir --collect \
  -p MemoryMax=12G -p MemorySwapMax=2G \
  --setenv=TMPDIR=/home/shared/tmp --setenv=RUSTC_WRAPPER= --setenv=PATH="$PATH" \
  cargo xtask build-xml-bridge 2>&1 | tee target/build-after3.log
```

- Exit code: `0`
- Wall time: `136s` (service runtime `2min 15.591s`; Gradle `BUILD SUCCESSFUL in 2m 7s`)
- Binary: `/home/shared/rust-camel-worktrees/388-xml-native-charset/bridges/xml/build/native/xml-bridge`
- Binary size: `100861008` bytes (`file`: statically linked, stripped)
- SHA256: `917fc00bd975ea979fe289f01fcfd234b3482130bc0ff605098e25bae2322cc6`

### Mechanism check (step 3b)

3b-literal check not executable (Quarkus 3.39.1 does not echo argv);
replaced by functional and size evidence. [Deviation recorded 2026-10-24,
task 2.2 review finding 1.]

`target/build-after3.log` does NOT echo the native-image argument list:
Quarkus 3.39.1 prints only the GraalVM warning/summary sections, so a
direct `grep 'AddAllCharsets' target/build-after3.log` returns 0 matches
for both the `+` and `-` forms. The ordering is therefore pinned by these
observations:

1. The property reached the build: the Gradle daemon environment log
   (`bridges/xml/.gradle-docker-cache/daemon/9.7.1/daemon-263.out.log:49`)
   lists `QUARKUS_NATIVE_ADD_ALL_CHARSETS` among the configured env vars
   (variable NAME only — the log line does not show the value `true`).
2. Quarkus's own switch emits the positive form when the property is
   true: bytecode verification of `quarkus-core-deployment-3.39.1`
   (`NativeImageInvokerInfo$Builder.build()`, offsets 1104-1122: emits
   `+AddAllCharsets` or `-AddAllCharsets` from
   `NativeConfig.addAllCharsets()`, OR'd with `enable-all-charsets` build
   items, AFTER the additional-build-args collection at offset 633) —
   performed in the escalation consultation (session
   ses_e9509c1d1ffeLaPYL49gQw31jX, 2026-10-24). This is a verified
   mechanism claim, not a direct observation of build #3's argv.
3. The charset providers are in the image: `strings -a` on the build #3
   binary contains `iso8859_15`, `iso8859_15_fdis`, `iso-8859-15`,
   `latin-9`, `latin9`, `csisolatin9`. No negative-control `strings` run
   on the build #1/#2 binaries was recorded, so the absence claim is not
   independently evidenced; the size delta (+5832704 bytes vs build #2,
   same args except the env var) and the probe flip below carry the
   proof.
4. Ground truth: the probe flipped from 5 RED / 3 GREEN (build #2, the
   8 charset tests) to 8/8 GREEN charset tests (16 passed / 0 failed
   aggregate — includes the 8 `mod support;` tests) on build #3 — the
   cancellation is gone. [Counting clarified 2026-10-24, holistic
   review.]

### Startup measurement (standalone)

Same template as `## before` (worktree root, `set -o pipefail`, three TLS
env vars, 10 runs, ready line, median):

Result: 10/10 runs reached the ready line. Per-run values (sorted, ms):
`60 60 60 60 60 61 61 61 61 61` — **median `60` ms**. Logs kept at
`target/startup-after3-1.log` … `target/startup-after3-10.log`.

### Probe suite against build #3

Exact invocation (probe template verbatim, unit `xml388-probe-after3`,
captured to `target/probe-after3.log`):

```
systemd-run --user --unit=xml388-probe-after3 --wait --pipe --same-dir --collect \
  -p MemoryMax=12G -p MemorySwapMax=2G \
  --setenv=TMPDIR=/home/shared/tmp --setenv=PATH="$PATH" --setenv=RUSTC_WRAPPER= \
  --setenv=CAMEL_XML_BRIDGE_BINARY_PATH=/home/shared/rust-camel-worktrees/388-xml-native-charset/bridges/xml/build/native/xml-bridge \
  env -u XML_BRIDGE_PATH \
  cargo test -p camel-test --features integration-tests \
  --test xml_bridge_charset_probe_test -- --nocapture
```

Exit code: `0`. Verbatim result line:

```
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.39s
```

All 8 charset tests GREEN:

| test | outcome |
| --- | --- |
| `charset_8859_15_ascii_declaration_parses` | GREEN |
| `charset_8859_15_euro_input_decodes_as_latin9` | GREEN |
| `charset_8859_15_output_side_serialized_byte_exact` | GREEN |
| `charset_8859_15_round_trip_in_and_out` | GREEN |
| `charset_alias_latin9_decl_resolves` | GREEN |
| `charset_controls_iso8859_1_and_utf8` | GREEN |
| `charset_unknown_input_name_transform_error` | GREEN |
| `charset_unknown_output_name_transform_error` | GREEN |

The other 8 passes are the `mod support;` include tests (all GREEN).

## deltas

- Binary size: `95028304` → `100861008` bytes (`+5832704 bytes`,
  `+6.14%`).
- Startup median: `60` ms → `60` ms (`+0 ms`).
- Threshold check: startup within +50 ms. Size delta `+5832704` bytes
  exceeds the `+5242880`-byte (+5 MB) budget by `589824` bytes (0.56 MiB).
- `OWNER ESCALATION`: **TRIGGERED** — the charset table adds ~5.6 MiB to
  the static native image. Numbers for the owner: before `95028304`,
  after `100861008`, delta `+5832704` bytes (`+6.14%`).
- Status: `ESCALATED` — functional probes 8/8 GREEN; size budget breached.

### Recorded deviations (phase-2 review)

- The windows/macos half of the "both consumers propagate" scenario was
  NOT run end to end: only the parser unit tests and code inspection of
  `build_bridge_native` support it (no win/macos host in this mission).
- The xtask containment unit was named `xml388-xtask-gate2` / `-fix2`
  (first unit name still registered); same flags as planned.
