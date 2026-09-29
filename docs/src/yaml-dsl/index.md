# YAML DSL

The YAML DSL declares routes as configuration files. The parser converts each
file into the same `RouteDefinition` the Rust builder API produces. The runtime
treats both authoring forms identically.

Reach for the YAML DSL when routes change more often than the application
binary. Operations teams ship route edits through config management, and the
hot-reload subsystem applies them without a redeploy. The same grammar also
parses as JSON.

- [Route structure](route-structure.md): anatomy of a route file
- [Step verbs](step-verbs/index.md): reference for every verb and field
- [Route-level config](route-level-config.md): error handlers, redelivery, circuit breaking, and security policy
- [Top-level blocks](top-level-blocks.md): REST DSL, MCP catalog, and template declarations
