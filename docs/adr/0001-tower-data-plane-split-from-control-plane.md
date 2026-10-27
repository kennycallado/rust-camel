# Split Tower Data Plane from Control Plane

rust-camel separates exchange processing (data plane) from component/endpoint/consumer lifecycle (control plane). Every Producer and every non-structural processor is `Service<Exchange>`; structural and outcome-aware EIPs are `OutcomePipeline` segments executed inside the route's Tower service (see ADR-0024/0025). The control plane uses its own trait hierarchy (`Component`, `Endpoint`, `Consumer`) with richer lifecycle semantics.

Forcing components into Tower's request/response model would break lifecycle operations (start, stop, suspend, health) that don't map to `call()`. Keeping Tower strictly for exchange processing makes EIP composition idiomatic while giving the control plane the contracts it actually needs.
