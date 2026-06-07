# file-pollenrich: EIP-7 pollEnrich Example

Demonstrates the `pollEnrich` EIP pattern: reading a file from disk mid-route
and merging its content into the exchange body.

## How it works

1. A timer fires every 1 second (`repeatCount=3`).
2. `pollEnrich` reads `config.json` from disk using the file component's
   `PollingConsumer`.
3. The file content replaces the exchange body (original headers/properties
   preserved via `UseEnrichedBody` strategy).
4. The enriched exchange is logged via `log:enriched`.

## Run

```bash
cargo run -p file-pollenrich
```

The example creates a temp file `/tmp/rust-camel-pollenrich/config.json`
with sample content, runs 3 timer ticks, and exits.

## YAML equivalent

See `routes.yaml` for the equivalent YAML route definition.
