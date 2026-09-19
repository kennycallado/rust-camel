# Stream

The stream component is the stdio data-plane adapter. It routes exchange bodies between the process's standard streams and the integration runtime. `stream:out` and `stream:err` are producers. They write exchange bodies to file descriptors 1 and 2. `stream:in` is a consumer. It reads exchange bodies from file descriptor 0.

The echo route wires the consumer to the stdout producer:

```yaml
routes:
  - id: stream-echo
    from: "stream:in"
    steps:
      - to: "stream:out"
```

## URI

```text
stream:<target>?appendNewline=<bool>&charset=<charset>&frame=<line|raw|fixed>&size=<bytes>
```

| Target | Direction | File descriptor | Role |
| --- | --- | --- | --- |
| `out` | producer | 1 (stdout) | Exchange body as data |
| `err` | producer | 2 (stderr) | Prompts and diagnostics |
| `in` | consumer | 0 (stdin) | Exchange body source |

| Parameter | Default | Description |
| --- | --- | --- |
| `appendNewline` | `true` | Append one trailing newline to each body written to `out` or `err` |
| `charset` | `utf-8` | Body charset. v1 supports utf-8 only |
| `frame` | `line` | Framing mode: `line`, `raw`, or `fixed` |
| `size` | — | Frame size in bytes. Required and non-zero when `frame=fixed` |

## Consumer

`stream:in` reads one line per Exchange from file descriptor 0. Line mode decodes UTF-8 and strips the terminator. Line mode terminates on a `\n` byte only. A `\r` is stripped only before a `\n`. Apache Camel's stream component also treats a bare `\r` as a terminator; this component does not. Raw mode carries bytes verbatim. Fixed mode reads frames of `size` bytes. End of input completes the consumer's route gracefully. EOF is never an error. Empty input produces zero exchanges.

The consumer keys on input presence, never on terminal detection. No `isatty` call exists in the crate.

## Producer

`stream:out` writes the exchange body to stdout as data. `stream:err` writes prompts and diagnostics to stderr. The producer flushes after every exchange. One lock per file descriptor guards the whole write and flush of one logical write. Two endpoints on the same fd never interleave a body and its trailing newline.

## No redaction

The body is data. The producer writes it verbatim. Producer-side redaction would silently corrupt data output. The redaction plane (ADR-0076) covers logging and tracing output only. Stdio data output is operator-owned trusted egress. See [ADR-0080: Stream Component (Stdio Data Plane)](https://github.com/kennycallado/rust-camel/blob/main/docs/adr/0080-stream-component-stdio-data-plane.md).

## Operator caveat

A `stream:out` route and an enabled tracer stdout sink write to the same file descriptor. The component warns at boot when both are active. It never auto-muxes and never auto-disables either side. The general log layer also writes to stdout. Operators piping `stream:out` data must set `RUST_LOG=off`. `log_level = "WARN"` is not airtight. Any WARN mid-pipe still lands on stdout.

**Reference**: [camel-stream crate CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-stream/CONTEXT.md).