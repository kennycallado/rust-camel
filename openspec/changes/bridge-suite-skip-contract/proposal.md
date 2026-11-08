# Proposal: bridge-suite-skip-contract

Affected crates: `camel-test` (dev-dependencies and tests only). bd: rc-uu7ax.

## Why

Local gate batteries fail six bridge integration targets when the native
bridge binaries are absent (`bridges/*/build/native/*-bridge`): 82 tests
panic instead of skipping. The archived change
`2026-11-01-bridge-test-stability` established the env contract — unset
`CAMEL_XML_BRIDGE_BINARY_PATH` prints one SKIP line and skips — but only
`mtls_bridge_test` uses it. CI is unaffected: it builds each bridge and
exports the path before running the suites (`ci.yml:143-148` "Build XML
bridge binary" + "XML bridge integration tests" with
`CAMEL_XML_BRIDGE_BINARY_PATH` at `ci.yml:147`; `ci.yml:153-158` "Build CXF
bridge binary" + "CXF bridge integration tests" with
`CAMEL_CXF_BRIDGE_BINARY_PATH` at `ci.yml:157`), so the skip branch is
unreachable there.

Evidence — each target run with all four env vars unset, base `18eea30e0`:

| Target | Unset + absent today | Classification | Action |
|---|---|---|---|
| cxf_test | rc=101, 40 failed, panic `support/cxf.rs:133` (`require_cxf_bridge_binary`) | fails on missing binary | suite gate reusing `resolve_cxf_bridge_binary()` |
| xml_bridge_charset_probe_test | rc=101, 8 failed, panic `support/xml_bridge.rs:120` (`require_worktree_native_bridge`) | fails on missing binary | suite gate |
| xml_bridge_validator_test | rc=101, 16 failed, panic `support/xml_bridge.rs:120` | fails on missing binary | suite gate |
| xml_bridge_xerces_diagnostics_test | rc=101, 3 failed, panic `support/xml_bridge.rs:120` | fails on missing binary | suite gate |
| xml_bridge_xj_test | rc=101, 12 failed, panic `support/xml_bridge.rs:44` (`require_xml_bridge_binary`) | fails on missing binary | suite gate |
| xml_bridge_xslt_test | rc=101, 3 failed, panic `support/xml_bridge.rs:44` | fails on missing binary | suite gate |

Every one of the 82 observed failures terminates at these missing-binary
checks; the early panics establish the failure cause for this host state,
not correctness beyond them. Fixture-only tests (22 in cxf_test, 19 in
each xml suite) pass today and make rc=101 look "partial".

Finding (reported, not fixed here): `xml_bridge_charset_probe_test` runs in
no CI workflow (grep across `.github/`); it is local-only. The skip
contract gives it a loud local skip. Adding CI coverage for it is out of
scope; file a follow-up if wanted.

## What Changes

- Skip rules (gate in new `crates/camel-test/tests/support/bridge_skip.rs`,
  driven pre-main by `ctor`, a new dev-dependency of `camel-test` only):
  - XML suites (five): skip only when `CAMEL_XML_BRIDGE_BINARY_PATH` is
    absent (`var_os`) AND the worktree-native binary
    (`bridges/xml/build/native/xml-bridge`) is absent → print exactly one
    `SKIP <suite>: <reason>. Provision with \`cargo xtask build-xml-bridge\`
    and set <ENV>=…` line, then exit 0 with zero cases run.
  - cxf_test: skip only when `CAMEL_CXF_BRIDGE_BINARY_PATH` AND
    `CXF_BRIDGE_PATH` are both absent (`var_os`) AND the native
    `bridges/cxf/build/native/cxf-bridge` is absent → same one-line SKIP +
    exit 0. Presence, not validity, is what suppresses the skip.
  - cxf selection tightening (the one deliberate exception to "runs
    unchanged", test-only): an explicitly configured selector that is set
    but invalid — empty, nonexistent, or non-Unicode — is a hard failure
    naming the offending value; it never falls back to the alias or the
    native binary, so a misconfiguration cannot be masked by a valid
    fallback. A valid alias-only configuration keeps working via
    `resolve_cxf_bridge_binary()`.
- When a selector is set to a valid path, the suite runs unchanged;
  a missing or broken binary at that path stays a hard failure. Existing
  `require_*` panics keep their semantics, including the deliberate
  fail-closed `require_worktree_native_bridge` contract of the schema-set
  suites when the env is unset but the native build exists.
- `require_xml_bridge_binary`'s panic message additionally names the
  offending path value (today it names only the variable), matching the
  message style of `require_worktree_native_bridge` and
  `xml_bridge_binary_or_skip`.
- No `#[ignore]`, no `--skip`, no silent `Ok(())` without the SKIP line.
  No runtime code, no CI workflow edits, no JMS suite changes (own
  contract, untouched).

## Acceptance criteria

- All six targets with the relevant selectors unset and the native
  binaries absent: exactly one actionable SKIP line each, exit 0, zero
  executed cases in those suites.
- All five XML targets with the env unset but the native binary present:
  hard failure without SKIP (fail-closed contract retained).
- cxf with a valid native auto-detect, and cxf with a valid alias-only
  configuration: the real cases run.
- Invalid explicit configuration never skips — including cxf with a valid
  native fallback available: hard failure naming the offending value.
- A set-but-nonexistent path is a hard failure naming the path — no skip.
  `xml_bridge_xslt_test` with `CAMEL_XML_BRIDGE_BINARY_PATH` pointing at a
  nonexistent file must fail naming that file.
- A broken existing executable stays a hard failure (the gate never fires
  when a selector is set; the suite runs and the cases fail).
- `xml_bridge_xslt_test` with a real built binary: the real cases run and
  pass (proves the skip did not eat the suite).
- Gates: `cargo fmt --check --all`; `cargo clippy -p camel-test
  --all-targets --features integration-tests -- -D warnings`.

## Risk budget

- Test infrastructure only (`crates/camel-test/tests/**`, dev-deps). No
  runtime behaviour change; `bridges/*/build/` stays out of git.
- The pre-main `exit(0)` fires only when the suite provably cannot run; a
  set-but-broken path never reaches it.
