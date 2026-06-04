# HTTP Co-hosting for API and Static Routes

HTTP API routes (`http:`) and static file mounts (`http-static:`) share one server per host/port. A single listener dispatches each request by precedence: exact API path match first, then static mounts by longest prefix, then SPA fallback or custom error page handling within the winning static mount. This makes co-hosting a public routing behavior rather than an incidental registry detail.

The alternative was starting separate servers for API and static routes, or requiring separate ports. That would keep each Route simpler internally, but it would make common web deployments awkward: an application API and its static assets would need extra reverse-proxy configuration or distinct origins. Shared hosting matches user expectations for one web origin while keeping route definitions independent.

The trade-off is that the HTTP component owns shared dispatch state for a host/port and must define deterministic precedence between route kinds. Exact API paths win over static mounts so API endpoints cannot be shadowed by a broad static prefix. Static mounts use longest-prefix matching so more-specific asset trees win before generic fallbacks. SPA fallback and error pages run last within the winning static mount because they are catch-all behavior.
