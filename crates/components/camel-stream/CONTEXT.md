# camel-component-stream

The Stream component routes exchange bodies between the process's
standard streams and the integration runtime. `stream:out` and
`stream:err` are producers. They write exchange bodies to file
descriptors 1 and 2. `stream:in` is a consumer. It reads exchange bodies
from file descriptor 0.

See `crates/components/CONTEXT.md` for the shared Component, Endpoint,
Consumer, and Producer vocabulary.

## Language

**StreamComponent**:
Component registered under the `stream` scheme. It parses the URI and
creates a StreamEndpoint.
_Avoid_: stdio adapter, stream handler

**StreamEndpoint**:
Crate-private Endpoint that holds a parsed StreamConfig and creates a
StreamProducer for the `out` and `err` targets. The `in` target is a
consumer.
_Avoid_: stream URI, fd endpoint

**StreamProducer**:
Crate-private Tower `Service` that materializes the exchange body and
writes the bytes to the target file descriptor, then returns the
Exchange unchanged.
_Avoid_: stdout writer, data sink

**StreamConfig**:
URI-derived configuration for target, trailing-newline insertion,
charset, framing, and fixed frame size.
_Avoid_: stream options

**StreamTarget**:
Closed set of stream targets: Out (fd 1), Err (fd 2), and In (fd 0).
_Avoid_: stream direction

**StreamFrame**:
Closed set of framing modes: Line, Raw, and Fixed.
_Avoid_: framing mode (ambiguous)

## Scheme table

| Scheme | Direction | File descriptor | Role |
|--------|-----------|-----------------|------|
| `stream:out` | producer | 1 (stdout) | Exchange body as data |
| `stream:err` | producer | 2 (stderr) | Prompts and diagnostics |
| `stream:in` | consumer | 0 (stdin) | Exchange body source |

## Config surface

| Option | Default | Meaning |
|--------|---------|---------|
| `appendNewline` | `true` | Append one trailing newline to each body written to `out` or `err` |
| `charset` | `utf-8` | Body charset. v1 supports utf-8 only |
| `frame` | `line` | Framing mode: `line`, `raw`, or `fixed` |
| `size` | — | Frame size in bytes. Required and non-zero when `frame=fixed` |

## Semantics

- **Framing.** Line mode writes one exchange per line. Raw mode writes
  bytes verbatim. Fixed mode writes frames of `size` bytes.
- **EOF.** End of input completes the consumer's route gracefully. EOF
  is never an error. Empty input produces zero exchanges.
- **Input presence.** The consumer keys on input presence, never on
  terminal detection. No `isatty` call exists in the crate.
- **No redaction.** The body is data. The producer writes it verbatim.
  The redaction plane (ADR-0076) covers logging and tracing output only.
- **Per-fd serialization.** One lock per file descriptor guards the
  whole write and flush of one logical write. Two endpoints on the same
  fd never interleave a body and its trailing newline.
- **Flush per exchange.** The producer flushes after every exchange.

## Related decisions

- ADR-0080: stream component stdio data plane. Records the rejected
  items, the un-redacted egress limitation, the tracer collision
  posture, the v1 charset rule, the job lifecycle boundary, and the
  backpressure shape.
- ADR-0076: URL redaction strictest-wins. The redaction plane covers
  logging and tracing output only. Stdio data output is operator-owned
  trusted egress.