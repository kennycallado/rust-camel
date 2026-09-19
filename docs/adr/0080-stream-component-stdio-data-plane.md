# ADR-0080: Stream Component (Stdio Data Plane)

- Status: Accepted (2026-09-18)
- Ruling: e_opus ruling of 2026-09-18, knowledge-base id
  `ruling-e-opus-stream-stdio-2026-09-18` (BUILD-WITH-CONSTRAINTS). The
  ruling is unversioned evidence, in the same class as `docs/audits`.
  The durable decision text lives in this ADR.
- Inapplicable: ADR-0060 Rule 7 (line 108), "stdio transport: rejected.
  Camel is not a process supervisor." That rejection covers process
  supervision: spawning and supervising child processes over their
  stdio. This ADR covers the process's own file descriptors 0, 1, and 2.
- Companion: ADR-0076. The redaction plane covers logging and tracing
  output only. Stdio data output is operator-owned trusted egress.

## Context

The Stream component (`stream:`) is the stdio data-plane adapter. It
routes exchange bodies between the process's standard streams and the
integration runtime. `stream:out` and `stream:err` are producers. They
write exchange bodies to file descriptors 1 and 2. `stream:in` is a
consumer. It reads exchange bodies from file descriptor 0.

The e_opus ruling of 2026-09-18 (knowledge-base id
`ruling-e-opus-stream-stdio-2026-09-18`) resolves the identity question.
A `stream:` component is ordinary integration vocabulary. The interactive
job runtime semantic is scope creep and is rejected. The ruling is
unversioned evidence. Its decision text lives in this ADR.

## Decision

The component is a stateless three-file-descriptor adapter. It carries
no conversational state and no terminal detection.

### Rejected items

1. **Blocking mid-route `read` DSL step.** A step that blocks the route
   until a human types input couples the route to a human at the
   keyboard. That coupling breaks headless runs.
2. **`set-prompt` step.** A prompt is just a body sent to
   `to(stream:err)`. It needs no component config surface.
3. **REPL or conversational state.** The component is a stateless
   three-fd adapter. REPL state belongs to no component.
4. **isatty-driven control flow.** Behavior keys on input presence, not
   on terminal detection. The EOF rule makes TTY detection unnecessary.

### Un-redacted egress

`to(stream:out)` is a trusted egress decision by the route author. The
body is data. It is written verbatim. No lint covers this egress. The
gap is the same class as the accepted `println!` gap in camel-cli.
Producer-side redaction would silently corrupt data output. The redaction
plane (ADR-0076) covers logging and tracing output only. A future
optional `?mask=true` may reuse the camel-log masker. It is explicitly
not part of v1.

### Tracer-stdout collision

A `stream:out` route and an enabled tracer stdout sink write to the same
file descriptor. The component warns at boot when both are active. It
never auto-muxes and never auto-disables either side. Silent
reconfiguration is forbidden. The general log layer also writes to
stdout (crates/camel-config/src/context_ext.rs); operators piping
stream:out data must set RUST_LOG=off (log_level = "WARN" is not
airtight — any WARN mid-pipe still lands on stdout) — same posture:
warn, never re-route.

### Charset

v1 supports utf-8 only. Any other charset is rejected at construction
time. In line mode, a line that is not valid utf-8 is skipped. The skip
increments the error metric `b-prime:stream:decode`. Raw and fixed
frames carry bytes verbatim.

Consumer-side frames are bounded at the same 100 MiB materialization
limit as producer-side bodies. A frame past the limit fails the route
loudly with `CamelError::StreamLimitExceeded`. The consumer never
skips an oversized frame.

### Job lifecycle boundary

This change adds the `stream:in` allowlist entry and the load-gate tests
only. Job live-source waits, where a job's lifetime follows stdin to
EOF, belong to the job epic. The coordination note is rc-d5dgc. There is
no dependency. `camel run` hosts the composition today. EOF completes
the consumer's route. The process itself is signal-managed.

### Backpressure

The consumer is sequential. It reads the next frame only after the
current send completes. The kernel pipe buffer absorbs slack. There is
no unbounded queue. A closed route channel ends the consumer loop and
increments `b-prime:stream:fire-send` (ADR-0012 category b′).

After cancellation, frame boundaries on restart are undefined: the
consumer does not re-emit partial frame bytes it already consumed, so a
restarted consumer resumes mid-frame. Operators must not rely on
restart-resume framing.

## Consequences

- The four rejected items stay out of the component. Reopening any of
  them requires a new decision that supersedes this ADR.
- The ruling evidence stays unversioned. This ADR is the durable record
  of the decision.
- The job epic (rc-d5dgc) owns live-source wait mechanics. This change
  does not implement them.