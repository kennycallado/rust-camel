# Route-Supervised Consumer Failure

Consumer internal task failure is route-supervised. A Consumer that cannot continue returns an error from its running task; the RuntimeBus records the Route as failed, and an optional supervision policy restarts the whole Route with backoff. Consumers may retry transient external operations inside their normal receive loop, but they must not restart their own long-running task after task-level failure.

Self-supervising Consumers would hide failures from the Route control plane and could emit Exchanges while hot reload is swapping Pipelines. Pure fail-fast would be simpler and safest, but would make transient source failures require operator action. Route-supervised failure keeps failure state auditable through the RuntimeBus, preserves hot-reload atomicity by treating Consumer crash as a Route lifecycle event rather than a Pipeline mutation, and centralizes restart policy outside Components.
