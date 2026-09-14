# Design: redactfix

## Approach

Fail-closed hardening of the parse-failure arm of
`redact_url_for_diagnostics` (camel-http), per the mission pre-flight
ruling (e_glm, 2026-09-14): **detection-then-suppress**, not surgical
scrubbing. A surgical scrub must be right about every obfuscation to avoid
false negatives; suppression only needs a broad detector because leak cost
far exceeds false-positive cost.

Algorithm for the `Err(_)` arm, in order:

1. **Authority window:** the substring beginning immediately after the
   first `//` (scheme-bearing OR protocol-relative — a `://`-only anchor
   false-negatives on `//u:p@host/`) and ending at the next `/`, `?`, or
   `#` searched from that offset. If that window contains `@`, return the
   constant sentinel `[redacted]` immediately. Grammar
   fact: userinfo cannot exist without an `//` authority, so `mailto:` and
   path-`@` strings correctly stay visible.
2. **Query redaction:** else, if the remaining string contains `?`, cut at
   the first `?` and append `?[redacted]` — BEFORE truncation; a
   truncate-first order leaks query bytes on ≤256-byte inputs. When the
   pre-query text alone exceeds the 256-byte cap, the cap governs: no
   query byte survives either way, and the suffix appears only when it
   fits within the cap.
3. **Char-boundary truncate:** cap at 256 bytes floored to a UTF-8 char
   boundary with a hand-rolled loop (`floor_char_boundary` is unstable;
   MSRV trap). Applied in BOTH arms; the Ok arm is behavior-identical
   because `url` serialization is ASCII.

Parse-success arm: byte-identical to today (rc-u4jk6 masking condition,
query drop, golden tests). Suppression wins over query-redaction when both
trigger (the sentinel is the whole output).

The sentinel is the literal `[redacted]` — never empty (ambiguous in logs).
The sentinel shape is distinct from the Ok arm's `***@` masking by design:
an unparseable authority cannot be safely re-rendered, so nothing of it is
rendered at all.

## Affected crates

- camel-component-http: `redact_url_for_diagnostics` Err arm + truncation
  helper; unit tests in lib.rs tests module; fence-path contract test
  (CamelHttpUri override fence rejection, message-level assertion only —
  no fixture network).

## Architecture boundaries

Component layer only (camel-component-http, pub(crate) helper). No Runtime,
DSL, Services, or Functions surface changes; no public API change. Honors
the data/control plane boundary (diagnostics-only path). Aligns with
ADR-0071 (allowedUriHosts fence scope). Reachability note: the named
reachable consumer of the parse-failure arm is the `CamelHttpUri` override
fence rejection on the resolve path (lib.rs), which evaluates the RAW
header string — `uri_host_allowed` rejects unparseable inputs with
`Ok(false)`, so the fence error renders them through
`redact_url_for_diagnostics`. The per-hop redirect fence (ssrf.rs) also
routes through this helper but consumes re-serialized `url::Url` values,
which always re-parse; it inherits the hardening without exercising the
failure arm. camel-dsl and camel-cli are BUSY fleet zones: zero diffs
there.

## Phases (optional)

Single-phase change — omitted. One coherent slice (one function's Err arm
plus tests); no milestone grouping.

## Alternatives considered

- **Surgical userinfo scrub** (strip `://…@` → `***@`): rejected — must be
  right about every malformed-authority obfuscation; one miss re-opens the
  leak. Spec pins absence-of-secret contracts, not replacement shapes.
- **Suppress ALL unparseable strings**: rejected — destroys diagnostics
  utility for credential-free strings (e.g. the pinned 1000-char truncation
  test) without a leak rationale; the authority-window detector already
  separates the two populations.
- **Re-parse after stripping userinfo**: rejected — second-guessing the
  parser on attacker-controlled input re-imports the false-negative class
  suppression exists to avoid.
