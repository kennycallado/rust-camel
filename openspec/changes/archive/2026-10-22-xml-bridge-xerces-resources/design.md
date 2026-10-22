# Design: xml-bridge-xerces-resources

## Approach

Single phase, three edits.

1. **Package the bundle.** Quarkus builds the native image from its own resource list. Evidence: entries in `src/main/resources/META-INF/native-image/resource-config.json` (glob form, then regex `pattern` form) did not reach the image: the build reported 45 resources both times, as in the baseline, and the binary still showed `Installation Problem???`. The same entry in `quarkus.native.resources.includes` gave 49 resources and a resolved bundle. Whether that file is dead config for other entries (and for `reflect-config.json`) is not investigated here; tracked in rc-2xxvz. The jar holds exactly two bundle locations (`impl/msg/*.properties`, `impl/xpath/regex/message*.properties`) plus a non-diagnostic `HTMLEntities.res`; both bundle locations are now in `application.yml`. A comment records the "must live here" fact and the jar-listing command for a Xerces bump.
2. **Keep the cause.** `SchemaSetCompiler` itself does not wrap exceptions; the loss happens in `XsdValidatorService` where a caught exception becomes a `BridgeError` using `e.getMessage()` only. `withCauses(Throwable)` returns the message chain (max 8 levels, duplicates and blanks dropped, joined with ` | `, falls back to the class name). It is used at the four catch sites that format compile/validate failures. `bound()` still truncates to `MAX_ERROR_TEXT` (512) at the wire, so the total is bounded. Known limit: Xerces formats `InvalidRegex: Pattern value '<pattern>' ... The reported error was '<message>'`, so for a pattern longer than about 400 characters the tail truncation can cut the parse message (the pattern prefix survives). Head-plus-tail truncation is tracked in rc-2vvqr and is not changed here because `bound()` is shared by every bridge error. The sites in scope are schema compilation only; document validation (`validateAgainstSchema`, `VALIDATION_ERROR` / `SECURITY_ERROR`) still uses `getMessage()` and is tracked in rc-o2lx9.
3. **Prove it natively.** A Rust integration test registers schemas against the worktree-native binary (same fail-closed binary resolver as the other xml-bridge tests).

## Alternatives rejected

- Edit `resource-config.json`: no effect under Quarkus (verified twice).
- Include `org/apache/xerces/**`: would pull class-adjacent non-diagnostic resources; the jar has none, but the broad glob hides a future jar change. Explicit two-location list kept.
- TCCL, Locale or System-property changes: R4, no reproducer.

## Boundaries

Confined to the XML sidecar internals; no gRPC contract or Rust production change. The change sharpens the bundle clause of the existing `sidecar-xml-security` requirement "Deterministic Xerces factory selection"; the security controls are unchanged. No CONTEXT-MAP ADR applies; data/control plane unaffected.
