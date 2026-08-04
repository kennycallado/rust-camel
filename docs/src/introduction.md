# Introduction

rust-camel is a Rust-native integration framework for routing messages between
timers, APIs, files, brokers, databases, and other systems. Routes may be
authored with a fluent Rust builder or a declarative YAML DSL.

The framework is inspired by Apache Camel's routing model, but is not a
drop-in implementation. Its data plane composes processors and producers as
Tower services, while its control plane manages components, endpoints,
consumers, and route lifecycle.

This guide is the home for concepts, tutorials, recipes, and operational
guidance. Use the [API reference](api-reference.md) for Rust contracts and the
[repository examples](https://github.com/kennycallado/rust-camel/tree/main/examples)
for runnable integrations.
