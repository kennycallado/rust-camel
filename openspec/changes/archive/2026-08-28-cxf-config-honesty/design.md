# Design: cxf-config-honesty

## Approach

One theme — the two signing paths and the Dispatch cache must do what
their configuration promises — realized as four tasks in `bridges/cxf`
(Java) plus one Rust validation site in `crates/components/camel-cxf`.

**Fix 1 — apply signature knobs on both paths (rc-0zE).**

*1a. Producer (Dispatch path).* In
`SecurityProfile.createOutInterceptor()`, inside the `Signature` action
block, when a knob is non-blank put it into the WSS4J properties map (constants from `org.apache.wss4j.common.ConfigurationConstants`):
`SIG_ALGO`, `SIG_DIGEST_ALGO`, `SIG_C14N_ALGO`, `SIGNATURE_PARTS`
(`;`-separated segments, each either bare non-empty `localName` or
`{modifier}{namespace}localName`; modifier empty or exactly
`Element`/`Content`, namespace possibly empty, and `localName`
non-empty; passed verbatim after validation).

*1b. Consumer (signed responses).* In
`WssSecurityProcessor.processOutbound()`, apply the three algorithm knobs
via `WSSecSignature` setters (`setSignatureAlgorithm`,
`setDigestAlgo`, `setSigCanonicalization`) when configured.
Coverage (`sign.getParts()`) is NOT configurable here: Body+Timestamp is
the rc-gevh replay-defense invariant. The consumer path selects profiles
per-request (`wssProcessorFor` lazy map — there is no static
registration to abort), so the PARTS prohibition is enforced in Rust:
`CxfEndpoint::create_consumer` fails endpoint construction when the
consumer's selected `CxfProfileConfig` sets `signature_parts`, with an
error naming the knob and the invariant. Defense-in-depth: the
`WssSecurityProcessor` constructor throws on a PARTS-configured profile
(unreachable through the validated Rust path; covers direct sidecar
use).

**Fix 2 — construction-time validation (fail loud).** A
`validateSignatureKnobs()` step in `SecurityProfile.Builder.build()`
(same call from `SecurityProfileStore` and test builders):
- knob set + out-actions without `Signature` → reject, name env var;
- knob set + no signing keystore (`!canSignOutbound()`) → reject;
- `SIGNATURE_PARTS` strict grammar (WSS4J canonical order): `;`-
  separated segments; each is either a bare non-empty localName (no
  braces) or `{modifier}{namespace}localName` with modifier empty or
  exactly `Element`/`Content`, namespace possibly empty, localName
  non-empty;
- algorithm knobs must be absolute URIs (any scheme;
  `java.net.URI#isAbsolute` semantics).
WSS4J stays the semantic authority for algorithm support (a well-formed
but unsupported URI fails loudly at first invoke — documented,
accepted). This authority line extends to the producer path's
emitted-signature behavior: the producer scenarios of the
dead-config-policy delta are verified at interceptor-property level
(the literal WSS4J property keys carry the configured values); the
signature bytes those keys produce are WSS4J's documented contract, not
ours to re-test. (No producer-path e2e harness exists and building one
is out of proportion for this change; the consumer path — where our
code calls `WSSecSignature` directly — IS tested behaviorally.)

**Fix 3 — Dispatch cache honesty (rc-bp5c).**
- New immutable `record DispatchKey(String wsdl, String address, String
  service, String port, String profile, String operation, long
  timeoutMs)` — component-wise equality, no `#`-concatenation (SOAPAction
  URIs and URLs contain fragments). Blank operation normalizes to `""`;
  timeout normalizes to `bridgeConfig.connectionTimeoutMs()` when the
  request leaves it unset.
- `createDispatch` sets the full immutable context before publish:
  endpoint address, both timeouts, `soapaction.use=true`, and — when
  operation non-blank — `soapaction.uri`.
- `CxfBridgeService` computes the normalized timeout, passes it into
  `getDispatch`, and performs NO post-cache context writes (the two
  timeout `put`s there are deleted).
- `CxfClientManagerTest` gains the cache-key and concurrency assertions.

## Affected crates

- `bridges/cxf` (Java): `SecurityProfile.java` (apply + validate),
  `WssSecurityProcessor.java` (algorithm setters + PARTS constructor
  guard), `CxfClientManager.java` (`DispatchKey`, moved writes),
  `CxfBridgeService.java` (delete post-cache writes, pass timeout),
  tests (`SecurityProfileTest`, `WssSecurityProcessorIntegrationTest`,
  `CxfClientManagerTest`), `README.md` (Task 4: document the four knobs,
  both-path application, fail-loud rules, PARTS consumer prohibition).
  (`SecurityProfileStore.java` needs no change: its constructor already
  calls `Builder.build()`, so validation surfaces at startup
  automatically.)
- `crates/components/camel-cxf` (Rust): `component.rs`
  (`create_consumer` rejects PARTS-configured profiles) + unit test. No
  option surface change — validation only.

## Architecture boundaries

Bridge-scoped: sidecar-internal Java changes plus one `camel-cxf`
consumer validation; no IPC/protobuf change, no Runtime/DSL/Component
API movement. Fail-loud at construction mirrors ADR-0033
(security config validated at load) and the lockstep TLS-material aborts.
The consumer coverage pin preserves the rc-gevh oracle contract
(`timestampRewriteCannotMintFreshCacheKey`) — narrowing coverage via
config would reopen the replay hole; the abort makes that explicit.
Logging follows existing `java.util.logging` bridge conventions.

## Phases

Single phase, four tasks by class cluster: (1) profile validation +
producer application, (2) consumer application + PARTS prohibition
(Java guard + Rust validation), (3) DispatchKey race fix + service-layer
cleanup, (4) operator README. Shared review unit;
no milestone split needed for a ~120-line change.
