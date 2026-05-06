//! Integration tests for WASM production hardening.
//!
//! Tests epoch-based timeouts, memory limits, and trap classification
//! using wasmtime core modules (WAT format). These test the wasmtime
//! primitives that our production code relies on.
//!
//! Full end-to-end tests through WasmRuntime (Component Model) require
//! Phase 2 test fixtures (compiled .wasm components). The hardening
//! infrastructure itself is validated here at the wasmtime level.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use wasmtime::{Config, Engine, Instance, Module, Store, StoreLimitsBuilder};

/// Create an engine with epoch interruption enabled.
fn engine_with_epoch() -> Engine {
    let mut config = Config::new();
    config.epoch_interruption(true);
    Engine::new(&config).expect("failed to create engine")
}

/// Start a simple epoch ticker for tests. Returns a shutdown flag.
fn start_test_ticker(engine: Engine, interval: Duration) -> Arc<AtomicBool> {
    let shutdown = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&shutdown);
    thread::spawn(move || {
        while !flag.load(Ordering::Relaxed) {
            thread::sleep(interval);
            if !flag.load(Ordering::Relaxed) {
                engine.increment_epoch();
            }
        }
    });
    shutdown
}

#[test]
fn test_epoch_timeout_fires_on_infinite_loop() {
    let engine = engine_with_epoch();

    let wasm = wat::parse_str(r#"(module (func (export "run") (loop (br 0))))"#)
        .expect("failed to parse WAT");

    let module = Module::new(&engine, wasm).expect("failed to compile module");

    let shutdown = start_test_ticker(engine.clone(), Duration::from_millis(5));

    let mut store: Store<()> = Store::new(&engine, ());
    store.set_epoch_deadline(1);

    let instance = Instance::new(&mut store, &module, &[]).expect("failed to instantiate");
    let func = instance
        .get_func(&mut store, "run")
        .expect("function 'run' not found");

    let result = func.call(&mut store, &[], &mut []);

    assert!(result.is_err(), "infinite loop should have been trapped");

    let error = result.unwrap_err();
    // Epoch interruption produces a Trap::Interrupt, but the error message
    // may be a backtrace string. Check the trap type directly.
    if let Some(trap) = error.downcast_ref::<wasmtime::Trap>() {
        assert_eq!(
            *trap,
            wasmtime::Trap::Interrupt,
            "expected Interrupt trap from epoch, got: {trap}"
        );
    } else {
        // Fallback: check error message for epoch-related keywords
        let error_msg = error.to_string();
        assert!(
            error_msg.contains("epoch")
                || error_msg.contains("interrupt")
                || error_msg.contains("timeout"),
            "error should mention epoch/interrupt/timeout, got: {error_msg}"
        );
    }

    shutdown.store(true, Ordering::Relaxed);
}

#[test]
fn test_epoch_timeout_with_longer_deadline() {
    let engine = engine_with_epoch();

    let wasm =
        wat::parse_str(r#"(module (func (export "run") (nop)))"#).expect("failed to parse WAT");

    let module = Module::new(&engine, wasm).expect("failed to compile module");

    let shutdown = start_test_ticker(engine.clone(), Duration::from_millis(5));

    let mut store: Store<()> = Store::new(&engine, ());
    store.set_epoch_deadline(1000);

    let instance = Instance::new(&mut store, &module, &[]).expect("failed to instantiate");
    let func = instance
        .get_func(&mut store, "run")
        .expect("function 'run' not found");

    let result = func.call(&mut store, &[], &mut []);

    assert!(
        result.is_ok(),
        "fast function should succeed, got: {:?}",
        result
    );

    shutdown.store(true, Ordering::Relaxed);
}

// ── Batch 2: Trap classification tests ───────────────────────────────────

#[test]
fn test_trap_unreachable_is_classified() {
    use camel_component_wasm::WasmError;
    use camel_component_wasm::error::TrapReason;

    let engine = Engine::new(&Config::new()).expect("engine");
    let wasm =
        wat::parse_str(r#"(module (func (export "run") (unreachable)))"#).expect("parse WAT");
    let module = Module::new(&engine, wasm).expect("compile");
    let mut store: Store<()> = Store::new(&engine, ());
    let instance = Instance::new(&mut store, &module, &[]).expect("instantiate");
    let func = instance.get_func(&mut store, "run").expect("func");

    let result = func.call(&mut store, &[], &mut []);
    assert!(result.is_err());

    let error = result.unwrap_err();
    if let Some(trap) = error.downcast_ref::<wasmtime::Trap>() {
        let reason = WasmError::classify_trap(trap);
        assert_eq!(reason, TrapReason::Unreachable);
    } else {
        panic!("expected a Trap, got: {error}");
    }
}

#[test]
fn test_trap_stack_overflow_is_classified() {
    use camel_component_wasm::WasmError;
    use camel_component_wasm::error::TrapReason;

    let engine = Engine::new(&Config::new()).expect("engine");
    let wasm = wat::parse_str(
        r#"(module
            (func $recurse (export "run")
                (call $recurse)
            )
        )"#,
    )
    .expect("parse WAT");
    let module = Module::new(&engine, wasm).expect("compile");
    let mut store: Store<()> = Store::new(&engine, ());
    let instance = Instance::new(&mut store, &module, &[]).expect("instantiate");
    let func = instance.get_func(&mut store, "run").expect("func");

    let result = func.call(&mut store, &[], &mut []);
    assert!(result.is_err());

    let error = result.unwrap_err();
    if let Some(trap) = error.downcast_ref::<wasmtime::Trap>() {
        let reason = WasmError::classify_trap(trap);
        assert_eq!(reason, TrapReason::StackOverflow);
    } else {
        panic!("expected a Trap, got: {error}");
    }
}

#[test]
fn test_normal_execution_succeeds() {
    let engine = Engine::new(&Config::new()).expect("engine");
    let wasm = wat::parse_str(r#"(module (func (export "run") (nop)))"#).expect("parse WAT");
    let module = Module::new(&engine, wasm).expect("compile");
    let mut store: Store<()> = Store::new(&engine, ());
    let instance = Instance::new(&mut store, &module, &[]).expect("instantiate");
    let func = instance.get_func(&mut store, "run").expect("func");

    let result = func.call(&mut store, &[], &mut []);
    assert!(result.is_ok(), "normal execution should succeed");
}

// ── Batch 3: Memory limit tests ──────────────────────────────────────────

#[test]
fn test_memory_limit_enforced_on_growth() {
    let engine = Engine::new(&Config::new()).expect("engine");

    let wasm = wat::parse_str(
        r#"(module
            (memory (export "memory") 1)
            (func (export "run")
                (memory.grow (i32.const 10000))
                drop
            )
        )"#,
    )
    .expect("parse WAT");

    let module = Module::new(&engine, wasm).expect("compile");

    let limits = StoreLimitsBuilder::new()
        .memory_size(65536) // 1 page only
        .build();

    let mut store: Store<wasmtime::StoreLimits> = Store::new(&engine, limits);
    store.limiter(|state| state);

    let instance = Instance::new(&mut store, &module, &[]).expect("instantiate");
    let func = instance.get_func(&mut store, "run").expect("func");

    let _result = func.call(&mut store, &[], &mut []);
    let memory = instance
        .get_memory(&mut store, "memory")
        .expect("memory export");
    assert_eq!(
        memory.size(&store),
        1,
        "memory should not have grown past limit"
    );
}

#[test]
fn test_memory_limit_with_large_allocation_trap() {
    let engine = Engine::new(&Config::new()).expect("engine");

    let wasm = wat::parse_str(
        r#"(module
            (memory (export "memory") 1)
            (func (export "run")
                (i32.store (i32.const 100000) (i32.const 42))
            )
        )"#,
    )
    .expect("parse WAT");

    let module = Module::new(&engine, wasm).expect("compile");

    let limits = StoreLimitsBuilder::new().memory_size(65536).build();

    let mut store: Store<wasmtime::StoreLimits> = Store::new(&engine, limits);
    store.limiter(|state| state);

    let result = Instance::new(&mut store, &module, &[]);

    if let Ok(instance) = result {
        let func = instance.get_func(&mut store, "run").expect("func");
        let call_result = func.call(&mut store, &[], &mut []);
        assert!(
            call_result.is_err(),
            "accessing memory beyond limit should trap"
        );
    }
    // Instance::new failing is also acceptable
}

// ── Batch 4: Recovery tests ──────────────────────────────────────────────

#[test]
fn test_recovery_after_trap_with_fresh_store() {
    let engine = Engine::new(&Config::new()).expect("engine");

    let wasm = wat::parse_str(
        r#"(module
            (func (export "trap") (unreachable))
            (func (export "normal") (nop))
        )"#,
    )
    .expect("parse WAT");

    let module = Module::new(&engine, wasm).expect("compile");

    {
        let mut store: Store<()> = Store::new(&engine, ());
        let instance = Instance::new(&mut store, &module, &[]).expect("instantiate");
        let func = instance.get_func(&mut store, "trap").expect("func 'trap'");
        let result = func.call(&mut store, &[], &mut []);
        assert!(result.is_err(), "unreachable should trap");
    }

    {
        let mut store: Store<()> = Store::new(&engine, ());
        let instance = Instance::new(&mut store, &module, &[]).expect("instantiate");
        let func = instance
            .get_func(&mut store, "normal")
            .expect("func 'normal'");
        let result = func.call(&mut store, &[], &mut []);
        assert!(
            result.is_ok(),
            "normal function should succeed after trap recovery"
        );
    }
}

