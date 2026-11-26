# Tasks: observer-docs-semantics

<!-- skip_specs: docs-only change; no normative behavior deltas. -->

## 1. Documentation statements

Every task writes STE-flavored prose (short sentences, active voice, no
em-dashes, no banned verbs). No fenced code blocks are added anywhere in this
change; parameters appear as code spans. Each task edits exactly one page.

### 1.1 Circuit breaker: business errors are breaker-neutral

- Files:
  - `docs/src/eip/circuit-breaker.md` (modified)
- Steps:
  1. Append a new section at the end of the file (after the "Half-open
     fallback asymmetry" section) with heading `## Business errors are
     breaker-neutral` and exactly this prose:

     A business error counts as a success for both breaker implementations, in
     every state. It never advances the failure counter in Closed. A HalfOpen
     probe that ends in a business error succeeds and closes the breaker.
     Only technical failures open the breaker or fail the probe. ADR-0097
     records this as owner decision Q4. Both breakers follow it: the
     `CircuitBreakerGate` compiled from a route with an `error_handler`, and
     the Tower `CircuitBreakerService` compiled from a route without one.

     This rule interacts with the `http:` component. The HTTP producer raises
     `CamelError::HttpOperationFailed` when the response status falls outside
     the `okStatusCodeRange` parameter. The default range is `200-299`, so a
     `4xx` response is a technical failure and counts toward opening the
     breaker. When `4xx` responses are expected outcomes of the business call,
     widen the range with `okStatusCodeRange=200-499`. Then only `5xx`
     responses raise `HttpOperationFailed`, and `4xx` responses count as
     successes for the breaker. The widened range also accepts `3xx`
     responses, so a redirect that the producer does not follow counts as a
     success too.
  2. Link the ADR inline as `[ADR-0097](../adr/0097-business-error-envelope.md)`
     at its first mention in the new section.
- Tests:
  - name: breaker-section-citation
    setup: task 1.1 applied to the worktree
    action: `grep -c "Business errors are breaker-neutral" docs/src/eip/circuit-breaker.md`
    assert: output is `1`
  - name: breaker-adr-link
    setup: task 1.1 applied
    action: `grep -c "0097-business-error-envelope" docs/src/eip/circuit-breaker.md`
    assert: output is `1`
  - name: ok-range-mentioned
    setup: task 1.1 applied
    action: `grep -c "okStatusCodeRange=200-499" docs/src/eip/circuit-breaker.md`
    assert: output is `1`
- Acceptance:
  - `nix shell nixpkgs#mdbook -c mdbook build docs` exits 0
  - The new section cites ADR-0097 and names both breaker implementations
  - No fenced code block added; `okStatusCodeRange=200-499` is a code span
- [x] 1.1

### 1.2 Direct: admission, failure snapshot, back-pressure

- Files:
  - `docs/src/components/direct.md` (modified)
- Steps:
  1. In the `## Producer` section, immediately after the paragraph ending
     "Once enqueued, an exchange can still execute after its caller times
     out.", insert exactly this paragraph:

     Sequential is the default consumer model. The admission wait is part of
     the call: the `timeout_ms` boundary spans registry lookup, the admission
     wait, and pipeline execution, so a call that queues behind another call
     can time out before its route runs. To run calls in parallel, declare
     `concurrent: <max>` on the consumer route; `0` means no limit. See
     [Route structure](../yaml-dsl/route-structure.md) for the route keys.
     The direct-dispatch spec owns this contract
     (`openspec/specs/direct-dispatch/spec.md` in the repository, outside the
     rendered book).
  2. In the `## Error handling` section, after the existing paragraph ending
     "The route's error handler owns the operational signal.", insert exactly
     this paragraph:

     A failure inside the called route does not leak its mutations into the
     caller. When the caller wraps the call in `do_try`, the catch chain
     starts from the pre-try snapshot: the caller exchange as it was when the
     `do_try` block began. The snapshot discards headers and properties that
     the called route set, and also discards changes from try-body steps that
     ran before the call. Carry diagnostics across the boundary in the `data`
     field of the `throw` step. Match by `code` or `business_kind`; `message`
     is log-only and never crosses a mapping or projection (ADR-0097). The
     do-try spec owns the snapshot rule (`openspec/specs/do-try/spec.md` in
     the repository, outside the rendered book).
  3. Immediately after the paragraph from step 2, insert exactly this
     paragraph:

     A bounded `concurrent: <max>` applies back-pressure, not rejection. The
     Concurrent consumer acquires a permit before it dequeues an exchange, so
     extra exchanges wait for a permit and are never rejected while the route
     runs. With `concurrent: 0` the route sets no limit and admits every
     exchange. The producer `timeout_ms` still covers the wait, so a call that
     waits too long fails with the timeout error. The wait bounds memory under
     load (ADR-0044). See
     [Architecture](../architecture/index.md) for the consumer-model details.
  4. Link ADR-0044 inline as
     `[ADR-0044](../adr/0044-route-admission-backpressure.md)` at its mention
     in the step-3 paragraph, and
     `[ADR-0097](../adr/0097-business-error-envelope.md)` at its mention in
     the step-2 paragraph.
- Tests:
  - name: direct-fifo-timeout
    setup: task 1.2 applied
    action: `grep -c "the admission wait, and pipeline execution" docs/src/components/direct.md`
    assert: output is `1`
  - name: direct-snapshot
    setup: task 1.2 applied
    action: `grep -c "pre-try snapshot" docs/src/components/direct.md`
    assert: output is `1`
  - name: direct-backpressure
    setup: task 1.2 applied
    action: `grep -c "applies back-pressure, not rejection" docs/src/components/direct.md`
    assert: output is `1`
  - name: direct-adr-links
    setup: task 1.2 applied
    action: `grep -c -e "0044-route-admission-backpressure" -e "0097-business-error-envelope" docs/src/components/direct.md`
    assert: output is `2`
- Acceptance:
  - `nix shell nixpkgs#mdbook -c mdbook build docs` exits 0
  - Statements 2, 3, 4 of the mission order are each covered by exactly one
    paragraph on the page, with citations
  - No fenced code block added
- [x] 1.2

### 1.3 Env interpolation: probe cap arithmetic

- Files:
  - `docs/src/configuration/env-interpolation.md` (modified)
- Steps:
  1. In the `## Route documents` section, immediately after the sentence
     "The cap is eight candidate leaves for the whole document, shared by
     every probed subtree." (inside the paragraph about smallest-first
     subsets), insert exactly this paragraph:

     Each `throttle` step can contribute two of those leaves: its two integer
     fields, `max_requests` and `period_secs`. A document that puts
     `${env:}` tokens on both fields of five or more `throttle` steps carries
     ten or more candidates against the budget of eight and keeps its
     first-pass parse error. Split the `throttle` steps across documents, or
     reduce the tokenized fields until the document carries at most eight
     candidates in total. Other tokenized integer or boolean leaves share the
     same budget. The
     [camel-dsl crate CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-dsl/CONTEXT.md)
     records the probe mechanics.
