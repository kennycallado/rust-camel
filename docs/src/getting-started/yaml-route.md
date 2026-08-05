# First route in YAML

The same route shape can be declared without compiling application-specific
Rust. This snippet is included from the `config-basic` example and is parsed
by rust-camel's YAML DSL tests:

```yaml
{{#include ../../../examples/config-basic/routes/hello.yaml:first-route}}
```

The top-level `routes` list contains route definitions. `from` identifies the
consumer endpoint, and `steps` run in order for every exchange. In a project
created by `camel new`, place route files under `routes/` and run `camel run`.
