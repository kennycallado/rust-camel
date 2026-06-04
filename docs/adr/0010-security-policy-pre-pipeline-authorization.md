# SecurityPolicy as Pre-Pipeline Authorization

Routes declare authorization with route-level `security_policy` rather than as a normal Step. The DSL accepts exactly one policy form (`roles`, `scopes`, `ref`, `wasm`, or `permission`) and the Runtime wraps the compiled Route Pipeline with `SecurityPolicyLayer`. A granted decision stores Principal properties on the Exchange before normal Route Steps run. A denied decision returns `Unauthorized` into the route error-handling path; downstream Route Steps do not run unless error handling routes or handles the error.

The alternative was modeling authorization as an ordinary Pipeline Step. That would make auth placement explicit and composable, but it would also make route safety depend on Step ordering. A transform, producer call, or side-effecting processor could run before authorization if the policy Step were misplaced. Pre-pipeline authorization makes the protected boundary the Route itself: all Step logic is inside the authorization gate.

This choice makes SecurityPolicy part of route lifecycle assembly rather than EIP processing. It is less flexible than arbitrary Step placement, but it gives one clear contract: protected Routes authorize before data-plane processing begins. Policies can still be backed by native role/scope checks, named references, WASM plugins, or permission engines; those choices affect the decision source, not the boundary where enforcement happens.

Current canonical/hot-reload compilation rejects `security_policy` until the canonical route contract carries SecurityPolicyConfig safely. See [ADR 0011](./0011-canonical-route-spec-minimal-contract.md) for why canonical contracts stay serializable and do not carry trait-object policy implementations.
