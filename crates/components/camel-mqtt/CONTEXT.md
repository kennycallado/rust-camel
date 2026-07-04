# MQTT Component

MQTT 3.1.1 messaging component for rust-camel. Uses rumqttc under the hood.

## Breaking Changes

### `default = ["tls"]`

TLS is now enabled by default. Previously `default = []` required an explicit `--features tls`
to use TLS connections. This is a breaking change for users compiling without TLS support.

**Migration:** Add `default-features = false` to the dependency to opt out:

```toml
camel-component-mqtt = { path = "...", default-features = false }
```

When TLS is disabled, the `tls` module is conditionally compiled out (`#[cfg(feature = "tls")]`)
and MQTT connections use plain TCP only (`mqtt://` scheme).

### `connect_timeout = 10s`

MQTT broker connections enforce a 10-second connection timeout via
`NetworkOptions::set_connection_timeout(10)` in both the consumer and producer. This prevents
indefinite hangs on unreachable brokers. The timeout is not currently configurable — open an
issue if a per-endpoint override is needed.

## Feature Flags

| Feature | Description |
|---------|-------------|
| `tls` | TLS support via `rumqttc/use-rustls` (enabled by default) |
| `default` | `["tls"]` |

## Language

**MqttEndpointConfig**:
Configuration for an MQTT endpoint (`mqn:/` URI scheme). Contains broker reference, topic,
QoS, client ID, and connection parameters.

**MqttBrokerConfig**:
Connection settings for a named MQTT broker defined in `[components.mqtt.brokers.*]`.

## Example dialogue

> "I get a TLS error when connecting to my MQTT broker."
> "TLS is enabled by default (`default = ["tls"]`). If your broker uses plain TCP (mqtt://),
> disable TLS with `default-features = false`."
>
> "My MQTT connection hangs forever."
> "A 10-second `connect_timeout` is enforced. If the broker is unreachable, the connection
> fails with a timeout error within 10s instead of hanging indefinitely."
