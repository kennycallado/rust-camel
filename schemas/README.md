# Canonical Route Spec

Cross-language route definition IR for rust-camel.

## Files

| File | Source | Description |
|---|---|---|
| `canonical-route-spec.json` | schemars | JSON Schema (draft 2020-12) |
| `ts/*.ts` | ts-rs | TypeScript types (9 files) |
| `canonical_route_spec.go` | quicktype | Go types (run manually) |
| `canonical_route_spec.py` | quicktype | Python types (run manually) |

## Regenerate

```bash
cargo run -p xtask schema
```

## Generate Go/Python (requires quicktype)

```bash
npx quicktype schemas/canonical-route-spec.json -l go -o schemas/canonical_route_spec.go
npx quicktype schemas/canonical-route-spec.json -l python -o schemas/canonical_route_spec.py
```

## Versioning

- Schema `version` field (u32): only bumps on breaking changes
- JSON Schema `$comment`: includes compatible rust-camel version
- Runtime rejects unknown versions via `validate_contract()`

## Usage (TypeScript)

```typescript
import { CanonicalRouteSpec } from './ts/CanonicalRouteSpec';

const route: CanonicalRouteSpec = {
  route_id: "my-route",
  from: "timer:tick?period=5000",
  steps: [
    { step: "log", config: { message: "Hello" } },
    { step: "to", config: { uri: "log:info" } }
  ],
  version: 1
};
```

## Limitations (v1)

Canonical Route Spec v1 is intentionally a **partial model**. The following `RouteDefinition` fields are NOT represented:

- `auto_startup` / `startup_order` — always defaults
- `concurrency` — always inherits consumer default
- `error_handler` — no per-route error handling
- `unit_of_work` — no completion/failure hooks

Round-tripping `YAML → CanonicalRouteSpec → YAML` loses these fields. They must be set via the programmatic builder or YAML DSL directly.
