# guard-mutation-hardening Specification

## Purpose
TBD - created by archiving change mutation-hardening. Update Purpose after archive.
## Requirements
### Requirement: SSRF IP-guard closure is mutation-hardened

The IPv4-mapped closure of `is_ssrf_blocked_ip` in
crates/camel-api/src/ssrf.rs SHALL be covered by `::ffff:`-wrapped mirror
tests that assert the blocked/unblocked verdict for embedded forms of each
range boundary (private, loopback, link-local, multicast, 0/8, CGNAT
in-window and out-of-window corners, benchmark pair, reserved, and their
public near-misses), such that every boolean/comparison mutation inside the
closure is killed by `cargo xtask mutants --file
crates/camel-api/src/ssrf.rs`, with exactly one documented exception: the
NAT64 `>>`→`<<` equivalent mutant in `nat64_embedded_blocked`, which no
input can distinguish because the mutation zeroes an octet no IPv4 predicate
reads.

#### Scenario: CGNAT mirror corners kill range mutations

- **GIVEN** `::ffff:100.63.0.1`, `::ffff:100.64.0.1`,
  `::ffff:100.127.0.1`, `::ffff:100.128.0.1`
- **WHEN** `is_ssrf_blocked_ip` classifies each
- **THEN** the verdicts are false, true, true, false respectively, so
  `>=`/`<=` boundary mutations in the closure's CGNAT window flip at least
  one verdict

#### Scenario: Off-guard mirror near-misses kill equality mutations

- **GIVEN** `::ffff:8.64.0.1` and `::ffff:199.18.0.1` (second octet
  in-window but first octet outside the guarded 100/198)
- **WHEN** `is_ssrf_blocked_ip` classifies each
- **THEN** both verdicts are false, so an `==`→`!=` mutation on the closure's
  first-octet guards changes at least one verdict

#### Scenario: One-arm mirror addresses kill OR-to-AND mutations

- **GIVEN** `::ffff:`-wrapped addresses satisfying exactly one closure arm
  (`::ffff:0.1.2.3` for 0/8, `::ffff:240.0.0.1` for reserved,
  `::ffff:224.0.0.1` for multicast, `::ffff:169.254.1.1` for link-local)
- **WHEN** `is_ssrf_blocked_ip` classifies each
- **THEN** every verdict is true, so any `||`→`&&` mutation between that arm
  and another false arm changes at least one verdict

#### Scenario: Public mirror addresses stay unblocked

- **GIVEN** `::ffff:8.8.8.8` and `::ffff:223.255.255.255` (the public
  near-miss of multicast 224.0.0.0/4 and reserved 240.0.0.0/4)
- **WHEN** `is_ssrf_blocked_ip` classifies each
- **THEN** both verdicts are false, and no mirror test anywhere asserts
  239.255.255.255 (multicast, blocked) as public

#### Scenario: NAT64 byte order is pinned as a regression, not a kill

- **GIVEN** `64:ff9b::0a00:0001` (embedded 10.0.0.1) and
  `64:ff9b::0808:0808` (embedded 8.8.8.8)
- **WHEN** `nat64_embedded_blocked` classifies each
- **THEN** the verdicts are true and false respectively; the surviving
  `>>`→`<<` mutation at the s[7] extraction site is a documented equivalent
  mutant (zeroes `octets[2]`, read by no predicate), and the acceptance
  records exactly one surviving mutation in this file

### Requirement: Broker URL redaction output is asserted exactly

The `redact_broker_url` function in crates/components/camel-jms/src/config.rs
SHALL be covered by `assert_eq!` tests on the complete redacted string for
the userinfo-mask shape, the multi-parameter sensitive-plus-benign query
shape, the bare-`@`-without-scheme passthrough shape, and the clean-URL
passthrough shape, such that no index-arithmetic mutation inside the
function survives `cargo xtask mutants --file
crates/components/camel-jms/src/config.rs`.

#### Scenario: Userinfo mask is byte-exact

- **GIVEN** `tcp://admin:secretpass@broker.example.com:61616`
- **WHEN** `redact_broker_url` processes it
- **THEN** the result equals exactly `tcp://***@broker.example.com:61616`

#### Scenario: Query join and per-key replacement are byte-exact

- **GIVEN** `tcp://host:61616?password=p&user=u&keepAlive=true`
- **WHEN** `redact_broker_url` processes it
- **THEN** the result equals exactly
  `tcp://host:61616?password=<redacted>&user=<redacted>&keepAlive=true`

#### Scenario: Bare at-sign without scheme passes through unchanged

- **GIVEN** `admin@host` (no `://` before the `@`)
- **WHEN** `redact_broker_url` processes it
- **THEN** the result equals exactly `admin@host`

### Requirement: Guards' production code is unchanged

This change SHALL NOT modify any non-test code in ssrf.rs or config.rs; the
guards are correct as implemented and only their test discrimination is
hardened.

#### Scenario: Diff contains no production changes

- **GIVEN** the completed change
- **WHEN** the diff over crates/camel-api/src/ssrf.rs and
  crates/components/camel-jms/src/config.rs is inspected
- **THEN** every changed or added line lies inside the `#[cfg(test)]` test
  modules of those files

