# ws-server-binding Specification

## Purpose
TBD - created by archiving change ws-bound-address. Update Purpose after archive.
## Requirements
### Requirement: Listener-injected server lifecycle

`camel-component-ws` MUST accept a pre-bound TCP listener for both plain and TLS servers, expose the active registry entry's bound address to the caller, and key `ServerRegistry` entries by the listener's actual port. Existing `get_or_spawn`/`start` behavior is unchanged.

#### Scenario: port-0 listener yields the real bound address

- **Given** an empty `ServerRegistry`
- **When** a caller spawns a server via `get_or_spawn_with_listener` with a listener bound to `127.0.0.1:0`
- **Then** the returned `SocketAddr` carries the OS-assigned port, and a client connecting to that address reaches the server

#### Scenario: same actual port reuses one server

- **Given** a server already spawned via `get_or_spawn_with_listener` on actual port P
- **When** a second caller spawns on port P — either through `get_or_spawn_with_listener` with a handle to the same pre-bound socket, or through `get_or_spawn(host, P, …)` with matching TLS mode (mixed entry)
- **Then** no second server starts and no rebinding occurs; both holders share the existing entry (ref-count 2), and any address-return assertion applies to the `get_or_spawn_with_listener` return value (the unchanged `get_or_spawn` signature carries no address)

#### Scenario: stopping the injected consumer preserves the server

- **Given** a server spawned via an injected listener on port P
- **When** the consumer that injected the listener stops
- **Then** the process-lifetime server stays up (existing lifecycle semantics); a new consumer on port P reuses it without rebinding

#### Scenario: test-only reset clears injected entries

- **Given** a server spawned via an injected listener on port P
- **When** the test-only `ServerRegistry::reset()` runs
- **Then** injected entries are aborted and cleared like non-injected ones, and a subsequent spawn on port P can bind a fresh listener

#### Scenario: TLS-mode mismatch on the same port errors

- **Given** a plain server already spawned for actual port P
- **When** a caller spawns via `get_or_spawn_with_listener` requesting TLS on port P
- **Then** the call returns an error, matching existing `get_or_spawn` mismatch behavior

#### Scenario: consumer round-trips via injected listener without port guessing

- **Given** a test that binds `127.0.0.1:0`, reads `local_addr()`, and builds the endpoint URI from that real address
- **When** `WsConsumer::start_with_listener` starts the consumer with the live listener
- **Then** the server serves on the listener's address and a producer round-trips a message, with no free-port guess involved

#### Scenario: existing get_or_spawn path is unchanged

- **Given** any caller of the existing `get_or_spawn(host, port, …)` API
- **When** a server is spawned through it
- **Then** binding, `WsAppState` return, ref-counting, and error behavior match the pre-change implementation

### Requirement: ws lib tests do not guess ports

The `camel-component-ws` Rust library-test suite MUST obtain server addresses exclusively from bound listeners (bind-0 + `local_addr()` + injected listener), not from `free_port()`-style bind-read-drop probes.

#### Scenario: free_port is absent from the ws lib tests

- **Given** the merged change
- **When** searching `crates/components/camel-ws/src/lib.rs` for `free_port`
- **Then** zero matches exist: the helper is deleted and no callsite references it

