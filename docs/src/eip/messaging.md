# Messaging

Messaging patterns change the cardinality or order of exchanges. They split one message into many, gather many into one, reorder a sequence, or sample one in every N. Several compose into split-process-aggregate flows.

- [Aggregator](aggregator.md) — collect exchanges by correlation key and emit a batch
- [Splitter](splitter.md) — break a composite message into one exchange per fragment
- [Streaming Splitter](streaming-splitter.md) — split a byte-stream body into fragment exchanges with backpressure
- [Zip Splitter](zip-splitter.md) — split a ZIP archive into one exchange per entry
- [Resequencer](resequencer.md) — reorder exchanges by sequence number in batches or streams
- [Sort](sort.md) — sort an array body by a comparator expression
- [Sampling](sampling.md) — pass through one out of every N exchanges
- [Claim Check](claim-check.md) — stash a large payload and replace it with a claim ticket

For the processor contract that implements these patterns, see [camel-processor/CONTEXT.md](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-processor/CONTEXT.md).
