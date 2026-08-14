# redis-sentinel

A minimal example that connects a Redis producer to a Redis Sentinel topology
through the `redis-sentinel://` URI scheme. It writes a key every few seconds
and shows that the component resolves the current master through the sentinels
on every connection.

Prerequisites: a running Redis Sentinel topology (a master, one or more
replicas, and one or more sentinels). Provision one with Docker Compose or
testcontainers. This example does not start one itself.

Run it against your topology:

```text
REDIS_SENTINEL_NODES="127.0.0.1:26379" REDIS_MASTER_NAME="mymaster" cargo run -p redis-sentinel
```