- Tests:
  - name: env-throttle-arithmetic
    setup: task 1.3 applied
    action: `grep -c "two of those leaves" docs/src/configuration/env-interpolation.md`
    assert: output is `1`
  - name: env-field-names
    setup: task 1.3 applied
    action: `grep -c "max_requests" docs/src/configuration/env-interpolation.md`
    assert: output is at least `1`
  - name: env-dsl-context-citation
    setup: task 1.3 applied
    action: `grep -c "crates/camel-dsl/CONTEXT.md" docs/src/configuration/env-interpolation.md`
    assert: output is `1`
- Acceptance:
  - `nix shell nixpkgs#mdbook -c mdbook build docs` exits 0
  - The arithmetic states the eight-leaf cap, the two-leaves-per-throttle
    fact, and the five-step consequence
  - No fenced code block added
- [x] 1.3

### 1.4 MCP: per-server policy and error projection

- Files:
  - `docs/src/components/mcp.md` (modified)
- Steps:
  1. In the `## Server (Consumer)` section, replace the sentence
     "The DSL block's `security_policy` propagates to every lowered tool and
     resource route; the kernel authenticates each request" with exactly:

     The DSL `mcp:` block carries one `security_policy` for the whole server and
     propagates it to every lowered tool and resource route. The block has no
     per-tool policy: bind, TLS, and policy are server properties, not
     per-tool URI options. A tool that needs its own policy needs an explicit
     consumer route declared with `from: mcp:<server>/tool/<name>` and its
     own `security_policy`. The kernel authenticates each request

     Keep the rest of the paragraph ("per the route plan's credential
     sources ... reject surplus registrations at start.") intact. Then append
     to the end of that paragraph exactly one sentence:

     The mcp-component spec owns the server-property rule
     (`openspec/specs/mcp-component/spec.md` in the repository, outside the
     rendered book).
  2. In the `### Tool and resource behavior (steps/to passthrough)` section,
     after the paragraph ending "Declaring both fails lowering with a named
     error.", insert exactly this paragraph:

     A business failure projects to a tool error result: `isError: true` and
     a payload of exactly `{"error": <code>, "kind": <kind>}`. A resource read
     returns error content with the same payload. The `message` field of a
     `Business` error is log-only, and `data` is redacted diagnostics that are
     never logged in clear. No projection sends either field to the client.
     ADR-0097 owns the envelope contract.
  3. Link ADR-0097 inline as
     `[ADR-0097](../adr/0097-business-error-envelope.md)` at its mention in
     the step-2 paragraph.
- Tests:
  - name: mcp-per-server-policy
    setup: task 1.4 applied
    action: `grep -c "no per-tool" docs/src/components/mcp.md`
    assert: output is `1`
  - name: mcp-explicit-route
    setup: task 1.4 applied
    action: `grep -c "from: mcp:<server>/tool/<name>" docs/src/components/mcp.md`
    assert: output is at least `1`
  - name: mcp-projection
    setup: task 1.4 applied
    action: `grep -c "isError: true" docs/src/components/mcp.md`
    assert: output is at least `1`
  - name: mcp-adr-link
    setup: task 1.4 applied
    action: `grep -c "0097-business-error-envelope" docs/src/components/mcp.md`
    assert: output is `1`
- Acceptance:
  - `nix shell nixpkgs#mdbook -c mdbook build docs` exits 0
  - Statement 6 of the mission order is covered: per-server policy, explicit
    route escape hatch, and the `isError` projection with code and kind only
- [x] 1.4

### 1.5 Rhai: deliberate caps and the lean boot

- Files:
  - `docs/src/languages/rhai.md` (modified)
- Steps:
  1. Immediately after the paragraph that starts "Resource limits bound CPU
     and memory use." (the paragraph listing the defaults), insert exactly
     this paragraph:

     The caps are deliberate. Change them in `[languages.rhai.limits]`; the
     Rhai crate CONTEXT owns the limit table. The word
     `match` is reserved by the Rhai engine itself: a script cannot declare
     or call a function named `match`.
  2. Append a new section at the end of the file, before the final
     `**Reference**` line, with heading `## Unit-tier documents` and exactly
     this prose:

     `camel test` unit documents boot the closed lean component set
     (`direct`, `log`, `mock`, `seda`, `timer`) with `direct:` stimulus only.
     They read no `Camel.toml` (ADR-0064). A `[languages.rhai.limits]` block
     in the project's `Camel.toml` therefore never reaches a unit document;
     the lean boot evaluates Rhai with the rust-camel runtime defaults. A
     route that needs configured limits or a component outside the lean set
     runs in the integration tier. See
     [Declarative camel test](../testing/camel-test.md) for the tier contract
     and the thin-adapter pattern.
  3. Link ADR-0064 inline as
     `[ADR-0064](../adr/0064-two-tier-testing-contract.md)` at its mention in
     the new section.
- Tests:
  - name: rhai-reserved-match
    setup: task 1.5 applied
    action: `grep -c "reserved by the Rhai engine" docs/src/languages/rhai.md`
    assert: output is `1`
  - name: rhai-unit-boot
    setup: task 1.5 applied
    action: `grep -c "read no \`Camel.toml\`" docs/src/languages/rhai.md`
    assert: output is `1`
  - name: rhai-adr-link
    setup: task 1.5 applied
    action: `grep -c "0064-two-tier-testing-contract" docs/src/languages/rhai.md`
    assert: output is `1`
- Acceptance:
  - `nix shell nixpkgs#mdbook -c mdbook build docs` exits 0
  - Statement 7 of the mission order is covered: deliberate caps, reserved
    `match`, lean boot semantics with ADR-0064 citation
- [x] 1.5

### 1.6 camel test: integration-tier routing rule

- Files:
  - `docs/src/testing/camel-test.md` (modified)
- Steps:
  1. Immediately after the paragraph ending "The warning is exempt for
     `tests/fixtures/` paths and `*.test.yaml` documents. Migrate the send to
     an `intercepts:` block in a `*.test.yaml` document, as described above.",
     insert a new section with heading `## Routes that need the integration
     tier` and exactly this prose:

     Routes that carry a `security_policy` or consume from a `from: mcp:`
     endpoint do not run in unit documents. A `security_policy` route needs
     security configuration from `Camel.toml`, such as an authenticator, and
     the lean boot reads no `Camel.toml`. `mcp:` is outside the closed lean
     component set
     (ADR-0064). Keep such routes as thin adapters that forward to a
     `direct:` route, and unit-test the `direct:` route. The adapter gets its
     coverage in the integration tier.
  2. Link ADR-0064 inline as
     `[ADR-0064](../adr/0064-two-tier-testing-contract.md)` at its mention in
     the new section.
- Tests:
  - name: camel-test-integration-tier-section
    setup: task 1.6 applied
    action: `grep -c "Routes that need the integration tier" docs/src/testing/camel-test.md`
    assert: output is `1`
  - name: camel-test-thin-adapter
    setup: task 1.6 applied
    action: `grep -c "thin adapters that forward to a \`direct:\` route" docs/src/testing/camel-test.md`
    assert: output is `1`
  - name: camel-test-adr-link
    setup: task 1.6 applied
    action: `grep -c "0064-two-tier-testing-contract" docs/src/testing/camel-test.md`
    assert: output is at least `2` (the page already cites ADR-0064 once)
- Acceptance:
  - `nix shell nixpkgs#mdbook -c mdbook build docs` exits 0
  - Statement 8 of the mission order is covered: tier routing rule,
    thin-adapter pattern, unit-test the direct route
- [x] 1.6

### 1.7 Docs gates

- Files: none (verification only)
- Steps:
  1. Run `nix shell nixpkgs#mdbook -c mdbook build docs`; record the exit
     code.
  2. Run `nix shell nixpkgs#mdbook -c mdbook test docs`; record the exit
     code.
  3. Run `openspec validate observer-docs-semantics --type change --json`;
     record the result.
  4. Run `bash scripts/gates-local.sh` as a transient
     `systemd-run --user --unit=gates-observer-docs-semantics --collect` unit,
     polled in-turn; record the exit code.
- Tests:
  - name: mdbook-build
    setup: all six page tasks applied
    action: `nix shell nixpkgs#mdbook -c mdbook build docs`
    assert: exit code 0
  - name: mdbook-test
    setup: all six page tasks applied
    action: `nix shell nixpkgs#mdbook -c mdbook test docs`
    assert: exit code 0
- Acceptance:
  - Both mdbook exit codes are 0 and recorded in the mission report
  - `openspec validate` passes
  - `scripts/gates-local.sh` exits 0, recorded in the mission report
- [x] 1.7
