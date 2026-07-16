# ADR-0042: Arc<[CompiledStep]> shared snapshot for pipeline steps

## Status
Accepted

## Context
`SequentialPipeline` and `TracedPipeline` held `Vec<CompiledStep>` and cloned
the entire Vec (including all boxed services) per Exchange in `call()`. Each
`BoxProcessor` (a `BoxCloneService`) may carry inner service state, so the
per-Exchange Vec clone is not a cheap internal Arc bump — it is a real
allocation and state-copy.

## Decision
Store steps as `Arc<[CompiledStep]>` (wrapped in a `SharedSnapshot` newtype
that adds the necessary `Send`/`Sync` impls). `call()` does `Arc::clone`
(refcount bump). `run_steps` takes `Arc<[CompiledStep]>` by value, iterates
by reference, cloning only the single step being invoked.

## Consequences
- In-flight Exchanges hold an Arc to the old snapshot during hot-reload swap.
  Snapshot isolation (ADR-0004) is strengthened, not weakened: the old Arc
  is dropped only after every in-flight `run_steps` future completes.
- Lifecycle ownership remains a `PipelineAssembly` concern. The newtype is
  private to the compiler module.
- No observable behavior change — only allocation cost reduced.

## Safety
`CompiledStep` contains `BoxProcessor` (`BoxCloneService`) and
`Box<dyn OutcomePipeline>`, both `Send + !Sync`. The std `Arc<T>: Send`
bound therefore fails to hold. We work around it with a private newtype
in `route_compiler.rs`:

- `SharedSnapshot(Arc<[CompiledStep]>)` — adds unconditional `Send`/`Sync`
  impls. The `!Sync` on the inner types is an artifact of Tower's
  trait-object bounds (`Box<dyn ... + Send>` lacks `+ Sync`), NOT a sign
  of interior mutability. Concurrent `&CompiledStep` access is sound
  because `run_steps` only reads shared references and `.clone()`s owned
  copies before invoking.

  The `INVARIANT` in the struct doc — no `Rc`/`RefCell`/`Cell`/
  `UnsafeCell` in `CompiledStep` — is what makes the `unsafe impl Send`
  + `Sync` sound. If any variant ever introduces interior mutability, this
  becomes UB.

### Why by-value `SharedSnapshot`, not `&[CompiledStep]`

An earlier sketch took `steps: &[CompiledStep]` instead of
`steps: SharedSnapshot`. That was rejected: the resulting `run_steps`
future would borrow from a `&[CompiledStep]` with a non-`'static` lifetime,
which is incompatible with the `BoxCloneService::Future` associated type
`Pin<Box<dyn Future + Send>>` (a `Box<dyn Future + Send>` is implicitly
`'static`-bounded by trait-object vtable semantics). The future returned
by `Service::call` must be `'static` so the executor can own it without
lifetime plumbing.

By-value `SharedSnapshot` lets the future own the `Arc` allocation
naturally — clone of the newtype is a refcount bump, and the future drops
the `Arc` on completion, keeping the allocation alive for the future's
full lifetime without any lifetime annotations.
