# Functions

Out-of-process executable units invoked as `function:` steps in a Route. A Function runs in an isolated container, receives an Exchange snapshot, and returns an ExchangePatch.

## Language

**Function**:
A named, out-of-process executable unit invoked as a `function:` Step in a Route. Receives an Exchange snapshot, runs user logic, and returns an Exchange patch.
_Avoid_: script (use `script:` for in-process synchronous evaluation), lambda, handler

**ContainerProvider**:
The Docker-based execution environment for Functions. Manages container lifecycle (spawn, warm pool, health check) for each registered Function. Configured at the runtime level, not in the route definition.
_Avoid_: runtime, executor, runner

**FunctionInvoker**:
Runtime contract for invoking a Function from within a Pipeline step. The `function:` Step calls the FunctionInvoker with an Exchange and applies the returned ExchangePatch.
_Avoid_: function client, caller

**FunctionDefinition**:
The complete specification of a Function: runtime identifier, source code, timeout, and optional route/step context. Created by the `function:` step compiler from DSL configuration.
_Avoid_: function config, function spec

**FunctionId**:
A content-derived identifier for a FunctionDefinition, computed from runtime, source, and timeout. Stable across identical definitions — same code and config always yields the same FunctionId.
_Avoid_: function name, function hash

**ExchangePatch**:
The mutation returned by a Function execution — a partial update specifying body replacement, header additions/removals, and property changes. Applied to the Exchange after the Function completes.
_Avoid_: response, result, diff

## Example dialogue

> "What is the difference between `script:` and `function:`?"
> "`script:` is synchronous, in-process evaluation (Boa JS, Rhai, etc.) — for predicates and simple transformations. `function:` is out-of-process — use it when you need async I/O, npm packages, or logic that belongs outside the pipeline."
>
> "Does the Function see the full Exchange?"
> "It receives a snapshot of body, headers, and properties. It returns an ExchangePatch — it cannot access CamelContext or send to other Routes directly from inside the Function."
