# ADR-0079: MCP allow_internal — align IP-literal cleartext rule with camel-http

- Status: Accepted (decided 2026-09-17: owner delegated to e_gpt, ruling
  certainty HIGH — exact alignment; implemented same day, mission 117
  phase 2)
- Source: bd rc-lztp2, filed from r_glm holistic + e_glm concurrence on mission 112
- Provenance: literal rule is F2-4 (audit 2026-08-31); hostname DNS-pinning
  landed in 6c4b81a6 (rc-juqrd, 2026-09-17). Git evidence before the
  2026-09-17 history re-root is unrecoverable
- Companion draft ADR-0080 (codify asymmetry) was WITHDRAWN and deleted;
  its strongest case is preserved in Rejected Alternatives
- Citation note: file:line references to config.rs describe the
  PRE-ADR-0079 state; the implementing commit (mission 117 phase 2) is
  the authoritative post-change source (rule now at `config.rs:379`)

## Context

The MCP component validates remotes on two paths. IP-literal remotes are
validated once at config load by `McpRemoteConfig::validate_url`
(`crates/components/camel-component-mcp/src/config.rs:335`, called from
`component.rs:142`). Hostname remotes pass that layer untouched and are
validated again at connect time by `adapter::dns_pin`
(`crates/components/camel-component-mcp/src/adapter/dns_pin.rs:154`),
which resolves once, checks every address, and pins the reqwest client to
the validated addresses.

Both paths share the blocked-IP classifier
`camel_api::is_ssrf_blocked_ip` (`crates/camel-api/src/ssrf.rs:60`).
"Blocked" covers loopback, RFC1918 private, link-local, cloud-metadata
(169.254/16), CGNAT, broadcast, multicast, unspecified, and more.

The blocked-target policy agrees everywhere. The divergence is the
cleartext rule for PUBLIC (non-blocked) targets over `http://`:

| Path (cleartext http:// to a PUBLIC target) | allow_internal=false | allow_internal=true |
|----------------------------------------------|----------------------|---------------------|
| camel-http, IP literal (`ssrf.rs:85-90,101-106`) | allow | REJECT |
| camel-http, hostname (`ssrf.rs:299-307,234-242`) | allow | REJECT |
| MCP, hostname (`dns_pin.rs:135-144`)           | allow | REJECT |
| MCP, IP literal (`config.rs:373-377`)          | REJECT | **allow** |

The MCP literal rule is `!blocked && http && !allow_internal` → reject
(`config.rs:373`). camel-http uses `allow_internal && !blocked && http`
→ reject (`crates/components/camel-http/src/ssrf.rs:85`). The
`allow_internal` conjunct is inverted between the two. The MCP-literal
row is therefore inverted in both columns: it rejects where every other
path allows (default policy), and allows where every other path rejects
(opt-in policy).

Two more facts:

1. The comment above the MCP rule claims it "mirrors the http component's
   no-cleartext rule" (`config.rs:371-372`). It does not: the conjunct is
   inverted. The "mirrors" comment plus the inversion together are the
   intent evidence: the rule was written as a parity transcription and
   lost the flag polarity. Both rules first appear together at the
   2026-09-17 history re-root, so git chronology cannot corroborate or
   refute this; the in-code evidence stands on its own.
2. The F2-4 doc comment on the field (`config.rs:320-327`) says cleartext
   is allowed when "the target is internal or this flag is set". So the
   flag has meant "operator accepts any transport" for literals since
   F2-4, while rc-juqrd gave hostnames the opposite meaning
   ("internal-network posture; cleartext never to public") one day later
   (6c4b81a6, 2026-09-17).

## Decision (Option A — align, exact parity)

Adopt the camel-http semantics. Change the literal-branch conjunct at
`config.rs:373` from `!self.allow_internal` to `self.allow_internal`,
mirroring `camel-http/src/ssrf.rs:85`. Update the field doc comment
(`config.rs:320-327`) and the rule comment (`config.rs:371-372`).

Under exact parity the matrix row for MCP literals becomes
`allow | REJECT`, matching the other three rows:

- `allow_internal=true` + `http://` + public literal: now REJECTED at
  config load. Remedy: use `https://`.
- `allow_internal=false` + `http://` + public literal: now ALLOWED,
  matching camel-http's default (plain cleartext HTTP to a public target
  is legitimate egress).

Implemented by mission 117 phase 2 (e_gpt ruling 2026-09-17). The
characterization pins flip with the rule:
`remote_url_rejects_public_cleartext_literal_when_allow_internal`
(renamed from the old `...allows...` pin),
`remote_url_hostname_cleartext_passes_config_layer` (behavior
unchanged), and the pre-existing default-row test became
`remote_url_permits_cleartext_public_ip_by_default`.

## Rejected Alternatives

