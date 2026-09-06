# Design: mutation-hardening

## Approach

Pure test additions. No production line changes; the two guards are correct —
their tests fail to discriminate. The kill strategy is derived mutation-by-
mutation from the cargo-mutates survivor classes recorded in bd rc-tb93 /
rc-kkwh.

### Survivor locality (why the matrix targets the closure)

All 21 `is_ssrf_blocked_ip` survivors sit inside the **IPv4-mapped closure**
(ssrf.rs:102–116): the closure's own OR-chain of v4 predicates is what the
operator counts (10 `||`, 5 `==`, 3 `&&`, 2 `>=`, 1 `<=`) match. The main
v4/v6 arms are already covered: ssrf.rs:211 (0/8 `0.1.2.3`), :222–223 (CGNAT
corners), :247 (reserved/multicast), :342 (NAT64 s[6] extraction),
:346–352 (6to4/teredo prefix). camel-http's `::ffff:` tests (lib.rs:7388+)
live in another package and are invisible to camel-api's package-scoped
mutants run — hence the closure is the untested surface.

### camel-api ssrf.rs — IPv4-mapped mirror matrix

Wrap every boundary case in `::ffff:` and assert the verdict; the closure
runs the full v4 predicate set on the embedded address, so each mirror assert
exercises the closure's arms one at a time.

Blocked (verdict `true`):
`::ffff:10.0.0.1` (private), `::ffff:127.0.0.1` (loopback),
`::ffff:169.254.1.1` (link-local/metadata), `::ffff:224.0.0.1`
(multicast), `::ffff:0.1.2.3` (0/8), `::ffff:100.64.0.1` /
`::ffff:100.127.0.1` (CGNAT corners in-window),
`::ffff:198.18.0.1` / `::ffff:198.19.255.254` (benchmark pair),
`::ffff:240.0.0.1` (reserved).

Public (verdict `false`):
`::ffff:8.8.8.8`, `::ffff:8.64.0.1` (kills `==`→`!=` on the 100 guard:
second octet in-window, first octet off-guard),
`::ffff:100.63.0.1` / `::ffff:100.128.0.1` (CGNAT corners out-of-window —
kill the `>=`/`<=` mutants), `::ffff:199.18.0.1` (off-guard near-miss for
the 198 guard), `::ffff:223.255.255.255` (public near-miss of both
multicast 224/4 and reserved 240/4 — 239.255.255.255 is multicast → blocked,
NOT public).

The plain (unwrapped) forms of the four CGNAT/one-arm cases stay as
regression pins alongside the existing tests (redundant for kill purposes —
ssrf.rs:211/222/247 already kill the main-arm mutants — but they document
the matrix intent in-place).

### NAT64: one documented equivalent mutant

`nat64_embedded_blocked` (ssrf.rs:136) extracts the embedded v4 with
`(s[6] >> 8) as u8, s[6] as u8, (s[7] >> 8) as u8, s[7] as u8`. The
surviving `>>`→`<<` mutation is at the **s[7] site** (the s[6] site is
already killed by the test at ssrf.rs:342). Under the mutant,
`(x << 8) as u8` is always `0x00`: it zeroes `octets[2]`. No IPv4 predicate
in `is_ssrf_blocked_ip` depends on `octets[2]` (all guards read
`octets[0]`, `octets[1]`, or the std helpers, which see only the mutated
third octet — 10.0.0.1 → 10.0.0.1-class, 8.8.8.8 → 8.8.0.8-class, both
public). Therefore no input distinguishes mutant from original: an
equivalent mutant. The two NAT64 pins already exist (ssrf.rs:332/:340:
`64:ff9b::0a00:0001` embedded 10.0.0.1 → blocked, `64:ff9b::0808:0808`
embedded 8.8.8.8 → public) and are retained unchanged — they protect the
byte-order semantics against future predicate changes, but the acceptance
records **21 of 22 killed; 1 equivalent
survivor**. Annotate the equivalent mutant in rc-tb93 notes at delivery.

### camel-jms config.rs — exact-output redaction

`redact_broker_url` (config.rs:400): the 2 survivors are index-arithmetic
mutations on `scheme_end` (`idx + 3` at config.rs:416), not concat changes —
under a mutant, the mask boundary shifts and the substring assertions still
pass. Kill with `assert_eq!` on the FULL string for five shapes:

1. userinfo: `tcp://admin:secretpass@broker.example.com:61616` →
   `tcp://***@broker.example.com:61616` (a shifted mask boundary produces a
   different exact string).
2. multi-param query with sensitive + benign mix:
   `tcp://host:61616?password=p&user=u&keepAlive=true` →
   `tcp://host:61616?password=<redacted>&user=<redacted>&keepAlive=true`.
3. no-query clean URL passthrough (exact `tcp://host:61616`).
4. bare `@` without `://` (`admin@host`) → unchanged (pins the `None` arm).
5. failover composite: exact substrings with delimiters
   (`jms.userName=<redacted>`, `jms.password=<redacted>`,
   `keepAlive=true`).

New `#[test]` fns in the existing test module; the legacy substring test
stays (documents intent) — the exact-output tests add discrimination.

### Verification

`cargo xtask mutants --file crates/camel-api/src/ssrf.rs` (expect exactly 1
survivor: the documented NAT64 equivalent) and `--file
crates/components/camel-jms/src/config.rs` (expect 0) — cargo-mutates
27.1.0 via the xtask wrapper, scoped single-file `--no-config`. Mutants
runs are informational (never a CI gate); acceptance is recorded in this
change, not added to AGENTS.md.

## Affected crates

- `crates/camel-api`: test additions in `src/ssrf.rs` (no src changes).
- `crates/components/camel-jms`: test additions in `src/config.rs` (no src
  changes).

## Architecture boundaries

Test-only; no Runtime/DSL/Component behavioral surface changes. camel-api
owns the SSRF classification contract (ADR-0032 trust boundary); camel-jms
owns broker-credential redaction. Both stay byte-identical in their
production code.

## Alternatives considered

- **Refactoring to kill the equivalent mutant** (e.g. range tables, or
  asserting on the extracted octets directly) — rejected: changes production
  code under a security charter without a behavioral defect, and the spec
  pins production-code-unchanged.
- **Property-based testing (proptest) over random IPs** — kills mutants
  statistically but obscures which boundary is pinned; the deterministic
  mirror matrix is auditable against the RFC list.
- **Re-running camel-http's `::ffff:` tests inside camel-api** — duplicate
  coverage across packages; the mirror matrix achieves the same kill within
  the owning package.

Single-phase change: two independent test-addition slices, one review pass.
