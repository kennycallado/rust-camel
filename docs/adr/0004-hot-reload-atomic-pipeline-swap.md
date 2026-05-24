# Hot Reload via Atomic Pipeline Swap

When a route's steps change but its `from:` URI stays the same, hot reload compiles the new Pipeline and swaps atomically via `ArcSwap`. The Consumer is never stopped. Each in-flight Exchange completes against the Pipeline snapshot it entered — `ArcSwap` guarantees readers hold a stable reference for the lifetime of their access, so old and new Pipelines can coexist momentarily without corruption.

A simpler stop/recreate approach would work but adds unnecessary downtime and Consumer reconnection overhead. The atomic swap keeps the Consumer running and makes the reload effectively zero-downtime for the common case. The tradeoff is more complexity in the reload diffing logic (`ReloadAction::Swap` vs `Restart`). There is no explicit drain step — snapshot isolation via `ArcSwap` makes it unnecessary.
