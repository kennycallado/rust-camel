# camel-component-stream

> Stream (stdio) component for rust-camel

## Overview

The Stream component is the stdio data-plane adapter for rust-camel: `stream:out` and `stream:err` are producers that write exchange bodies to the process's standard output and standard error streams, and `stream:in` is a consumer that reads exchange bodies from standard input. Framing (`line`, `raw`, `fixed`), trailing-newline insertion, and charset are configured per endpoint URI. UTF-8 is the only charset in v1.

## URI Format

```
stream:target[?options]
```

## Targets

| Target | Direction | Description |
|--------|-----------|-------------|
| `out` | producer | Writes exchange bodies to stdout |
| `err` | producer | Writes exchange bodies to stderr |
| `in`  | consumer  | Reads exchange bodies from stdin |

## URI Options

| Option | Default | Description |
|--------|---------|-------------|
| `appendNewline` | `true` | Append a trailing newline to each body written to `out`/`err` |
| `charset` | `utf-8` | Body charset; only `utf-8` is supported in v1 |
| `frame` | `line` | Framing mode: `line`, `raw`, or `fixed` |
| `size` | — | Frame size in bytes; required (and non-zero) when `frame=fixed` |

## Design

Rationale for the stdio primitive mapping:
[ADR-0080](../../../docs/adr/0080-stream-component-stdio-data-plane.md)
(ruling `ruling-e-opus-stream-stdio-2026-09-18`).

## License

Apache-2.0
