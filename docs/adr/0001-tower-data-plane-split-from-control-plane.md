# Split Tower Data Plane from Control Plane

rust-camel separates exchange processing (data plane) from component/endpoint/consumer lifecycle (control plane). The data plane uses `Service<Exchange>` throughout — every Processor and Producer is a Tower service. The control plane uses its own trait hierarchy (`Component`, `Endpoint`, `Consumer`) with richer lifecycle semantics.

Forcing components into Tower's request/response model would break lifecycle operations (start, stop, suspend, health) that don't map to `call()`. Keeping Tower strictly for exchange processing makes EIP composition idiomatic while giving the control plane the contracts it actually needs.
