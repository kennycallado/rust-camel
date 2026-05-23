# Hot Reload via Atomic Pipeline Swap After Drain

When a route's steps change but its `from:` URI stays the same, hot reload compiles the new Pipeline, drains in-flight Exchanges on the old one, then swaps atomically via `ArcSwap`. The Consumer is never stopped.

A simpler stop/recreate approach would work but adds unnecessary downtime and Consumer reconnection overhead. The atomic swap keeps the Consumer running and makes the reload effectively zero-downtime for the common case. The tradeoff is more complexity in the reload diffing logic (`ReloadAction::Swap` vs `Restart`) and the requirement that in-flight Exchanges must drain before the swap.
