# Proposal: cxf-strict-config

## Why

camel-112 placed `exposed_soap_headers` at profile level instead of
`profiles.<name>.security`. The TOML loaded, nothing was logged, and the feature
silently did nothing — it cost a partner thread and a bad docs example (fixed in
996025c1b). `CxfProfileConfig`, `CxfSecurityFields`, and `CxfTlsConfig` ignore
unknown keys during deserialization, so any typo or misplaced key on the cxf
profile surface is accepted and dropped. This is the cxf-side instance of the
same fail-closed class already covered by `dead-config-policy` (bd rc-2rrzc).

## What Changes

- The cxf TOML surface (`[components.cxf]` pool table, `[[components.cxf.profiles]]`,
  `profiles.<name>.security`, `profiles.<name>.security.tls`) fails closed:
  a misplaced known key is a load error naming the profile and the
  location(s) where the key is accepted; an unknown key names the level and
  lists the accepted keys there. Pool-level violations identify
  `[components.cxf]`.
- Misplaced-key hints are derived from the config struct field lists (not a
  hand-picked list); keys accepted at more than one level list every location.
- A literal `listener_tls = {...}` pool table (today silently ignored) becomes
  a load error pointing at the five flat `soap_tls_*` keys.
- `docs/src/components/ws-soap.md` documents the accepted-keys-per-level
  contract and why that list is the contract.
- Out of scope: no new cxf feature, no change to existing accepted key
  semantics, no JMS changes (rc-d8svj), no version bump, no strictness change
  to `NetworkRetryPolicy` (owned by camel-component-api).

## Acceptance criteria

- Unknown key in the profile table → load error naming the profile, the key,
  and the accepted keys at that level.
- Unknown key in the security table → same contract.
- Misplaced `exposed_soap_headers` → error of the form "`exposed_soap_headers`
  is accepted under `profiles.<name>.security`, not under `profiles.<name>`".
- Misplaced known keys at any of the four levels get the same actionable hint,
  derived from the config structs; multi-location keys (keystore/truststore
  paths and passwords) list every accepted location.
- The correct placement still loads and works (positive case).
- The env-var assembly path (`CxfProfileEnvVars`, outbound only) is unaffected;
  unknown `CXF_PROFILE_*` env vars are not config keys and stay out of scope.
- All walker violations in one load are reported in a single deterministic
  (sorted) error; walker diagnostics carry key names, accepted locations, and
  (only as identity) the profile `name` — no other config values.
- Tests fail on main and pass after, one per spec scenario, each error test
  asserting the classification-appropriate message (accepted location for a
  misplaced key, accepted-keys list for an unknown key).

## Risk budget

- Breaking for configs that previously loaded with silently ignored keys —
  that is the intent; the error message lists accepted keys so the fix is
  actionable. Called out in the commit body.
- Existing valid TOML in the repo (docs examples, camel-test fixtures,
  `config_parsing_test.rs`) must keep loading — verified by the existing test
  suite.
- The `reconnect` sub-table is not recursed into (owned by
  camel-component-api); recorded as a known boundary, follow-up bd only if a
  mission later owns that crate.
