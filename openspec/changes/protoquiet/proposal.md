# Proposal: protoquiet

## Why

`resolve_protoc_with` contains the vendored-protoc panic via
`catch_unwind`, but the default panic hook still prints the
`thread panicked` stderr noise BEFORE the typed
`ProtoCompileError::ProtocUnavailable` surfaces. Operators see a
scary trace plus the clean error. Non-blocking cosmetics follow-up
from protofix (bd rc-5bz6r, landed e2b6087e); tracked as bd rc-5vbmf.

## What Changes

- Scope a panic-hook silencer around the `catch_unwind` window in
  `resolve_protoc_with` (crates/services/camel-proto-compiler):
  `panic::take_hook` + silent `set_hook`, previous hook restored by a
  `Drop` guard. No global `set_hook` leak — hook state is restored on
  every exit path of the guarded window.
- Unit test pins that a vendored panic does NOT invoke the installed
  panic hook (recording-hook pattern under `PROTOC_COMPILE_LOCK`),
  and that the typed error still carries the panic payload message.

## Impact

- Affected code: `crates/services/camel-proto-compiler/src/compiler.rs`,
  tests in `src/lib.rs`.
- No spec delta: the observable contract (env override first, vendored
  fallback second, typed `ProtocUnavailable`) is unchanged; this
  removes stderr noise only.
- No docs change: `docs/src/data-formats/protobuf.md` already describes
  the typed failure mode and does not mention hook noise.
