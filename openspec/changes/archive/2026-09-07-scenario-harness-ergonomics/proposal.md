# Proposal: scenario-harness-ergonomics

## Why

Epic rc-enbw (scenario-tier pilot follow-ups). Waves A–D landed wire fidelity, the HTTP contract surface, and the assertion vocabulary. The pilot's remaining pain is harness ergonomics: nested test trees cannot boot (rc-jjzy5), a `${env:X}` inside a YAML comment kills route load (rc-ayke), same-key client sends silently drop parked responses (rc-qogy), harness queue overflow masquerades as a verdict-class receive-timeout (rc-7mli), two statically-detectable document defects surface late or misclassed (rc-9dpx, rc-j87j), sends carry no fail-fast bound (rc-tr4w), `direct:` replies are unassertable (rc-qvz6), the concurrency recipe is undocumented (rc-3uihb), and inbound routes must pin fixed ports (rc-5yon). Each one pushes pilot teams back to bash workarounds.

## What Changes

- Scenario boot root becomes the nearest Camel.toml ancestor (lean-tier parity); relative `routeFiles` stay document-anchored.
- camel-dsl discovery interpolation becomes a parse-tree walk, so placeholders in comments never fail resolution (shared with `camel run`).
- ClientLane parks same-key responses in a bounded FIFO; overflows fail apparatus-class, never silently.
- Arrival-lane overflow converts the subsequent receive into an apparatus-class failure.
- Inline routes and bound-authority-less provisioning reject at load time (exit 2, doc-validation, before partners bind).
- Document-level `sendDeadline` bounds every send (default stays 30 s; no virtual time).
- `expectReply` on `direct:` sends asserts the parked reply through `camel-matchers::Expectation` (the fake adapter records sends and produces no reply — `expectReply` is a load error on any non-`direct:` target).
- Burst-send concurrency recipe documented (crate README + testing guide).
- Inbound listener provisioning: the harness stages a port-0 listener and exposes the bound address through a bindVar.

Excluded: virtual time (ADR-0069 §6), native concurrency primitives, unit-tier grammar changes, camel-matchers edits (consumed, never modified).

## Acceptance criteria

- A `.test.yaml` in a nested directory boots against the ancestor Camel.toml; a missing root fails named with exit 2.
- `${env:X}` in a YAML comment loads identically with the variable set or unset.
- N same-key sends followed by N receives complete in wire order; FIFO overflow fails apparatus-class.
- An overflowed arrival lane reports apparatus-class arrival-lane-overflow, not receive-timeout.
- Inline routes and no-bound-authority provisioning reject at load, exit 2, before partner bind.
- `sendDeadline: 500ms` fails a hung send at the document deadline.
- `expectReply` matches and mismatches the direct reply body; on non-`direct:` sends it is a load error.
- An inbound document runs on a bindVar-injected ephemeral port; no fixed ports remain in itest inbound tests.
- All quality gates green; itest suites pass with and without `--features http`.

## Risk budget

- Medium: boot-root resolution and interpolation touch `camel run` shared paths — back-compat pinned by existing dsl/boot tests plus new ones.
- No new dependencies, no camel-matchers changes, no virtual time, no unit-tier surface changes.
- rc-5yon expects a rebase against pyramid step 2 (camel-test growth) — isolated as the last phase.

Bd: rc-enbw — rc-jjzy5 rc-ayke rc-qogy rc-7mli rc-9dpx rc-j87j rc-tr4w rc-qvz6 rc-3uihb rc-5yon
