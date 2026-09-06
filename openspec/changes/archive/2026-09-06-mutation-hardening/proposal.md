# Proposal: mutation-hardening

## Why

A preliminary cargo-mutates run (bd rc-eba8, kill-rate 80.4%) left surviving
mutations in two security guards. A mutation that survives means the guard's
tests cannot tell correct code from broken code — for a security control this
is silent-failure territory:

- bd rc-tb93: 22 survivors in crates/camel-api/src/ssrf.rs — 21 inside the
  IPv4-mapped closure of `is_ssrf_blocked_ip` (lines ~102–116: 10× `||`→`&&`,
  5× `==`→`!=`, 3× `&&`→`||`, 2× `>=`→`<`, 1× `<=`→`>`), plus 1 in
  `nat64_embedded_blocked` (`>>`→`<<`, s[7] site — an equivalent mutant: the
  shifted byte zeroes `octets[2]`, which no IPv4 predicate depends on, so no
  input can distinguish mutant from original). The main-arm v4/v6 mutants are
  already killed by existing tests (ssrf.rs:211/222/247, 342, 346–352); the
  closure is the untested surface, because camel-api's package-scoped run
  cannot see camel-http's `::ffff:` tests.
- bd rc-kkwh: 2 survivors in `redact_broker_url`
  (crates/components/camel-jms/src/config.rs, index arithmetic on
  `scheme_end` at line 416). The existing test asserts substrings instead of
  exact output, so a mangled mask still passes — broker credentials could
  reach logs mis-redacted without any test failing.

## What Changes

- An IPv4-mapped mirror matrix for the closure: `::ffff:`-wrapped forms of
  every boundary case (blocked: private/loopback/link-local/multicast/
  0/8/CGNAT corners/benchmark pair/reserved; public: 8.8.8.8, 8.64.0.1,
  100.63.0.1, 100.128.0.1, 199.18.0.1) — verified to kill all 21 closure
  mutants.
- Two NAT64 byte-order regression pins (embedded 10.0.0.1 → blocked,
  embedded 8.8.8.8 → public), kept as regression pins only — the `>>`→`<<`
  survivor is a documented equivalent mutant (see design), NOT credited as
  killed.
- Exact-output (`assert_eq!`) tests for `redact_broker_url` covering the
  userinfo mask, sensitive/benign query mix, the multi-parameter `&` join,
  the bare-`@`-without-scheme passthrough, and the clean-URL passthrough.
- No production code changes: the guards are correct; their tests are not.

Explicitly excluded: raising the workspace-wide kill-rate (rc-eba8 epic),
mutation-testing in CI gates (informational per xtask), any behavioral change
or refactor of the guards themselves (the only cure for the equivalent
mutant, rejected as out of scope for a security charter with no behavioral
defect).

## Acceptance criteria

- `cargo xtask mutants --file crates/camel-api/src/ssrf.rs` reports exactly
  1 surviving mutation — the documented NAT64 equivalent mutant
  (21 of 22 killed).
- `cargo xtask mutants --file crates/components/camel-jms/src/config.rs`
  reports 0 surviving mutations **in `redact_broker_url`** (function-scoped
  per the spec: `--json | jq -c 'select(.function=="redact_broker_url")' |
  wc -l` = 0; the file's 8 pre-existing survivors in unrelated functions are
  filed as bd rc-isxj0, out of scope).
- `cargo test -p camel-api --lib ssrf` and `cargo test -p
  camel-component-jms --lib config` pass; fmt/clippy green.

## Risk budget

Test-only change in two crates; runtime risk zero. Main risk is
boundary-matrix tests that encode WRONG expectations — mitigated by deriving
every case from the function's documented RFC ranges (RFC 1918, 5771
multicast, 6598, 2544, 1112, 3879, 6052) and cross-checking each verdict
against the classifier's semantics before writing the assert (e.g.
239.255.255.255 is multicast → blocked; its public near-miss is
223.255.255.255).
