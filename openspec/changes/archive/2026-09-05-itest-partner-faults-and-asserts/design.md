# Design: itest-partner-faults-and-asserts

## Approach

Two extensions over the shipped partner machinery, one phase each.

**Phase 1 — script behaviors (rc-8f6r).** A `partners:` entry gains three
optional fields beside `response`:

```yaml
partners:
  http://127.0.0.1:0/order:
    - method: GET
      path: /health
      times: 2              # serve N matching requests, then spent
      delay: 500ms          # hold before serving (humantime)
      response: {status: 200}
    - method: GET           # a faulted entry: no response at all
      path: /order
      fault: close          # drop the connection, no HTTP response
```

Today a matched script is consumed by the first matching request
(serve-once); `times` generalizes that to N. The adapter's script state
becomes `{script, remaining}` behind the existing mutex: match, decrement
(remove at zero) under the lock, then sleep the delay and serve — or, for
`fault: close`, abort the connection — outside the lock. Recording happens
before scripting decisions, so faulted requests stay on the recorder. Load
validation: `times` >= 1, exactly one of `response`/`fault`, humantime
`delay`, unknown fault names rejected; every failure names the partner key
and entry.

**Phase 2 — partner verification (rc-7x0l).** `validate` gains a
`partner` target. The declared endpoint URI must equal a declared harness
`http` partner ref (same cross-check as `partners:` keys; typo is a load
error). The assertion surface is the recorder snapshot:

```yaml
- validate:
    target: {partner: http://127.0.0.1:0/order}
    expectation: {count: 3, method: POST}   # method/path optional filters
    deadline: 5s                            # optional, partner targets only
```

`count` is exact and required; filters are case-insensitive on method,
exact on path-and-query, matching the script-matcher semantics. Without
`deadline` the assert reads one immediate snapshot. With `deadline` it
polls at a fixed short interval until the count matches or the deadline
passes — retry routes settle asynchronously, and immediate-only asserts
would flake. Mismatches fail with the existing `validation-mismatch`
verdict class naming the partner, filters, expected and actual counts.
`deadline` on a non-partner target is a load error. The router grows one
snapshot accessor (`recorded_requests(declared_key)`); assertions and
filtering live in the runner, keeping the recorder read-only.

The phases interlock in one e2e: a faulting-then-healthy partner drives a
retry route, asserted by a polled count.

## Affected crates

- camel-integration-test: `document.rs` (grammar, raw mirrors, load
  validation, `PartnerScript` shape), `adapters/http.rs` (script state,
  delay/fault serving, recorder snapshot passthrough on the router),
  `runner.rs` (partner-target validate, polling), tests.
- camel-cli: no driver changes expected (validate flows through the
  existing action loop); only feature-gated tests if any.
- None else; no product crate.

## Architecture boundaries

The change stays inside the integration-tier boundary (ADR-0069): the
harness scripts partners and asserts on wire evidence; product code is
untouched. Grammar strictness (deny-unknown-fields, load-time cross-checks)
follows the shipped pattern. The failure taxonomy is unchanged — new load
errors join exit 2, count mismatches join `validation-mismatch` at exit 1.
Recorder data remains harness-internal except through the new read-only
snapshot.

## Phases

### Phase 1: Script behaviors (delay, fault, times)
- **Goal:** partners can be slow, broken, and repeated on script.
- **Dependencies:** shipped `partners:` grammar and serve loop.
- **Externally-visible types/interfaces:** `PartnerScript` fields
  (`times`, `delay`, `fault`), wire `ScriptedResponse` carrying them.
- **Deliverable:** crate code + unit/e2e tests + README grammar rows.
- **Exit-criteria:** e2e green for held response, transport-error fault
  (still recorded), `times` spend-through; old documents unchanged.

### Phase 2: Partner verification (count asserts)
- **Goal:** scenarios assert on what partners actually received.
- **Dependencies:** Phase 1 (the retry e2e needs `fault` + `times`).
- **Externally-visible types/interfaces:** `ScenarioTarget::Partner`,
  router `recorded_requests`, partner-expectation grammar.
- **Deliverable:** validate wiring + tests + book/README (noting partner
  expectations are exact-count, not subset) + retry example.
- **Exit-criteria:** immediate and polled count asserts, filtered and
  unfiltered, pass and fail paths proven by e2e; retry example green.

## Alternatives considered

- **Probabilistic faults (chaos-style).** Rejected: nondeterministic CI;
  deterministic scripts plus `times` express the same scenarios.
- **Fault shapes beyond `close` (truncate, stall-forever, bad-status-line).**
  Deferred: `close` covers the retry/circuit-breaker class; more shapes
  when a route test needs them.
- **History-shape asserts (Nth-request body) in v1.** Deferred: count
  with filters proves the retry story; shape asserts follow once count
  semantics are field-proven (bd follow-up).
- **A separate `assert` action.** Rejected: `validate` already owns
  assertions and the `validation-mismatch` class; a partner target is a
  target, not a new action.
