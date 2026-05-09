# `function:` Step — Out-of-Process Functions

## Syntax

```yaml
routes:
  - id: "my-route"
    from: "direct:start"
    steps:
      - set_body: "hello world"
      - function:
          runtime: deno
          source: |
            export default (camel) => {
              camel.setBody(String(camel.body()).toUpperCase());
              camel.setHeader("X-Enriched", "true");
            };
          timeout_ms: 5000
      - log: "done"
```

## Fields

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `runtime` | string | Yes | — | Only `"deno"` in v1. |
| `source` | string | Yes | — | TypeScript/JavaScript source. |
| `timeout_ms` | u64 | No | `5000` | Invocation timeout in ms. |

## Camel API (inside source)

The `camel` object passed to `export default (camel) => { ... }`:

| Method | Description |
|--------|-------------|
| `camel.body()` | Returns current body (string, object, or null) |
| `camel.setBody(value)` | Sets body: string→text, object→json, null→empty |
| `camel.header(name)` | Returns header value |
| `camel.setHeader(name, value)` | Sets a header |
| `camel.removeHeader(name)` | Removes a header |
| `camel.property(name)` | Returns property value |
| `camel.setProperty(name, value)` | Sets a property |

## `script:` vs `function:`

| Aspect | `script:` | `function:` |
|--------|-----------|-------------|
| Execution | In-process (Rhai) | Out-of-process (Deno container) |
| Language | Rhai | TypeScript / JavaScript |
| Startup | None | Container spawn + health poll |
| Isolation | Shared process | Full container isolation |
| Timeout | Not enforced | Mandatory (`timeout_ms`) |
| Hot-reload | Script recompile | Container lifecycle management |

## Error Mapping

| Kind | `CamelError::ProcessorError` prefix | Source |
|------|-------------------------------------|--------|
| `timeout` | `function:timeout: ...` | Client or runner timeout |
| `user_error` | `function:user_error: ...` | User code threw |
| `runner_unavailable` | `function:runner_unavailable: ...` | Container not healthy |
| `not_registered` | `function:not_registered: ...` | Not registered on runner |
| `transport` | `function:transport: ...` | Network / decode error |
| `invalid_patch` | `function:invalid_patch: ...` | Patch validation failed |

## Security

Runner flags: `--allow-net=0.0.0.0 --allow-env=PORT` only. No filesystem, no subprocesses, no other env.

## Container Lifecycle

- Containers are spawned on first function registration per runtime.
- `ContainerProvider::Drop` attempts graceful shutdown when the provider is dropped (best-effort, no panic outside tokio runtime).
- `ctx.stop()` triggers service shutdown which stops all runners.
- After stop, no containers with label `camel.function.runner=true` should remain.
- If a container leaks (e.g. process killed without graceful shutdown), re-running will create new containers; orphaned ones must be cleaned manually with `docker rm $(docker ps -q --filter label=camel.function.runner=true)`.

## v1 Limitations

- Only `deno` runtime
- Single runner per runtime (no pooling)
- No streaming bodies
- Container shared across function registrations
