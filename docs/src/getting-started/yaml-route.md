# First route in YAML

Declare a route in YAML that produces log messages from a timer. You write
no Rust code and compile nothing. The CLI parses the YAML file and starts
the route.

The YAML route and the Rust builder route lower to the same
`RouteDefinition`. The runtime cannot tell which authoring form produced
it. Pick YAML when routes live with configuration, change without a
rebuild, or belong to an ops team. See ADR-0026 for the canonical
authoring decision.

The route file comes from the
[`config-basic`](https://github.com/kennycallado/rust-camel/tree/main/examples/config-basic)
example.

## The complete route

```yaml
{{#include ../../../examples/config-basic/routes/hello.yaml:first-route}}
```

## Project layout

A YAML route project needs two files: a route file and a `Camel.toml`
config file.

```
my-integration/
├── Camel.toml          # Config: route discovery, log level
└── routes/
    └── hello.yaml      # Route definitions
```

Run `camel new my-integration` to scaffold this layout. The config file
tells the CLI where to find route files. A minimal config declares the
route glob and a log level:

```toml
{{#include ../../../examples/config-basic/Camel.toml:minimal-config}}
```

The `routes` glob selects which files the CLI loads. The `log_level`
sets the tracing threshold for the whole context. See
[CLI usage](cli.md) for the full config reference.

## How it works

### Top-level structure

`routes` is the top-level list. Each entry is one route definition. A
file can hold many routes. Each route has three required fields: `id`,
`from`, and `steps`.

### Route identity

`id` is a unique name for the route. The CLI prints this id in log
output and startup messages. Use it to inspect or stop one route among
many at runtime.

### Consumer endpoint

`from` is the consumer endpoint URI.
`timer:tick?period=2000&repeatCount=3` creates a timer that fires every
two seconds, three times.

The URI has three parts. `timer` is the component scheme. `tick` is the
endpoint name inside the component. The query string sets the schedule.
`period=2000` fires every 2000 milliseconds. `repeatCount=3` stops the
timer after three ticks.

### Processing steps

`steps` is the ordered list of processing steps. The route runs them top
to bottom for each exchange.

The `log` step writes a fixed message through tracing. The `to` step
sends the exchange to a producer endpoint. Here `to: "log:info"` writes
the exchange body at info level. Both step verbs map to the same
processor types the Rust builder exposes.

## Run it

From the project directory, run:

```console
camel run
```

The CLI reads `Camel.toml` from the current directory. It loads every
file that matches the `routes` glob, parses each YAML route into a
`RouteDefinition`, and starts the context.

## What you see

The timer fires three times. Each tick produces one exchange. The `log`
step writes its message, then the `to: "log:info"` producer writes the
exchange body. You see log output every two seconds for six seconds.

After three ticks the timer consumer stops producing. The process keeps
running until you press Ctrl+C.

## Next steps

- [CLI usage](cli.md): run, scaffold, and inspect routes from the
  terminal.
- [YAML DSL](../yaml-dsl/index.md): every step verb and route option.
- [Core concepts](../concepts/index.md): the Exchange, Message, and
  CamelContext model.

**Reference**: [DSL crate](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-dsl/CONTEXT.md),
[Config crate](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-config/CONTEXT.md)
