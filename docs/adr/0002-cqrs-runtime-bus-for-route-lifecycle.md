# CQRS RuntimeBus for Route Lifecycle Control

Route lifecycle mutations go through `RuntimeCommandBus` and reads through `RuntimeQueryBus`, backed by projections and an optional redb event journal. Direct mutable state would be simpler, but the CQRS model gives us crash recovery, command deduplication, and projection-backed reads that don't block the command path.

The added architectural weight is justified because route lifecycle operations (start, stop, hot-reload) are infrequent, and the journal makes the control plane auditable and recoverable without adding a separate database dependency.
