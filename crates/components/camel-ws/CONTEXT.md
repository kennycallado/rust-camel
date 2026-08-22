# WebSocket component

This file defines crate-local terms and invariants. See `../CONTEXT.md` for the shared
Component, Endpoint, Consumer, and Producer terms.

## Language

**`WsComponent` / `WssComponent`**:
Components for plain and TLS WebSocket Endpoints. Each Endpoint can create an inbound Consumer or
an outbound Producer.

**`ServerRegistry`**:
Process-wide owner of shared inbound servers. It keys servers by port and keeps them alive after
the last Consumer stops. Paths can register and deregister independently on one server.

**`WsConnectionRegistry`**:
Per-path registry of active WebSocket connections. Producers use it for targeted or broadcast
delivery.

**`dispatch_handler`**:
Inbound upgrade handler. It checks path and origin before optional bearer authentication and
policy evaluation.

## Lifecycle and security invariants

- `ServerRegistry::get_or_spawn` creates at most one server per port. The first registration fixes
  the host and TLS mode. A later registration with the other TLS mode fails.
- `ServerRegistry::release` removes the Consumer reference but does not stop the server. Consumer
  shutdown removes its path, policy, and connection registry.
- `WsReloadHandler::matches` matches `wss` servers by port and intentionally ignores host. This
  mirrors the port-keyed `ServerRegistry`.
- Plain WS binds its TCP listener before `ConsumerContext::mark_ready`. WSS
  starts its bind in the server task, so `WsConsumer::start` awaits
  `axum_server::Handle::listening()` before signalling readiness. On bind
  failure (`listening()` returns `None`) `start` returns `Err`, so the route
  never marks itself ready on a dead listener. The health pin
  (`g:ws:bind-tls`) and the `WsConsumer::stop` error remain secondary failure
  signals for bind errors that surface after readiness. ADR-0007 requires
  such task failures to remain visible to Route supervision.
- When a SecurityContext exists, `dispatch_handler` fails closed on missing credentials, denied
  policy decisions, and future `AuthorizationDecision` variants. Query-token values are redacted
  before logging.
- A route may declare `credential_sources` (ADR-0059). The consumer resolves them in declared
  order through the shared camel-auth extraction. The component authenticates the token itself and
  passes the resulting principal to the policy; the removed `trust_upstream_principal` flag no
  longer exists and exchange-property principal evidence never authorizes. URI logging redacts
  every declared query-parameter source name, plus the `access_token` and `token` defaults.
- `WsEndpointConfig::fmt` redacts TLS certificate and key paths. ADR-0051 does not classify paths
  as credential bytes, so this crate uses a stricter diagnostic policy.
- ADR-0052 does not apply. `camel-ws` is an inbound data-plane component, not a diagnostic
  endpoint. Its `0.0.0.0` default therefore does not inherit the diagnostic loopback rule.

## Log-level policy

Per ADR-0012, this component's `error!` sites are outside the handler contract:

- **Class (g)** (`spawn_server`, TLS-bind arm): the code first calls
  `force_unhealthy_for_route` with `g:ws:bind-tls`. The health pin is the operator signal.
- **Class (g)** (`spawn_server`, plain-bind arm): the code first calls
  `force_unhealthy_for_route` with `g:ws:bind-plain`. The health pin is the operator signal.
- **Class (e)** (`dispatch_handler`, policy-evaluation error arm): the code first increments
  `e:ws:policy-eval`. The metric is the operator signal.

Each site keeps `error!` for loud log visibility and carries `// log-policy: outside-contract`.
