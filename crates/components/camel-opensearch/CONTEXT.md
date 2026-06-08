## Log-level policy

Per ADR-0012, this component's `error!` sites are categorized as:

- **(c) system-broken** (producer/mod.rs L97, L108): endpoint init failures (URL parse + transport build) in `build_client`. Static method — no `self.runtime` access for inline health-pin call. These are init-time config failures (bad URL, broken transport) that prevent the entire producer from functioning — system-broken is appropriate. Keeps `error!` with `// log-policy: system-broken`. No metric call (operator alert via `error!` is the signal).

Reviewer: r_glm5.1 verifies these classifications against source at Phase C review time.
