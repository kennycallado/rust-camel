# Core concepts

- A **route** connects one inbound endpoint to an ordered processing pipeline.
- An **exchange** carries the input and output messages, properties, and error
  state through that pipeline.
- An **endpoint** is a configured address such as `timer:tick` or `log:info`.
- A **component** owns an endpoint URI scheme and creates its endpoints,
  consumers, and producers.
- A **processor** transforms or inspects an exchange. An **Enterprise
  Integration Pattern (EIP)** composes processing behavior such as choice,
  split, aggregate, retry, or filtering.

Consumers create exchanges and submit them to routes. Each step receives the
current exchange, and producers deliver it to outbound endpoints. Components
must be registered with the context before routes using their schemes start.
