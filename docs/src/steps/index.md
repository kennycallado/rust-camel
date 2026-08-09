# Processing steps

Route steps that are not Enterprise Integration Patterns. They handle body
materialization, method invocation, and flow control. These are route-building
utilities, not named patterns from Hohpe and Woolf.

- [Stream Cache](stream-cache.md) — materialize a stream body into bytes so later steps can re-read it
- [Bean](bean.md) — call a registered Rust function as a route step
- [Stop](stop.md) — halt the pipeline without raising an error

For the route structure that hosts these steps, see [Routes and pipelines](../concepts/routes-pipelines.md).
