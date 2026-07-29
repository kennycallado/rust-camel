# Design: exec-cli-startup-guard

## Approach

Gate ExecBundle registration in `camel run` on whether any discovered route
actually uses exec. Two changes:

1. **Scanner (camel-core).** Add a reusable, public scheme-presence check that
   walks every **statically declared** URI in a route: the `from_uri()` plus
   every reachable step URI, identical in coverage to the existing SQL scanner
   (`To`, `WireTap`, `Enrich`, `PollEnrich`, recursing into `Filter`/
   `DeclarativeFilter`, `Split`/`DeclarativeSplit`/`DeclarativeStreamSplit`,
   `Multicast`, `Throttle`, `LoadBalance`, `Loop`/`DeclarativeLoop`,
   `IdempotentConsumer`, `Choice`/`DeclarativeChoice`, and
   `DeclarativeDoTry` try/catch/finally). Dynamic-URI steps (routing slip,
   recipient list, dynamic router) are runtime-resolved and skipped, matching
   the SQL scanner's documented contract. To avoid duplicating the structural
   `match`, refactor the private `walk_step_uris` into a generic
   `for_each_step_uri(step, &mut FnMut(&str))` and have both the SQL check
   collector and the new scheme scanner consume it. The existing
   `scan_route_definitions_for_sql_checks` tests guard the refactor.

   New API:
   ```rust
   pub fn route_definitions_reference_scheme(
       routes: &[RouteDefinition],
       scheme: &str,
   ) -> bool
   ```

2. **Gate (camel-cli).** Move the
   `#[cfg(feature = "exec")] register_bundle!(ctx, camel_config, ExecBundle)`
   block from the bundle-registration phase to **after route discovery**
   (currently step 5), wrapped in:
   ```rust
   let exec_used = route_definitions_reference_scheme(&defs, "exec");
   let exec_configured = camel_config.components.raw.contains_key("exec");
   if exec_used || exec_configured {
       register_bundle!(ctx, camel_config, camel_component_exec::ExecBundle);
   }
   ```
   Rationale: an explicit `[components.exec]` declaration is operator intent,
   so it must still be validated (catches typos, enforces ADR-0033 fail-closed
   on zero profiles). The skip applies only when exec is neither statically
   used nor declared — the actual over-firing bug.

Outcome:
- Exec neither used nor declared → ExecBundle never constructed → non-exec
  routes start.
- Exec used, OR exec declared → `ExecBundle::from_toml` runs `validate()` →
  zero-profiles still aborts (ADR-0033 preserved exactly).

Registration order is otherwise irrelevant (components register into a
registry resolved lazily at endpoint creation), so relocating exec
registration to after discovery but before `ctx.start()` is safe.

## Affected crates

- `camel-core`: add `route_definitions_reference_scheme`; refactor the private
  step-URI walker for reuse. New unit tests for the scanner.
- `camel-cli`: reorder/gate exec bundle registration in `commands/run.rs`.
- `camel-component-exec`: **unchanged** (fail-closed stays authoritative).
- `examples`: add `examples/camel-cli-no-exec` (Camel.toml without exec config
  + a `timer -> log` route; documents the behavior and serves as a regression
  demo). Added to the workspace `exclude` list (no Cargo.toml, CLI-run style).

## Architecture boundaries

Respects the data/control plane split and crate roles:
- **Runtime (camel-core)** owns route-definition introspection — it already
  hosts `scan_route_definitions_for_sql_checks`. The new scanner is a Runtime
  utility over trusted operator route definitions, not exchange data.
- **camel-cli** is the control-plane bootstrap that decides *when* to register
  the exec bundle from static route definitions (trusted config).
- **camel-component-exec** remains the security authority (ADR-0033). The CLI
  gate only decides whether the bundle participates; it never weakens the
  bundle's own validation.

No exchange (data-plane) input influences the gate — only discovered route
URIs from operator config.

## Alternatives considered

1. **Remove `exec` from camel-cli `default` features.** Rejected: removes exec
   from the default install, does not fix the general principle that
   fail-closed should be scoped to capability use, and shifts burden to users.
2. **Make `ExecBundle` treat zero profiles as "skip" (bundle-level).**
   Rejected: weakens ADR-0033 at the component layer for every caller.
3. **CLI-level route-driven gate (chosen).** Preserves ADR-0033 exactly,
   minimal, localized to the only auto-registering caller.

## Known limitation

Hot-reload semantics: if exec profiles were configured (or an exec route was
present) at startup, the ExecBundle is registered and later reloads that
introduce or change exec usage work normally. If exec was **neither used nor
declared** at startup, the bundle is not registered; a hot-reload that then
introduces the first `exec:` usage will fail endpoint resolution (the watcher
re-discovers routes but does not re-register bundles) and requires a restart.
A follow-up could register-on-demand during reload; intentionally deferred.