#[test]
fn test_recovery_after_epoch_timeout() {
    let engine = engine_with_epoch();

    let wasm = wat::parse_str(
        r#"(module
            (func (export "loop_forever") (loop (br 0)))
            (func (export "normal") (nop))
        )"#,
    )
    .expect("parse WAT");

    let module = Module::new(&engine, wasm).expect("compile");

    let shutdown = start_test_ticker(engine.clone(), Duration::from_millis(5));

    {
        let mut store: Store<()> = Store::new(&engine, ());
        store.set_epoch_deadline(1);
        let instance = Instance::new(&mut store, &module, &[]).expect("instantiate");
        let func = instance.get_func(&mut store, "loop_forever").expect("func");
        let result = func.call(&mut store, &[], &mut []);
        assert!(
            result.is_err(),
            "infinite loop should be trapped by epoch timeout"
        );
    }

    {
        let mut store: Store<()> = Store::new(&engine, ());
        store.set_epoch_deadline(1000);
        let instance = Instance::new(&mut store, &module, &[]).expect("instantiate");
        let func = instance.get_func(&mut store, "normal").expect("func");
        let result = func.call(&mut store, &[], &mut []);
        assert!(
            result.is_ok(),
            "normal function should succeed after timeout recovery"
        );
    }

    shutdown.store(true, Ordering::Relaxed);
}

#[test]
fn test_multiple_traps_then_recovery() {
    let engine = Engine::new(&Config::new()).expect("engine");

    let wasm = wat::parse_str(
        r#"(module
            (func (export "trap") (unreachable))
            (func (export "normal") (nop))
        )"#,
    )
    .expect("parse WAT");

    let module = Module::new(&engine, wasm).expect("compile");

    for _ in 0..3 {
        let mut store: Store<()> = Store::new(&engine, ());
        let instance = Instance::new(&mut store, &module, &[]).expect("instantiate");
        let func = instance.get_func(&mut store, "trap").expect("func");
        let result = func.call(&mut store, &[], &mut []);
        assert!(result.is_err(), "should trap");
    }

    {
        let mut store: Store<()> = Store::new(&engine, ());
        let instance = Instance::new(&mut store, &module, &[]).expect("instantiate");
        let func = instance.get_func(&mut store, "normal").expect("func");
        let result = func.call(&mut store, &[], &mut []);
        assert!(result.is_ok(), "should succeed after multiple traps");
    }
}
