# Design: detretry

## Approach

Extend the rc-rif19 typed-provenance discipline (rc-3px7o: markers, never
Display sniffing) to the outer transport arm. Four moves:

1. **Typed transport failure (camel-cli).** `attempt_send`'s outer error
   changes `String` → `TransportFailure` enum:
   `ComponentNotRegistered { uri, scheme }`, `EndpointCreation { uri,
   cause: CamelError }`, `ProducerCreation { uri, cause: CamelError }`.
   `Display` reproduces today's three `format!` strings byte-identically
   (`failed to send to {uri}: `{scheme}:` component not registered` /
   `failed to create endpoint {uri}: {e}` / `failed to create producer
   for {uri}: {e}`).    `SendError::Transport` carries `TransportFailure`;
   its consumption site (`{detail}` at the error-policy stderr site,
   early exit-2 class — no JSON report on this path) is Display-driven
   and stays unchanged.

2. **Deterministic classifier (camel-cli).**
   `is_deterministic_transport_failure(&TransportFailure) -> bool`:
   - `ComponentNotRegistered` → true (registry frozen after boot; the
     load-time `direct:`/`seda:` send-target allowlist makes the miss
     near-unreachable, but the classification is correct by construction).
   - cause is `CamelError::InvalidUri(_)` → true, structurally — the
     variant decides (rc-utx98). Both direct and seda URI/param parse
     errors ride `InvalidUri` (`invalid size`, `parse_wait_for_task`,
     `validate_name`, …). A bad URI never becomes valid on retry.
   - cause carries the seda `TerminalConfigError` marker → true, via the
     existing pub predicate `is_seda_terminal_config_error` (bounded
     8-hop source-chain walk).
   - else → false (retryable default; foreign components keep the
     default until they own a marker — sedaretry design doctrine).

3. **Loop change (camel-cli).** The `Err(detail)` arm of
   `send_with_startup_retry` returns `SendError::Transport` on the first
   attempt when `is_deterministic_transport_failure`; otherwise keeps the
   sleep-and-retry window semantics exactly.

4. **Config-conflict marker (camel-component-seda).**
   `get_or_create_state`'s incompatibility path maps
   `is_compatible_with`'s `Err(String)` to
   `CamelError::EndpointCreationFailedWithSource(detail,
   TerminalConfigError::EndpointConfigConflict)` — detail byte-identical.
   The crate-private `TerminalConfigError` enum grows the
   `EndpointConfigConflict` variant (Display stays deliberately
   non-canonical). `is_seda_terminal_config_error` matches both variants;
   `is_direct_startup_race`'s `EndpointCreationFailedWithSource` arm
   already excludes whatever the predicate matches, so the inner arm
   fails fast on the conflict too — consistent: a deterministic conflict
   never clears on any arm. Rustdoc on both predicates records the
   widened class.

**Deterministic/transient boundary** (mission requirement, both sides
pinned by tests):

- Deterministic: registry miss; `InvalidUri` (bad endpoint URI/params);
  seda config conflict; seda multipleConsumers+wait conflict (existing).
- Transient (retry window's purpose): plain
  `EndpointCreationFailed` — foreign components' plain creation failures
  on the transport arm (mid-boot connection states, DNS blips at
  creation time all ride plain variants today; they keep the window).
  The SEDA queue-full/enqueue/fanout-timeout residual is an INNER-arm
  (pipeline) class — fires during the producer's send, not during
  apparatus creation — and is untouched by this change.
- Ambiguous-class note: DNS/host-resolution at endpoint creation is the
  genuinely ambiguous case (blip clears vs typo'd host never clears). On
  this arm it cannot occur through the job path (see keycloak boundary
  below); where plain variants carry it elsewhere, the retryable default
  is the conservative choice — fail-fast misclassification would break
  the startup-race retry the window exists for.

**Keycloak boundary.** The bd claims the arm carries keycloak
endpoint/role config errors. Job documents reject non-`direct:`/`seda:`
send targets at load (cli-jobs spec, exit 2), so `keycloak:` endpoints
never reach `attempt_send` — the claim is refuted by the allowlist. No
keycloak marker is added: component-owned doctrine says a component opts
in when a real seam needs classification. The other transport-shaped
call sites (`commands/test/runner.rs`, `camel-integration-test`
`adapters.rs`) do accept arbitrary schemes, but their mission scope is
comment-only; keycloak marker work stays out of scope.

**Comment drift fold.** The three `is_direct_startup_race` call sites
still describe only the no-active-consumers gate exclusion
(rc-tgaxf); since rc-rif19 the predicate also excludes the
terminal-config class. Comments at `commands/test/runner.rs` (~270-279),
`commands/test_support.rs` (~41-44), `camel-integration-test/src/adapters.rs`
(~885) name both exclusions. Comment-only; no behavior change.

## Affected crates

- `camel-component-seda`: `TerminalConfigError::EndpointConfigConflict`
  variant, `get_or_create_state` conversion, predicate/classifier
  rustdoc, unit tests (marker present, byte-identical detail, inner-arm
  narrowing, foreign-imitation retryability).
- `camel-cli`: `TransportFailure` enum, `attempt_send` signature,
  classifier, loop arm, doc comments, classification characterization
  tests, subprocess e2e (config-conflict job exits fast with exit 2,
  elapsed far below window; stderr text pinned — no JSON report on the
  transport path).
- `camel-integration-test`: call-site comment only.

## Architecture boundaries

Components own their classification (seda owns marker + predicate; no
caller-side Display sniffing — rc-fr20u/rc-3px7o). camel-api's
`EndpointCreationFailedWithSource` is the carrier; `OpaqueErrorSource`
keeps markers unforgeable outside the crate. camel-cli's job loop
consumes only pub predicates and the `CamelError` variant — no new
cross-crate dependency (camel-cli already depends on
camel-component-seda; keycloak stays behind camel-bundles' `security`
feature, untouched). ADR-0012 error family unchanged (same
`error!` site, same Display). ADR-0024 verdict fidelity: outcome mapping
unchanged — pipeline failures keep `Failed`/exit 1, transport failures
keep exit 2.

## Alternatives considered

- **Classify by Display sniffing the transport strings**: rejected —
  violates the typed-provenance doctrine (foreign imitations would
  fail-fast wrongly).
- **Shared generic terminal marker in camel-api for all components**:
  rejected — repeats the sedaretry ruling; each component owns its
  marker and opts in at a real seam.
- **Keycloak markers now (defense in depth)**: rejected — unreachable on
  this arm (send-target allowlist); dead classification until another
  seam needs it.
- **Treat plain `EndpointCreationFailed` as deterministic unless marked
  transient (invert the default)**: rejected — breaks the foreign
  plain-creation retries the transport window exists for, and would
  misclassify the inner-arm classes' plain variants; conservative
  default stays retryable.

Bd: rc-zovuy
