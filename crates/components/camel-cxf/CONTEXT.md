## Security posture

### XML parsing delegation

The Rust Component treats SOAP and WSDL XML as opaque bytes. It does not run an
XML parser. The supervised Java CXF bridge owns XML parsing, DTD handling, and
entity resolution. This places the XXE boundary in the sidecar, not in this
crate. Audit parser hardening in the bridge process. See ADR-0032 for the trust
boundary.

### Credential channel separation

The bridge process receives keystore, truststore, and signature passwords
through per-profile environment variables. `CxfProfileEnvVars` wraps these
values in `Redacted` while it builds the child-process configuration. gRPC
requests carry only the configured `security_profile` selector. They do not
carry password bytes. See ADR-0051 for credential redaction requirements.

## Log-level policy

Per ADR-0012:

- **(b′) outside-contract** (consumer.rs L~272): response-marshalling failure after handler returns (post-handler side-effect). `runtime.metrics().increment_errors(route_id, "b-prime:cxf:response-marshalling")` BEFORE `error!`. Keeps `error!`.

- **(a) handler-owned** (consumer.rs L~285): route handler invocation returned Err to CXF consumer. Downgraded to `warn!`. No metric — route ErrorHandler owns ERROR.

- **(c) system-broken** (pool.rs L~441): pool restart max-attempts exhausted → "staying degraded" (lifecycle termination). Keeps `error!` with `system-broken` annotation. No metric — operator action required (fix CXF service config + restart).