**A-strict (reject public cleartext under BOTH polarities).** This
variant closes the same unsafe cell with zero loosening: keep the
F2-4 default rejection AND reject under `allow_internal=true`. It was
rejected because it leaves a permanent, fail-closed divergence from
camel-http (MCP stricter by default) and the ruling selected exact
alignment. Recorded here so future maintainers know the stricter
default was considered and consciously traded for parity.

**Option B — codify the asymmetry (draft ADR-0080, withdrawn).** Its
strongest case: a literal remote is operator-pinned BY VALUE. The
operator typed the exact address; there is no name resolution and no
DNS delegation, so `allow_internal=true` on a literal reads as "I take
responsibility for this exact address and its transport". The pinning
pipeline exists precisely because hostnames delegate addressing to
DNS; literals opt out by nature (`dns_pin.rs:160-166`). F2-4 also
documented this meaning first ("the target is internal or this flag is
set"). Rejected because: the flag then keeps two opposite meanings by
URL spelling, the control-plane-writable public-cleartext cell stays
open (a config write of literal-plus-flag opens a cleartext MCP
channel to any public IP), and every future review re-flags the
divergence. One flag must mean one thing.

**Dedicated `allow_cleartext` schema flag.** Separates internal-target
permission from transport permission cleanly, at the price of new
config surface, a migration path, and a second flag for operators to
mis-set. Not taken with this ADR; remains available if the overloaded
flag causes operator confusion in practice.

## Comparison

| Criterion | A: align (chosen) | B: codify asymmetry (rejected) |
|---|---|---|
| One flag, one meaning | Yes: `allow_internal=true` = internal-network posture everywhere | No: literal = "trust this remote fully"; hostname = "internal posture" |
| Parity with camel-http | Exact | Permanent divergence, documented |
| Default (`false`) public cleartext | Allowed (same as camel-http default egress) | Rejected (stricter than hostname path: inconsistent spelling) |
| Unsafe cell (true + public cleartext literal) | Closed | Open, accepted |
| Breaking risk | `true` + `http://` + public-literal configs stop loading (mechanical fix) | None |
| New default egress | Exact form permits public cleartext literals by default | None |
| Review noise | Ends re-flagging | Requires every future reviewer to find this ADR |
| Escape-hatch trap (see Consequences) | Removed | Remains, documented |

## Consequences

**Breaking (exact form):** configs with `allow_internal=true` +
`http://` + public IP literal fail config load with "use https://".
Fix is mechanical. Configs with the flag unset are unaffected or newly
allowed; none break.

**Operator trap removed:** today the default rejection says "use https://"
(`config.rs:374-376`) while the nearby blocked-IP rejection says "set
allow_internal=true to override" (`config.rs:367-369`). An operator who
applies that advice to a public cleartext rejection silently downgrades
to cleartext egress to the public internet. Option A makes the flag stop
granting that power.

**Deployment examples (post-ADR-0079 behavior):**

- Loopback server `http://127.0.0.1:8000` + `allow_internal=true`:
  unchanged, allowed (blocked target, flag set).
- Same server as `http://localhost:8000` + `allow_internal=true`:
  unchanged, allowed (all resolved addresses internal).
- Public server `http://93.184.216.34` + `allow_internal=true`: REJECTED
  at load (before ADR-0079: allowed). Use `https://`.
- Same server as `http://mcp.example.com` + `allow_internal=true`:
  unchanged, already rejected at connect (`dns_pin.rs:135`).
- Public cleartext lab server `http://93.184.216.34`, no flag: allowed
  (camel-http default egress parity; before ADR-0079: rejected).

## Recommendation (mission 117, advisory — upheld by ruling)

Recommend **Option A**. Reasoning:

1. The "mirrors" comment beside an inverted conjunct shows the literal
   rule intended parity and lost it in transcription. Aligning restores
   the stated intent rather than inventing new policy.
2. `allow_internal=true` must not mean two opposite things depending on
   URL spelling of the same server. Operators cannot build a correct
   mental model, and reviewers will keep re-flagging the divergence.
3. The unsafe cell is reachable through the flag operators set when they
   want to be MORE permissive about internal targets. That flag should
   never widen cleartext egress to the public internet.
4. The hostname path already chose camel-http parity (6c4b81a6). The
   literal branch is the sole outlier.

The owner delegated the decision to e_gpt; the ruling (certainty HIGH,
2026-09-17) selected exact alignment.

## References

- bd rc-lztp2 (this decision), rc-juqrd (hostname pinning, closed)
- History re-root 2026-09-17 (pre-re-root chronology unrecoverable);
  6c4b81a6 (DNS pinning, post-re-root)
- ADR-0078 (audit-family residual decisions)
- Post-change pins: `camel-component-mcp/src/config.rs` tests
  `remote_url_rejects_public_cleartext_literal_when_allow_internal`,
  `remote_url_hostname_cleartext_passes_config_layer`,
  `remote_url_permits_cleartext_public_ip_by_default`;
  `adapter/dns_pin.rs` test `resolution_public_over_http_rejected_when_allow_internal`
