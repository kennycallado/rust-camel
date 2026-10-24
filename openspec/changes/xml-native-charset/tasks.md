# Tasks: xml-native-charset

<!--
  Multi-phase plan: phases are an implementation-ordering construct
  (design.md ## Phases). The WHOLE file is plan-blessed ONCE.
  Native builds are expensive: exactly TWO dockerized GraalVM builds
  are planned (build #1 before the env change, build #2 after).
  Every heavy command runs under systemd-run with:
    --wait --pipe --same-dir --collect -p MemoryMax=12G \
    -p MemorySwapMax=2G --setenv=TMPDIR=/home/shared/tmp
  (--wait --pipe so the exit code and stdout are capturable;
  --same-dir so the unit inherits the invoking cwd; env vars via
  --setenv/--unsetenv, never shell-prefix inside the unit).
  ABS_WT below denotes /home/shared/rust-camel-worktrees/388-xml-native-charset
  (workers expand it literally; test binaries run with cwd
  crates/camel-test, so binary paths MUST be absolute).
  Environment adaptations (observed on this host, required for every
  systemd-run command below): the unit does not inherit the shell PATH
  (Nix rustc/sccache) — add `--setenv=PATH="$PATH"`; systemd 260 has no
  --unsetenv — neutralize XML_BRIDGE_PATH with an `env -u XML_BRIDGE_PATH`
  prefix on the unit command; keep `--setenv=RUSTC_WRAPPER=` (empty) to
  avoid flaky sccache.
  sccache flakiness: on exit 254 retry once with RUSTC_WRAPPER= empty.
  Probe run template (used verbatim in 1.2 step 6 and 2.1 step 5, only
  the unit name changes):
    systemd-run --user --unit=<unit> --wait --pipe --same-dir --collect \
      -p MemoryMax=12G -p MemorySwapMax=2G \
      --setenv=TMPDIR=/home/shared/tmp \
      --setenv=PATH="$PATH" --setenv=RUSTC_WRAPPER= \
      --setenv=CAMEL_XML_BRIDGE_BINARY_PATH=$ABS_WT/bridges/xml/build/native/xml-bridge \
      env -u XML_BRIDGE_PATH \
      cargo test -p camel-test --features integration-tests \
        --test xml_bridge_charset_probe_test -- --nocapture
  Startup measurement template (used verbatim in 1.2 step 5 and 2.1
  step 4; ready line literal from PortAnnouncer.java:88; runnable bash,
  prints one duration in ms per run — take the median; pipefail makes a
  not-ready abort fail the whole pipeline; the bridge aborts without
  runtime TLS env (PortAnnouncer.java:52), so the three TLS vars point at
  the repo's test material):
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
  Any `startup-not-ready` exit (pipeline exit != 0 under pipefail) means
  STOP and report `startup-not-ready`.
-->

## Phase 1: Reproduce in native (before the fix)

### camel-test

#### Task 1.1: Charset probe test cases in camel-test

**Files:**
- `crates/camel-test/tests/xml_bridge_charset_probe_test.rs` (new)

**Steps:**
1. Create `crates/camel-test/tests/xml_bridge_charset_probe_test.rs` with
   `#![cfg(feature = "integration-tests")]`, `mod support;`, and the same
   imports/pattern as `crates/camel-test/tests/xml_bridge_xslt_test.rs`
   (`XsltComponent` + `CamelTestContext` + `direct:start` YAML route via
   `send_to_direct`). Every test obtains its binary via
   `support::xml_bridge::require_worktree_native_bridge()` — NEVER
   `require_xml_bridge_binary()` and never a `resolve_*` silent-skip path.
2. Add stylesheet/tempfile helpers following `write_xslt()` in
   `xml_bridge_xslt_test.rs`, parameterized over `xsl:output @encoding`
   (one helper: `write_stylesheet(output_encoding: Option<&str>)`; pass
   `None` for `omit-xml-declaration="yes"` default UTF-8 output).
3. Write documents as raw bytes: `Body::Bytes` from explicitly encoded
   byte vectors (ISO-8859-15: byte `0xA4` = euro U+20AC; ISO-8859-1: byte
   `0xA4` = currency sign U+00A4; UTF-8: `E2 82 AC`). Do NOT use
   `String::from_utf8` on non-UTF-8 payloads.
4. Implement the eight test functions listed under Tests below. Assert
   bytes exactly (`assert_eq!` on `&[u8]` slices / `windows()` scans); for
   error cases assert that the `CamelError::ProcessorError` text CONTAINS
   `TRANSFORM_ERROR:` (see the wrapping note in Tests — the Rust client
   prefixes `transform failed: `, so `starts_with` would be wrong).
5. Wrap the clippy and compile gates in the systemd-run containment
   (unit `xml388-compile-gate`, `--wait --pipe --same-dir`, same
   MemoryMax/TMPDIR flags as the header):
   `cargo fmt -p camel-test --check` (light, no containment) must exit 0;
   `cargo clippy -p camel-test --features integration-tests --all-targets -- -D warnings` must exit 0;
   fix findings.
6. Compile-gate the suite without a binary present (same containment,
   unit `xml388-compile-gate`):
   `cargo test -p camel-test --features integration-tests --test xml_bridge_charset_probe_test`
   must compile and every test must FAIL with a panic whose text contains
   `xml-bridge binary not found at` (the require gate's missing-binary
   panic — the worktree binary does not exist yet; the stale-binary
   variant of this gate is proven in Task 1.2 step 4b, once the binary
   exists).
7. Commit.

**Tests:** (executable spec — name, arrange, act, assert)

Canonical fixtures (byte literals in Rust `b"\xA4"` escape form):
- `DOC_8859_15_DECL`: the ASCII bytes `<?xml version="1.0" encoding="ISO-8859-15"?>`
- `DOC_LATIN9_DECL`: the ASCII bytes `<?xml version="1.0" encoding="LATIN-9"?>`
- `DOC_8859_1_DECL`: the ASCII bytes `<?xml version="1.0" encoding="ISO-8859-1"?>`
- input-side stylesheet body (fixed, UTF-8 out):
  `<?xml version="1.0" encoding="UTF-8"?><xsl:stylesheet version="3.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform"><xsl:output method="xml" omit-xml-declaration="yes"/><xsl:template match="/doc"><out><xsl:value-of select="sym"/></out></xsl:template></xsl:stylesheet>`
- output-side stylesheet body (encoding parameterized, declaration NOT
  omitted):
  `<?xml version="1.0" encoding="UTF-8"?><xsl:stylesheet version="3.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform"><xsl:output method="xml" encoding="{ENC}"/><xsl:template match="/doc"><out><xsl:value-of select="sym"/></out></xsl:template></xsl:stylesheet>`
  with `{ENC}` replaced by the case's encoding literal.

- `charset_8859_15_ascii_declaration_parses`: doc bytes =
  `DOC_8859_15_DECL + b"<doc><sym>AB-42</sym></doc>"` (pure ASCII);
  input-side stylesheet → transform →
  `assert_eq!(out, b"<out>AB-42</out>")` (FULL byte equality).
- `charset_8859_15_euro_input_decodes_as_latin9`: doc bytes =
  `DOC_8859_15_DECL + b"<doc><sym>\xA4</sym></doc>"`; input-side
  stylesheet → transform → `assert_eq!(out, b"<out>\xE2\x82\xAC</out>")`
  (FULL byte equality; a Latin-1 misdecode would yield
  `<out>\xC2\xA4</out>` and fail).
- `charset_8859_15_output_side_serialized_byte_exact`: doc bytes =
  `b"<?xml version=\"1.0\" encoding=\"UTF-8\"?><doc><sym>\xE2\x82\xAC</sym></doc>"`;
  output-side stylesheet with `ENC=ISO-8859-15` → transform → output
  starts with `b"<?xml version=\"1.0\" encoding=\"ISO-8859-15\"?>"`, the
  body after the declaration is exactly `b"<out>\xA4</out>"`, and the
  full output contains NEITHER `b"\xE2\x82\xAC"` NOR `b"&#8364;"`.
- `charset_8859_15_round_trip_in_and_out`: doc bytes =
  `DOC_8859_15_DECL + b"<doc><sym>\xA4</sym></doc>"`; output-side
  stylesheet with `ENC=ISO-8859-15` → transform → same assertions as the
  output-side case (declaration prefix + `b"<out>\xA4</out>"` body +
  negative UTF-8/entity scans).
- `charset_controls_iso8859_1_and_utf8`: three sub-cases, all FULL
  `assert_eq!`: (a) doc = `DOC_8859_1_DECL + b"<doc><sym>\xA4</sym></doc>"`
  (0xA4 = U+00A4 in Latin-1), input-side stylesheet →
  `b"<out>\xC2\xA4</out>"`; (b) doc =
  `b"<?xml version=\"1.0\" encoding=\"UTF-8\"?><doc><sym>\xE2\x82\xAC</sym></doc>"`,
  input-side stylesheet → `b"<out>\xE2\x82\xAC</out>"`; (c) doc as (a)
  (U+20AC does not exist in Latin-1, so the output-side control uses the
  Latin-1-encodable U+00A4 doc), output-side stylesheet with
  `ENC=ISO-8859-1` → output starts with
  `b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?>"` and body after
  declaration is exactly `b"<out>\xA4</out>"` (U+00A4 as single byte).
  [Correction 2026-10-24, task 1.1 review finding: (c) originally
  specified the UTF-8 euro doc, unsatisfiable under ISO-8859-1 output
  since Saxon must emit a character reference; fixed to the (a) doc.]
- `charset_alias_latin9_decl_resolves`: two sub-cases, both FULL
  `assert_eq!`: (a) doc bytes =
  `DOC_LATIN9_DECL + b"<doc><sym>\xA4</sym></doc>"`; (b) doc bytes =
  `DOC_8859_15_DECL` written with `ISO_8859-15` spelled
  `<?xml version="1.0" encoding="ISO_8859-15"?>` +
  `b"<doc><sym>\xA4</sym></doc>"` (the second Xerces-mapped alias form);
  input-side stylesheet → transform → both yield
  `b"<out>\xE2\x82\xAC</out>"` (alias reached the registered charset;
  `C2 A4` must not pass).
- `charset_unknown_input_name_transform_error`: doc bytes =
  `b"<?xml version=\"1.0\" encoding=\"X-BOGUS-CHARSET\"?><doc><sym>x</sym></doc>"`
  → transform → the observed `CamelError::ProcessorError` text contains
  `TRANSFORM_ERROR:` (the Rust client wraps the bridge error as
  `transform failed: {bridge message}` — client.rs/error.rs — so the full
  text is `transform failed: TRANSFORM_ERROR: …`; the BRIDGE-level
  message itself starts with `TRANSFORM_ERROR:`, which is what the spec
  scenario asserts).
- `charset_unknown_output_name_transform_error`: output-side stylesheet
  with `ENC=X-BOGUS-CHARSET`, doc bytes = the UTF-8 euro document from
  control (b) (`b"<?xml version=\"1.0\" encoding=\"UTF-8\"?><doc><sym>\xE2\x82\xAC</sym></doc>"`,
  so the input side parses cleanly and the failure is genuinely
  output-side) → transform → error text contains `TRANSFORM_ERROR:`
  (same wrapping note; if Saxon silently falls back to UTF-8 instead, the
  byte assertions fail — record that output verbatim in the task report
  as a `saxon-fallback` finding, do NOT weaken the assertion).

**Acceptance:**
- `cargo fmt -p camel-test --check` exits 0.
- `cargo clippy -p camel-test --features integration-tests --all-targets -- -D warnings` exits 0.
- `cargo test -p camel-test --features integration-tests --test xml_bridge_charset_probe_test` compiles; with no worktree binary present every test's panic text contains `xml-bridge binary not found at`.
- No `unwrap()` introduced outside existing file conventions (`cargo xtask lint-unwrap` passes).
- Task report lists the eight test names.

- [x] 1.1

### native-build (evidence)

#### Task 1.2: Native build #1 (current args) + before evidence

**Files:**
- `openspec/changes/xml-native-charset/evidence.md` (new)

**Steps:**
1. Verify docker is available (`docker info` exits 0). If not, STOP and
   report `native-build-blocked: no docker` — do not degrade to JVM.
2. Record the pre-change env file line (the full current
   `NATIVE_BUILD_ARGS_NEUTRAL=` line, quoted verbatim from
   `bridges/xml/native-build-args.env`) in `evidence.md` under a `## before`
   heading.
3. Build native image #1 in the worktree:
   `systemd-run --user --unit=xml388-build-before --wait --pipe --same-dir --collect -p MemoryMax=12G -p MemorySwapMax=2G --setenv=TMPDIR=/home/shared/tmp cargo xtask build-xml-bridge`
   (cwd: mission worktree root). On exit 254 retry once with
   `RUSTC_WRAPPER=` prepended as `--setenv=RUSTC_WRAPPER=` to the same
   systemd-run command line. Record wall time and final exit code.
4. Record binary size: `stat -c%s $ABS_WT/bridges/xml/build/native/xml-bridge`.
4b. Stale-binary gate proof (binary now exists): re-run ONE probe test
   under the probe containment with
   `--setenv=CAMEL_XML_BRIDGE_BINARY_PATH=/bin/sh` (an existing file that
   is NOT the worktree build); it must FAIL with a panic whose text
   contains `must point at the worktree native build`. Record the panic
   line in `evidence.md ## before` under `gate-proof:`.
5. Record startup: execute the startup measurement template from the
   file-header comment verbatim (10 runs, ready line
   `{"status":"ready","port":`, median in ms).
6. Run the probe against build #1 using the probe run template from the
   file-header comment verbatim, unit name `xml388-probe-before`
   (absolute binary path via `--setenv`; `XML_BRIDGE_PATH` unset via
   `--unsetenv`).
7. Write `evidence.md` `## before` section: the exact test command line,
   the list of test names that ran, per-test pass/fail outcome, and the
   VERBATIM failure text for each failing test (copy exact assertion/panic
   lines from probe-before.log; do not paraphrase). Create empty `## after`
   and `## deltas` headings at the same time (filled in Task 2.1). The
   expected outcome set is EXACT (corrected 2026-10-24 after the actual
   build-#1 run — in the combined in-and-out case the serializer's
   encoder check runs BEFORE the parser, so it surfaces the Saxon error,
   not the parser text):
   - RED with verbatim `I/O error reported by XML parser processing null`
     in each failure text: `charset_8859_15_ascii_declaration_parses`,
     `charset_8859_15_euro_input_decodes_as_latin9`,
     `charset_alias_latin9_decl_resolves`.
   - RED with output recorded verbatim as-is (failure text NOT
     pre-asserted): `charset_8859_15_output_side_serialized_byte_exact`,
     `charset_8859_15_round_trip_in_and_out` (observed:
     `Unknown encoding requested: ISO-8859-15` — Saxon serializer encoder
     check precedes input parsing; still a charset-registry failure).
   - GREEN: `charset_controls_iso8859_1_and_utf8`,
     `charset_unknown_input_name_transform_error`,
     `charset_unknown_output_name_transform_error` (control pass count
     must be non-zero).
   ANY deviation from this exact red/green set means STOP and report
   `probe-hypothesis-mismatch: <detail>` — do NOT touch the env file and
   do NOT proceed to Phase 2 (owner order: report instead of expanding
   scope).
8. Commit `evidence.md`. Worktree git status must be clean after commit.

**Tests:**
- Evidence check (no new code): `evidence.md` `## before` must contain the
  verbatim string `I/O error reported by XML parser processing null` in the
  failure text of the three pure input-side test names, the test command
  line, all eight test names, and a non-zero control pass count.

**Acceptance:**
- `cargo xtask build-xml-bridge` exit 0; binary at
  `bridges/xml/build/native/xml-bridge`.
- `evidence.md` exists with `## before`, `## after` placeholder headings
  (after filled in Task 2.1) and `## deltas`.
- Before-pattern confirmed: input-side 8859-15 red with verbatim parser
  text, output-only red recorded as-is, controls green, unknown-charset
  green.
- Worktree git status clean after commit.

- [x] 1.2

## Phase 2: Fix + prove (after the fix)

<!--
  AMENDED 2026-10-24 (consultation e_opus, session ses_e9509c1d1ffeLaPYL49gQw31jX):
  the blessed mechanism (append -H:+AddAllCharsets to the comma list) is
  cancelled by Quarkus 3.39.1 — NativeImageBuildStep appends -H:-AddAllCharsets
  AFTER additional-build-args (build #2, commit 41a666067: +0 bytes, probe
  unchanged 5 RED / 3 GREEN). Working mechanism per bytecode verification of
  quarkus-core-deployment-3.39.1: the build-time property
  quarkus.native.add-all-charsets=true makes Quarkus emit -H:+AddAllCharsets
  itself. Declared in the env file as an export line (build-native.sh sources
  the whole file -> zero script changes); xtask gains a fail-closed parser.
  Build numbering: build #1 = pre-change (evidence ## before), build #2 =
  cancelled-flag attempt (evidence ## cancelled-flag attempt), build #3 =
  the real after build (evidence ## after). Three builds total, no matrix.
-->

### scripts/xtask

#### Task 2.1: xtask propagates the charset opt-in (fail-closed)

**Files:**
- `scripts/xtask/src/main.rs` (modified)

**Steps:**
1. Add `fn read_add_all_charsets_flag(bridge_path: &Path) -> Result<Option<String>, String>`
   beside `read_neutral_native_build_args` (~line 1182): scan
   `native-build-args.env` for a line whose trimmed form starts with
   `export QUARKUS_NATIVE_ADD_ALL_CHARSETS=`; strip a leading `export ` and
   surrounding double quotes; return `Ok(Some(value))`. When NO such line
   exists return `Ok(None)` — ABSENCE IS VALID because `build_bridge_native`
   is shared by the jms, cxf and xml bridges and only xml declares the
   line (a hard error would break jms/cxf windows/macos builds). Fail
   closed with an error naming the file ONLY for malformed forms: a value
   that is not `true`/`false`, OR the unexported assignment form (the
   `QUARKUS_NATIVE_ADD_ALL_CHARSETS=` prefix WITHOUT the leading
   `export ` — a shell-local that silently never reaches gradle on
   linux). Do NOT
   match the `NATIVE_BUILD_ARGS_NEUTRAL` line.
2. In `build_bridge_native` (the win/macos gradle invocation path), call
   it; on `Ok(Some(flag))` pass it to the gradle command environment
   beside the existing `QUARKUS_NATIVE_ADDITIONAL_BUILD_ARGS` export
   (`.env("QUARKUS_NATIVE_ADD_ALL_CHARSETS", flag)`, site near line 1135);
   on `Ok(None)` set nothing; on `Err` return the error. linux/docker
   needs no change: build-native.sh sources the whole env file, so the
   export line reaches gradle directly.
3. Add unit tests in the existing `#[cfg(test)] mod tests` (style of the
   tests at ~line 1615). (i)-(iii) parse a temp env file (use
   `std::env::temp_dir` + unique subdir, as existing tests do):
   (i) with `export QUARKUS_NATIVE_ADD_ALL_CHARSETS="true"` ->
   `Ok(Some("true".to_string()))`; (ii) without the line ->
   `Ok(None)`; (iii) with `export QUARKUS_NATIVE_ADD_ALL_CHARSETS="banana"`
   -> Err; (iv) with the unexported form
   `QUARKUS_NATIVE_ADD_ALL_CHARSETS="true"` (no `export `) -> Err.
   NOTE: the real-file test `xml_bridge_env_declares_add_all_charsets` is
   added in Task 2.2 step 1c — NOT here (the env line does not exist yet;
   adding it now would break this task's own `cargo test -p xtask` gate).
4. Gates: `cargo fmt -p xtask --check` exit 0;
   `cargo clippy -p xtask --all-targets -- -D warnings` exit 0;
   `cargo test -p xtask` (the new tests pass; existing tests unaffected);
   `RUSTDOCFLAGS="-D warnings" cargo doc -p xtask --no-deps` exit 0.
   Wrap clippy/test/doc in the systemd-run containment (unit
   `xml388-xtask-gate`, header flags + `--setenv=PATH="$PATH" --setenv=RUSTC_WRAPPER=`).
5. Commit: `feat(xtask): propagate charset opt-in from bridge env file (task 2.1)`.

**Tests:** (executable spec — name, arrange, act, assert; final names
after phase-2 review fixes, commit a061ac91f)
- `read_add_all_charsets_flag_parses_export_line`: temp env file with
  `export QUARKUS_NATIVE_ADD_ALL_CHARSETS="true"` (+ a NATIVE_BUILD_ARGS_NEUTRAL
  line present) → call → `Ok(Some("true".to_string()))`.
- `read_add_all_charsets_flag_absent_is_none_for_other_bridges`: temp env
  file with only the NEUTRAL line → call → `Ok(None)` (jms/cxf keep
  building).
- `read_add_all_charsets_flag_rejects_non_boolean`: temp env file with
  `export QUARKUS_NATIVE_ADD_ALL_CHARSETS="banana"` → call → Err naming
  `native-build-args.env`.
- `read_add_all_charsets_flag_rejects_unexported_assignment`: temp env
  file with `QUARKUS_NATIVE_ADD_ALL_CHARSETS="true"` (no `export `) →
  call → Err (silent-loss trap: shell-local never reaches gradle).
- `read_add_all_charsets_flag_rejects_duplicate_declarations` (phase-2
  review fix): temp file with two declaration lines ("true" then "false")
  → Err (bash source semantics make the LAST one win; divergent
  linux/win values must fail closed).
- `add_all_charsets_flag_accepts_arbitrary_export_whitespace` (review
  fix): `export` followed by tab/multi-space before the prefix →
  `Ok(Some("true".to_string()))` (valid bash; previously silently
  ignored → linux/win asymmetry).
  (The real-file test `xml_bridge_env_declares_add_all_charsets` belongs
  to Task 2.2 — see its Tests block.)

**Acceptance:**
- All six temp-file unit tests pass; `cargo test -p xtask` exit 0.
- fmt/clippy/doc gates exit 0.
- linux/docker path untouched (no build-native.sh changes).
- `Ok(None)` path proven by the absent-line test — jms/cxf unaffected.

- [x] 2.1

### bridges/xml

#### Task 2.2: env-file charset opt-in + native build #3 + after/deltas evidence

**Files:**
- `bridges/xml/native-build-args.env` (modified)
- `scripts/xtask/src/main.rs` (modified — real-file unit test, step 1c)
- `openspec/changes/xml-native-charset/evidence.md` (modified)

**Steps:**
1. Edit `bridges/xml/native-build-args.env`: (a) REMOVE the dead
   `,-H:+AddAllCharsets` token from the `NATIVE_BUILD_ARGS_NEUTRAL` line
   (restore that line to its pre-change content — Quarkus cancels the
   token; build #2 proved it); (b) ADD a new line after the
   `NATIVE_BUILD_ARGS_NEUTRAL=` line:
   `export QUARKUS_NATIVE_ADD_ALL_CHARSETS="true"`;
   (c) update the file's header comment: the file now holds the
   platform-neutral comma list PLUS exported Quarkus native env-config;
   note that the env-var form (not YAML) is mandatory because YAML+env
   precedence silently drops the comma form (existing header rule).
   Change nothing else; do NOT touch `application.yml` or `build.gradle.kts`.
1c. Add the real-file unit test to scripts/xtask `#[cfg(test)] mod tests`:
   `xml_bridge_env_declares_add_all_charsets` reads the REAL
   `bridges/xml/native-build-args.env` from the workspace root and asserts
   `Ok(Some("true".to_string()))` — this pins the xml bridge against a
   missing declaration. Run `cargo test -p xtask` (systemd-run
   containment, unit `xml388-xtask-gate`): all seven tests pass now.
2. Verify single source of truth (must succeed, empty output):
   ```
   HITS=$(git -C "$ABS_WT" grep -nE 'AddAllCharsets|ADD_ALL_CHARSETS|quarkus\.native\.(add-all-charsets|additional-build-args)|^[^#]*(add-all-charsets|additional-build-args)[[:space:]]*:' -- bridges/xml | grep -v '^bridges/xml/native-build-args.env' | grep -vE ':[0-9]+:[[:space:]]*#')
   test -z "$HITS"
   ```
   (The working-mechanism check moved to step 3b, which runs after the
   build log exists.)
3. Build native image #3 (the REAL after build):
   `systemd-run --user --unit=xml388-build-after3 --wait --pipe --same-dir --collect -p MemoryMax=12G -p MemorySwapMax=2G --setenv=TMPDIR=/home/shared/tmp --setenv=RUSTC_WRAPPER= --setenv=PATH="$PATH" cargo xtask build-xml-bridge 2>&1 | tee target/build-after3.log`
   from the worktree root. Record wall time + exit code (the tee pipeline
   exit; verify the build exit via the gradle success line in the log).
3b. Mechanism check on target/build-after3.log: the native-image argument
   list must contain `+AddAllCharsets` and must NOT contain any
   `-H:-AddAllCharsets` occurrence AFTER it (the ordering bug that killed
   build #2). Record the matched line(s) in evidence. A failure here →
   STOP, report `mechanism-not-applied: <detail>`.
4. Record binary size (`stat -c%s`) and startup median (the exact
   invocation recorded in evidence.md `### Startup measurement` — TLS env
   included, 10 runs, per-run values + median).
5. Run the FULL probe suite (probe run template, unit `xml388-probe-after3`).
   Expected: ALL 8 charset tests GREEN (exit 0). Capture to
   target/probe-after3.log.
6. Restructure `evidence.md`: rename the current `## after` content to
   `## cancelled-flag attempt (build #2)` with one added line
   `outcome: cancelled — Quarkus appended -H:-AddAllCharsets after our flag; +0 bytes; probe unchanged (5 RED / 3 GREEN)`; then write the real
   `## after` (build #3: command, exit, wall, size, sha256, startup
   median + per-run values, mechanism-check lines from step 3b, probe
   command line, all 8 test names GREEN, verbatim `test result:` line)
   and rewrite `## deltas`: size 95028304 → after (+X bytes), startup
   median 60 ms → after (+X ms), using the SAME invocation as `## before`.
   If size delta > +5 MB (5242880 bytes) OR startup delta > +50 ms: mark
   `OWNER ESCALATION` with the numbers.
7. Commit ALL THREE files ALWAYS (even on escalation):
   `feat(xml-bridge): register all charsets via quarkus env-config (task 2.2)`.
   On escalation: commit first, then STOP and report. Tree clean after.

**Tests:**
- `xml_bridge_env_declares_add_all_charsets`: the REAL
  `bridges/xml/native-build-args.env` (post-step-1) → call →
  `Ok(Some("true".to_string()))`; gate: `cargo test -p xtask` exits 0 with
  ALL SEVEN tests (the six temp-file tests from 2.1 + this one).
- Full probe suite green (unit `xml388-probe-after3`) exits 0, all eight
  charset tests pass.
- Mechanism check: build log (target/build-after3.log) shows
  `+AddAllCharsets` with no later `-H:-AddAllCharsets` (step 3b).

**Acceptance:**
- env file: NEUTRAL line byte-identical to pre-change; one new export
  line; header comment updated.
- `cargo test -p xtask` exit 0 (seven relevant tests, incl. the real-file test).
- SSOT check empty; probe suite 8/8 GREEN; evidence complete.
- Tree clean (all three files committed).

- [x] 2.2 <!-- ESCALATED: +5,832,704 bytes (+6.14%) exceeds the +5 MB budget; startup +0 ms; probe 8/8 GREEN; committed and stopped per plan; owner decides at merge gate -->

## Phase 3: Docs + Java bridge verification

### docs

#### Task 3.1: Supported-encodings posture documentation

**Files:**
- `docs/src/services/bridge.md` (modified)
- `docs/src/components/xml-transform.md` (modified)

**Steps:**
1. In `docs/src/services/bridge.md`, add a new `## XML bridge: supported
   encodings in native` section placed immediately after the existing
   `## Environment overrides` section (bridge.md has no xml-bridge H2
   today; its H2s are Architecture, gRPC channel, Process lifecycle,
   Binary acquisition, Reconnect contract, Credential redaction,
   Configuration, Environment overrides). Word it to the OBSERVED probe
   behavior [amended to the env-config mechanism, holistic review]:
   the native image registers all standard JDK charsets via the
   build-time property `quarkus.native.add-all-charsets`, declared as
   `export QUARKUS_NATIVE_ADD_ALL_CHARSETS="true"` in
   `bridges/xml/native-build-args.env` (the single source of truth;
   Quarkus then emits `-H:+AddAllCharsets` itself); XML declarations
   naming Xerces-mapped IANA encodings parse (probed forms:
   `ISO-8859-15`, aliases `LATIN-9` / `ISO_8859-15`; others expected by
   the same registry mechanism, not separately measured); `xsl:output
   @encoding` serializes for the probed charset with byte-exact
   verification (ISO-8859-15 euro round trip), the rest expected by the
   same registry mechanism; an encoding name outside the JDK registry
   surfaces as a `TRANSFORM_ERROR:` bridge error on both the input and
   output side. Note that a JVM bridge runs on the full JDK runtime,
   which always carried every JDK charset — the opt-in matters only for
   the native image.
2. In `docs/src/components/xml-transform.md`, add a short pointer to that
   section plus the user-visible rule: stripping an encoding declaration
   is NOT required; declare the actual encoding of your bytes. Name the
   `QUARKUS_NATIVE_ADD_ALL_CHARSETS` opt-in and note the accepted
   surface: the opt-in ships CJK and EBCDIC decoders too (measured image
   growth +5.6 MiB, 95028304 → 100861008 bytes; startup unchanged,
   median 60 ms → 60 ms).
3. English prose per the language policy; no code blocks > 5 lines.
4. Run `cd docs && mdbook test` (docs changed). Fix any link/test
   failures.
5. Commit both docs files.

**Tests:**
- `cd docs && mdbook test` exits 0.

**Acceptance:**
- Both files carry the charset opt-in posture (bridge.md the full
  mechanism + measurements; xml-transform.md at minimum the
  `QUARKUS_NATIVE_ADD_ALL_CHARSETS` name, the CJK/EBCDIC surface, and a
  link) and the `TRANSFORM_ERROR:` failure behavior.
- `mdbook test` exits 0.
- No Rust or Java sources touched by this task.

- [x] 3.1

### bridges/xml

#### Task 3.2: Java bridge test suite green

**Files:**
- none modified (verification-only task)

**Steps:**
1. Run the JVM test suite in the worktree:
   `systemd-run --user --unit=xml388-java-tests --wait --pipe --collect -p MemoryMax=12G -p MemorySwapMax=2G --setenv=TMPDIR=/home/shared/tmp --working-directory=$ABS_WT/bridges/xml ./gradlew test --no-daemon`
   If the gradle wrapper needs a network download and the sandbox denies
   it, report `java-tests-blocked: <error verbatim>` and STOP — do not
   skip silently.
2. Record the test summary line (tests run / failed / skipped) in the
   task report.
3. No commits expected (no file changes); if any file changed, STOP and
   report.

**Tests:**
- `./gradlew test --no-daemon` exits 0.

**Acceptance:**
- Gradle test task exit 0 with the summary recorded in the task report.
- `git -C <wt> status --short` empty after the task.

- [x] 3.2 <!-- verification-only: gradle test 77 run / 0 failed; no file changes -->
