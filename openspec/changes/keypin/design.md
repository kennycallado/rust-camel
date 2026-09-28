# Design: keypin

## Approach

**Decision 1 — Truststore format.** A single line-oriented text file,
deployment-owned. Each non-empty, non-comment line pins one verifying
key: `blake3:<64 lowercase hex>` (the manifest `key_fingerprint` form,
`signature.rs:161`), optionally followed by whitespace and a decimal
u64 freshness floor. Blank lines and `#` comments are ignored; a
directory is not supported. Parse fails closed with step
`truststore-parse`: a file-level failure (unreadable, or not UTF-8)
exits 2 naming the path; a line-level failure (malformed entry,
duplicate pin) exits 2 naming the path and the line. An empty
truststore — zero bytes or only blank and comment lines — is valid and
pins nothing: every signed artifact fails its pin check. The floor
column is runtime-maintained (boot) and operator-writable; the runtime
never lowers it.

**Decision 2 — Supply surface.** The truststore reaches both verify
sites — boot (`verify_for_boot`, `runtime.rs:951`) and `--verify`
(`run_verify_only`, `runtime.rs:905`) — through two surfaces, mirroring
the compile-side signing-key pattern (`commands/compile.rs:57,203-208`):
the sanctioned artifact argument `--truststore <PATH>` and the
`CAMEL_TRUSTSTORE` environment variable; the argument wins. The artifact
argument surface grows by this one sanctioned argument (ADR-0083:142-143;
the spec delta adds it). `--truststore` is a modifier, not a mode: it
pairs with a boot (with or without `--report`) and with `--verify`; it
is rejected with `--help`, `--version`, or `--manifest` (exit 2,
`Exclusive`); a duplicate or missing value exits 2. The runtime has no
clean-environment guard (unlike the compile side,
`commands/compile.rs:162-188`); `CAMEL_TRUSTSTORE` is read whenever a
truststore policy could apply. **Strip rule:** when a truststore is
supplied, an artifact whose manifest carries a signing block (schema 4
or 5) with an ABSENT envelope exits 2 with step `truststore-pin`,
naming the manifest fingerprint — deleting the `.sig` file must not
convert a signed artifact into an unsigned one under pin policy; the
envelope is the only carrier of signature proof. The compile
side carves out `CAMEL_TRUSTSTORE` as a verify-side input never read at
compile time: its presence is benign and does not reject (unlike the
stray `CAMEL_COMPILE_SIGNING_KEY`, a compile input being ignored).

**Decision 3 — Freshness carrier.** The marker lives in the manifest,
not the envelope: the envelope is 148 bytes and format-frozen
(`signature.rs:60`; ADR-0083:28-39), and the manifest bytes are inside
the signed byte domain — the manifest is inside the trailer
(`trailer.rs:472`) and the signature covers the complete final artifact
bytes (`runtime.rs:882-887`). A signed compile emits manifest schema 5:
schema 4 plus a mandatory `freshness` field in the signing block,
encoded as a u64 unix-seconds JSON integer. A new schema number, not an
optional field in schema 4, because the repo's strictness pattern
rejects unknown fields without a schema bump (`manifest.rs:405-407,
712-716, 786-790`). Readers accept schemas 2, 3, 4, and 5
(`manifest.rs:280-288`); schema 5 without a valid freshness marker is
rejected, mirroring schema 4's mandatory signing block (ADR-0083:75-76).
u64 unix-seconds: stateless at compile time (no producer-side counter
file to lose or race), monotonic in practice, JSON-native (the manifest
already carries u64 integers, `manifest.rs:148,209`), and exact in
serde_json. Clock skew fails closed, never open.

