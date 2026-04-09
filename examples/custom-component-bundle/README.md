# custom-component-bundle

Demonstrates how to implement a custom [`ComponentBundle`] for rust-camel.

## What this example shows

- Defining a `ComponentBundle` with its own TOML config key
- Registering a custom `Component` / `Endpoint` / producer via `ComponentRegistrar`
- Using `NoOpComponentContext` for standalone endpoint creation (useful in tests)
- Loading routes from `routes/*.yaml` with `discover_routes`

## Structure

```
custom-component-bundle/
├── Cargo.toml
├── Camel.toml          # [components.echo] config block
├── routes/
│   └── echo.yaml       # timer → echo:hello route
└── src/
    └── main.rs         # EchoConfig, EchoComponent, EchoEndpoint, EchoBundle
```

## How it works

### 1. Config

`EchoConfig` is deserialized from `[components.echo]` in `Camel.toml`:

```toml
[components.echo]
prefix = ">> "
```

### 2. ComponentBundle

`EchoBundle` implements `ComponentBundle`:

```rust
impl ComponentBundle for EchoBundle {
    fn config_key() -> &'static str { "echo" }

    fn from_toml(raw: toml::Value) -> Result<Self, CamelError> { ... }

    fn register_all(self, registrar: &mut dyn ComponentRegistrar) {
        registrar.register_component_dyn(Arc::new(EchoComponent::new(self.config.prefix)));
    }
}
```

### 3. Registration in main

```rust
if let Some(raw) = config.components.raw.get(EchoBundle::config_key()).cloned() {
    EchoBundle::from_toml(raw)?.register_all(&mut ctx);
} else {
    EchoBundle { config: EchoConfig::default() }.register_all(&mut ctx);
}
```

### 4. Route

```yaml
- route:
    id: echo-demo
    from: timer:tick?period=2000
    steps:
      - to: echo:hello
```

## Running

```bash
cd examples/custom-component-bundle
cargo run
```

Output (every 2 seconds):

```
INFO echo-demo >> <non-text body>
```

(The timer produces an empty body; configure `prefix` in `Camel.toml` to customise the output.)
