//! Spike evidence: wasmtime 46 concurrent API verification.
//! This file compiles but is never called — purely API discovery.
//! Branch: feature/wasm-streaming-spike, commit after this file.

use std::pin::Pin;
use std::task::{Context, Poll};
use wasmtime::component::{
    Accessor, Destination, StreamProducer, StreamReader, StreamResult, VecBuffer,
};
use wasmtime::StoreContextMut;

// === DISCOVERY 1: StreamProducer trait exact signature ===
// Verified: Destination by value (not &mut), finish: bool, VecBuffer<u8>, set_buffer()

pub struct SpikeProducer {
    data: Vec<u8>,
    pos: usize,
}

impl<D> StreamProducer<D> for SpikeProducer {
    type Item = u8;
    type Buffer = VecBuffer<u8>;

    fn poll_produce<'a>(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _store: StoreContextMut<'a, D>,
        mut destination: Destination<'a, Self::Item, Self::Buffer>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        if finish {
            return Poll::Ready(Ok(StreamResult::Cancelled));
        }
        if self.pos < self.data.len() {
            let chunk: VecBuffer<u8> = self.data[self.pos..].to_vec().into();
            self.pos = self.data.len();
            destination.set_buffer(chunk);
            Poll::Ready(Ok(StreamResult::Completed))
        } else {
            Poll::Ready(Ok(StreamResult::Dropped))
        }
    }
}

// === DISCOVERY 2: StreamReader::new via accessor.with() ===
// Accessor does NOT impl AsContextMut. Use accessor.with(|access| ...)
// which yields Access<'_, T> that DOES impl AsContextMut.
// Accessor::with takes &self (immutable).

pub fn spike_create_reader<D: 'static + Send>(
    accessor: &Accessor<D>,
) -> wasmtime::Result<StreamReader<u8>> {
    let producer = SpikeProducer { data: b"hello".to_vec(), pos: 0 };
    accessor.with(|mut access| StreamReader::new(&mut access, producer))
}

// === DISCOVERY 3: Store::run_concurrent ===
// Closure receives &Accessor<T> (immutable). Returns nested Result.

pub async fn spike_run_concurrent<T: 'static + Send>(
    store: &mut wasmtime::Store<T>,
) -> wasmtime::Result<()> {
    store.run_concurrent(async |accessor| {
        let _reader = spike_create_reader(accessor)?;
        wasmtime::Result::Ok(())
    }).await?
}

// === DISCOVERY 4: func_wrap_concurrent ===
pub fn spike_linker<T: 'static + Send>(
    engine: &wasmtime::Engine,
) -> wasmtime::Result<wasmtime::component::Linker<T>> {
    let mut linker = wasmtime::component::Linker::new(engine);
    linker.root().func_wrap_concurrent(
        "test-fn",
        |_accessor: &Accessor<T>, (): ()| {
            Box::pin(async move { Ok::<(), wasmtime::Error>(()) })
        },
    )?;
    Ok(linker)
}

// === DISCOVERY 5: Config requirement ===
pub fn spike_config() -> wasmtime::Config {
    let mut config = wasmtime::Config::new();
    config.epoch_interruption(true);
    config.concurrency_support(true); // REQUIRED for StreamReader/run_concurrent
    config
}