**Decision 4 — Freshness rule.** The truststore records, per pinned
key, the highest previously accepted marker (the floor). Verify
decision: marker >= floor → accept; marker < floor → exit 2, step
`freshness-rollback`; first sight of a pinned key with no recorded
floor → accept and record. **Legacy schema 4:** a pinned schema-4
artifact carries no marker and is treated as below any recorded floor:
it passes pin-only while NO floor is recorded for its key, and exits 2
`freshness-rollback` once one is. Rollback protection for a key
therefore engages when the first schema-5 artifact of that key boots;
that first boot is the operator's migration boundary, and it never
re-opens (the floor never decreases). **Resolution bound:** the marker
is u64 unix-seconds; rollback detection is bounded to artifacts older
than the floor by at least one second. Two artifacts compiled in the
same second are not orderable by the marker; producer clock skew
behind the floor fails closed, skew ahead can reject a later
correctly-clocked artifact (an availability cost, never a rollback
hole). **Serialization:** the floor check and update are one critical
section. Boot takes an exclusive advisory lock (flock) on the sibling
lockfile `<truststore>.lock` (created on demand), re-reads the floor
under the lock, decides, and — on accept — writes max(current, marker)
atomically via tmp+rename (the envelope-write precedent,
`commands/compile.rs:577-626`), releasing the lock after. Lock
acquisition failure fails closed. Both boot and `--verify` apply the
rollback decision on the floors read at decision time; only boot
records. `--verify` is a dry run: it reads floors without the lock and
never writes, so a read-only truststore stays valid for audit-only
deployments. A boot inside the critical section that cannot acquire
the lock, or that must record a floor and cannot write the
truststore, fails closed with exit 2, step `truststore-update`:
silently degrading to no-freshness is
exactly the rollback hole this change closes.
Documented limitation: a deployment that only runs `--verify` never
advances floors; rollback protection engages on boot.

**Decision 5 — Compat matrix.** See the two tables below. The
no-truststore table is exactly today's R4 behavior (ADR-0083:83-88).
The truststore does not apply to unsigned artifacts: pinning is about
which signing key, an unsigned artifact has none, and a deployment file
must not silently re-sign the world; unsigned compiles stay schema 3
and byte-identical (`commands/compile.rs:374-380`). A signed manifest
under a supplied truststore requires its envelope (strip rule,
Decision 2). A schema-4 (pre-keypin) signed artifact under a
truststore passes pin-only until a floor is recorded for its key, then
fails as a rollback (Decision 4): same-key replay of legacy pairs
cannot outlive the first schema-5 boot.

**No truststore (R4, unchanged):**

| artifact state | verdict |
|---|---|
| unsigned, no envelope (schema 3) | boot, no signature hashing (v1 compat) |
| unsigned, stray envelope (schema 3 + .sig) | exit 2, unpaired envelope |
| signed (4/5), envelope absent, required:false | boot, no signature hashing |
| signed (4/5), envelope absent, required:true | exit 2, missing required signature |
| signed (4/5), envelope present | verify chain; valid → boot / `--verify` exit 0; any step fails → exit 2 |

**Truststore present:**

| artifact state | verdict |
|---|---|
| unsigned, no envelope (schema 3) | same as R4 — truststore does not apply |
| unsigned, stray envelope (schema 3 + .sig) | same as R4 — exit 2, unpaired |
| signed (4/5), envelope absent (any required bit) | exit 2, `truststore-pin` (strip rule) |
| signed (4/5), envelope present, key not pinned | exit 2, `truststore-pin` |
| signed 5, pinned, marker >= floor | verify + pin + freshness; boot records floor under lock; `--verify` 0 (dry run) |
| signed 5, pinned, marker < floor | exit 2, `freshness-rollback` |
| signed 4, pinned, no floor recorded for the key | pin check only; boot / `--verify` exit 0 |
| signed 4, pinned, floor recorded for the key | exit 2, `freshness-rollback` (legacy below floor) |

