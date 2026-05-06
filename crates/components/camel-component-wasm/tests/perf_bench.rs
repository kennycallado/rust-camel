//! Performance benchmark for WASM per-call instantiation overhead.
//!
//! Run with: cargo test -p camel-component-wasm --test perf_bench -- --nocapture

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use camel_component_wasm::StateStore;
use camel_component_wasm::WasmConfig;
use camel_component_wasm::bindings::camel::plugin::types::{
    WasmBody, WasmExchange, WasmMessage, WasmPattern,
};
use camel_component_wasm::runtime::WasmRuntime;
use camel_core::Registry;

fn echo_wasm_path() -> PathBuf {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("examples/wasm-example/fixtures/echo.wasm");
    if base.exists() {
        return base;
    }
    panic!("echo.wasm not found at {:?}", base);
}

fn make_registry() -> Arc<Mutex<Registry>> {
    Arc::new(Mutex::new(Registry::new()))
}

fn make_exchange() -> WasmExchange {
    WasmExchange {
        input: WasmMessage {
            headers: vec![],
            body: WasmBody::Text("hello".to_string()),
        },
        output: None,
        properties: vec![],
        pattern: WasmPattern::InOnly,
        correlation_id: "bench".to_string(),
    }
}

#[tokio::test]
async fn bench_instantiation_cost() {
    let wasm_path = echo_wasm_path();
    let config = WasmConfig::default();

    // Measure one-time setup cost (Engine + Component + Linker)
    let setup_start = Instant::now();
    let runtime = WasmRuntime::new(&wasm_path, config.clone()).await.unwrap();
    let setup_time = setup_start.elapsed();
    println!(
        "One-time setup (Engine + Component + Linker): {:.2?}",
        setup_time
    );

    // Warm up
    let exchange = make_exchange();
    runtime
        .call_process(make_registry(), HashMap::new(), StateStore::new(), exchange)
        .await
        .unwrap();

    // Measure per-call cost (Store + instantiate + process)
    let iterations = 100u64;
    let call_start = Instant::now();
    for _ in 0..iterations {
        let exchange = make_exchange();
        runtime
            .call_process(make_registry(), HashMap::new(), StateStore::new(), exchange)
            .await
            .unwrap();
    }
    let total_call_time = call_start.elapsed();
    let per_call = total_call_time / iterations as u32;

    println!("Per-call (Store + instantiate + process): {:.2?}", per_call);
    println!(
        "Throughput: {:.0} exchanges/sec",
        iterations as f64 / total_call_time.as_secs_f64()
    );
    println!("Total for {} calls: {:.2?}", iterations, total_call_time);

    // Decision threshold
    let per_call_ms = per_call.as_secs_f64() * 1000.0;
    if per_call_ms < 1.0 {
        println!("DECISION: No pooling needed (< 1ms per call)");
    } else if per_call_ms > 5.0 {
        println!("DECISION: Pooling recommended (> 5ms per call)");
    } else {
        println!("DECISION: Borderline (1-5ms per call) — defer to user");
    }
}
