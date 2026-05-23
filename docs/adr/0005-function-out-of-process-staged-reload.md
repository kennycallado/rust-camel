# `function:` Executes Out of Process with Staged Registration

`function:` steps run in isolated containers managed by a `ContainerProvider`, not in-process. Registration follows a staged prepare/finalize/discard flow to keep hot reloads transactional — a new function version is prepared before the old one is discarded.

Embedded JS (Boa) or WASM would be simpler but cannot support async I/O, npm packages, or full language runtimes. Out-of-process execution gives functions a real event loop and filesystem access at the cost of transport overhead and a more complex lifecycle. The staged reload prevents half-registered functions from being served during a reload.