**Decision 6 — Code placement.** New module `compile/trust.rs` for the
truststore (parse, pin set, floors, lockfile critical section, atomic
update) — a distinct concern
from the frozen envelope codec in `signature.rs`. New `TrustError`
taxonomy (variants name the step: `Unreadable`, `NotUtf8`,
`MalformedEntry { path, line }`, `DuplicatePin { path, line }`,
`LockFailure`, `Unwritable`, `Io`), separate from `EnvelopeError`
(`signature.rs:76`).
`signature.rs` is unchanged. `manifest.rs` gains `MANIFEST_SCHEMA_V5`,
the `freshness` field on `SigningBlock`, and schema-5 field checks.
`commands/compile.rs` sets schema 5 and the marker on `--sign` (the
single signing-block call site, `commands/compile.rs:377-380`) and
carves out `CAMEL_TRUSTSTORE` in the clean-environment guard.
`runtime.rs` grows `ArtifactArgs.truststore`, the parse rules, and a
trust-policy step after `verify_envelope_bytes` (`runtime.rs:858`): pin
check, then freshness check; boot records the floor. Diagnostics name
the failing step — `truststore-parse`, `truststore-pin`,
`freshness-rollback`, `truststore-update` — exit 2 class
(`runtime.rs:78`).

## Affected crates

- `camel-cli`: `src/compile/trust.rs` (new), `src/compile/manifest.rs`
  (schema 5 + freshness), `src/compile/runtime.rs` (surface, pin and
  freshness steps, floor recording), `src/commands/compile.rs` (schema
  5 emission, env carve-out), `tests/compile_command_test.rs`,
  `tests/compiled_artifact_test.rs`.
- Docs: `docs/adr/0083-artifact-signing-envelope.md` (amendment),
  `CONTEXT-MAP.md` glossary (truststore, freshness marker, floor).

## Architecture boundaries

Signing stays a CLI-boundary concern: compile output post-processing
plus a pre-boot runtime gate. No component, DSL, core, or service crate
changes. The artifact argument surface grows by the one sanctioned
`--truststore`. The 148-byte envelope framing does not change. Unsigned
compiles stay schema 3 and byte-identical. The truststore holds public
fingerprints and floors only; no key material enters the artifact, the
envelope, the manifest, the truststore, or a log.

## Phases

The work splits into two ordered delivery phases, each closing one bd
gap:

- **Phase 1 — Pinning.** Truststore format, parse, `--truststore` /
  `CAMEL_TRUSTSTORE` surface, pin check at both verify sites. Closes
  gap 1 (third-party pinning). Schema 4 artifacts are pin-checked;
  freshness is not yet enforced.
- **Phase 2 — Freshness.** Schema 5 marker, floor recording on boot,
  rollback rule, `truststore-update` handling. Closes gap 2
  (rollback/freshness). The truststore format from Phase 1 already
  carries the optional floor column, so Phase 2 adds no format change.

## Alternatives considered

- **JSON truststore** — rejected: a line-oriented file carries the same
  information with no ceremony, and malformed lines are trivially named
  by path and line.
- **Directory-of-files truststore** — rejected: merge semantics (which
  file wins on duplicate pins?) add ambiguity without information.
- **Flag-only or env-only surface** — rejected: flag-only makes pinning
  opt-in per invocation (a forgotten flag silently disables it);
  env-only hides the policy from the invocation. Both, flag wins.
- **Optional freshness field in schema 4** — rejected: an optional
  field in a frozen schema forks readers; the repo's pattern is a
  schema bump for field additions (`manifest.rs:405-407`).
- **Compile counter marker** — rejected: needs producer-side state (a
  counter file) that can be lost or raced; a unix timestamp is
  stateless and monotonic in practice. Trade-off: signed compiles are
  no longer byte-reproducible, which the bd's freshness requirement
  accepts.
- **`--verify` updates the floor** — rejected: side effects on a
  read-looking operation break the dry-run contract and make read-only
  truststores unusable for audit; boot is the enforcement point.
- **Lock-free max-merge floor writes** — rejected: an accept decision
  made on a stale floor can slip past a concurrent writer that already
  raised it; the check and the update must share one critical section
  (flock on `<truststore>.lock`).
- **Truststore pin check reads the manifest fingerprint without an
  envelope** — rejected (strip bypass, e_gpt Critical 1): the manifest
  fingerprint is attacker-editable metadata without the envelope's
  signature proof; under pin policy a signed manifest requires its
  envelope.
- **Schema-4 pinned artifacts accepted forever** — rejected (e_gpt
  Critical 2): same-key replay of legacy pairs is exactly the bd's
  rollback attack; acceptance ends at the first schema-5 boot of the
  key (the migration boundary).
- **Nanosecond marker** — rejected: producer wall-clock nanoseconds are
  not comparable across machines; seconds plus an explicit resolution
  bound is the honest claim.
- **Unwritable truststore fails open** — rejected: silently degrading
  to no-freshness is the rollback hole this change closes.
- **Empty truststore means "no truststore"** — rejected: silently
  disabling the pinning the operator asked for is fail-open.
- **Reject schema-4 pinned artifacts under a truststore** — rejected:
  breaks existing signed deployments on adoption; pin-only acceptance
  preserves compat.

## ADR-0083 amendment (draft)

Append to `docs/adr/0083-artifact-signing-envelope.md`; also update the
Status line to add: `Amended 2026-09-26: truststore pinning and
rollback freshness (keypin change)`.

```markdown
## Amendment: truststore pinning and rollback freshness (2026-09-26)

The `keypin` change adds a deployment-owned truststore and a rollback
freshness rule. It amends the Consequences deferral of third-party key
pinning and trust stores.

**Truststore.** A truststore is a single text file owned by the
deployment. Each non-empty, non-comment line pins one verifying key as
`blake3:<64 lowercase hex>`, the same form as the manifest
`key_fingerprint`, optionally followed by a recorded freshness floor.
The truststore holds public fingerprints and floors only; no key
material enters it. It is supplied by the artifact argument
`--truststore <PATH>` or the `CAMEL_TRUSTSTORE` environment variable;
the argument wins. Malformed input fails closed with exit 2: a
file-level failure names the path, a line-level failure names the path
and the line. An empty truststore pins nothing, so every signed
artifact fails its pin check. The truststore does not apply to unsigned
artifacts: unsigned compiles stay schema 3 and boot as before. Under a
supplied truststore, a manifest with a signing block and no envelope
fails closed: deleting the `.sig` file must not convert a signed
artifact into an unsigned one.

**Freshness.** A signed compile emits manifest schema 5: schema 4 plus
a mandatory `freshness` marker in the signing block, encoded as u64
unix-seconds. The marker lives inside the signed byte domain, so an
attacker cannot change it without breaking the signature. The
truststore records, per pinned key, the highest previously accepted
marker (the floor). At verify time, a marker below the floor fails
closed as a rollback with exit 2; a marker at or above the floor is
accepted. The first sight of a pinned key records its marker as the
floor. A pre-keypin schema-4 artifact passes pin-only until its key has
a recorded floor; afterwards it fails as a rollback — the first
schema-5 boot of a key is the migration boundary, and it does not
re-open. Rollback detection is bounded by the marker's one-second
resolution: artifacts compiled in the same second are not orderable by
the marker. Boot records the floor inside a lock-serialized critical
section (an advisory lock on `<truststore>.lock`) so concurrent boots
cannot accept on a stale floor or lower a recorded one; `--verify` is
a dry run and does not lock or write. A boot that must record a floor
and cannot lock or write the truststore fails closed.

**Compatibility.** Without a truststore, behavior is unchanged from R4:
the self-contained chain verifies and boots, and `--verify` prints the
algorithm and fingerprint. With a truststore, a signed artifact whose
key is not pinned fails closed, a signed manifest without its envelope
fails closed, and a pinned artifact below its floor fails closed.
Readers accept manifest schemas 2, 3, 4, and 5. The 148-byte envelope
framing does not change.
```