//! Shared fixtures for the data-format unit tests (`tar`, `tar.gz`, `gzip`).
//!
//! Every helper here is `#[cfg(test)]`-gated through the module declaration in
//! `mod.rs`; nothing ships in release builds. The `warn_capture` recorder is a
//! deliberate copy of `error_handler::tests::log_capture` (that module keeps
//! its own — do not merge them).

use bytes::Bytes;
use camel_api::body::{Body, StreamBody, StreamMetadata};
use camel_api::error::CamelError;
use std::sync::Arc;

/// Test-only log capture: installs a minimal subscriber via
/// `tracing::subscriber::with_default` for the duration of one closure and
/// records WARN-level (and above) event fields. No global state — safe
/// under parallel test threads.
pub(super) mod warn_capture {
    use std::fmt;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Record};
    use tracing::{Event, Id, Level, Metadata, Subscriber};

    type Sink = Arc<Mutex<Vec<String>>>;

    struct Recorder {
        events: Sink,
        next_span_id: AtomicU64,
    }

    struct FieldVisitor(String);

    impl Visit for FieldVisitor {
        fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
            if !self.0.is_empty() {
                self.0.push(' ');
            }
            let _ = fmt::write(&mut self.0, format_args!("{}={:?}", field.name(), value));
        }
    }

    impl Subscriber for Recorder {
        fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _attrs: &Attributes<'_>) -> Id {
            Id::from_u64(self.next_span_id.fetch_add(1, Ordering::Relaxed) + 1)
        }

        fn record(&self, _span: &Id, _values: &Record<'_>) {}
        fn record_follows_from(&self, _span: &Id, _follows_from: &Id) {}

        fn event(&self, event: &Event<'_>) {
            if *event.metadata().level() >= Level::WARN {
                let mut visitor = FieldVisitor(String::new());
                event.record(&mut visitor);
                if let Ok(mut slot) = self.events.lock() {
                    slot.push(visitor.0);
                }
            }
        }

        fn enter(&self, _span: &Id) {}
        fn exit(&self, _span: &Id) {}
    }

    /// Runs `f` with a capturing subscriber installed and returns
    /// `(f's result, captured event field strings)` in emission order.
    pub(crate) fn capture_warns<T>(f: impl FnOnce() -> T) -> (T, Vec<String>) {
        let sink: Sink = Default::default();
        let recorder = Recorder {
            events: Arc::clone(&sink),
            next_span_id: AtomicU64::default(),
        };
        let out = tracing::subscriber::with_default(recorder, f);
        let collected = sink
            .lock()
            .ok()
            .map(|slot| slot.clone())
            .unwrap_or_default();
        (out, collected)
    }
}

pub(super) use warn_capture::capture_warns;

/// Shared slot holding the optional byte stream behind `Body::Stream`.
pub(super) type StreamSlot =
    Arc<tokio::sync::Mutex<Option<futures::stream::BoxStream<'static, Result<Bytes, CamelError>>>>>;

/// Builds a `Body::Stream` plus its shared slot so tests can assert the
/// stream was not consumed by a rejecting code path.
pub(super) fn stream_body_pair() -> (Body, StreamSlot) {
    let slot: StreamSlot = Arc::new(tokio::sync::Mutex::new(Some(Box::pin(
        futures::stream::iter(vec![Ok(Bytes::from_static(b"data"))]),
    ))));
    let body = Body::Stream(StreamBody {
        stream: Arc::clone(&slot),
        metadata: StreamMetadata::default(),
    });
    (body, slot)
}

/// Asserts a data-format output body is `Body::Bytes` equal to `expected`.
pub(super) fn assert_bytes(body: Body, expected: &[u8]) {
    match body {
        Body::Bytes(b) => assert_eq!(b.as_ref(), expected),
        _ => panic!("expected Body::Bytes"),
    }
}

// ustar typeflag bytes used to hand-build fixture entries.
pub(super) const REGULAR: u8 = b'0';
pub(super) const HARDLINK: u8 = b'1';
pub(super) const SYMLINK: u8 = b'2';
pub(super) const CHARACTER_DEVICE: u8 = b'3';
pub(super) const DIRECTORY: u8 = b'5';

/// Entry name used by the TAR/`tar.gz` marshal output.
pub(super) const ENTRY_NAME: &str = "payload";

/// Builds a raw 512-byte ustar header block. `Header::set_path` refuses
/// malicious paths, so fixtures modeling attacker archives assemble
/// headers by hand.
pub(super) fn raw_header(name: &str, typeflag: u8, size: u64) -> Vec<u8> {
    let mut block = vec![0u8; 512];
    block[..name.len()].copy_from_slice(name.as_bytes());
    block[100..108].copy_from_slice(b"0000644\0");
    block[108..116].copy_from_slice(b"0000000\0");
    block[116..124].copy_from_slice(b"0000000\0");
    block[124..136].copy_from_slice(format!("{:011o}\0", size).as_bytes());
    block[136..148].copy_from_slice(b"00000000000\0");
    block[148..156].copy_from_slice(b"        "); // checksum placeholder
    block[156] = typeflag;
    block[257..263].copy_from_slice(b"ustar\0");
    block[263..265].copy_from_slice(b"00");
    let sum: u32 = block.iter().map(|&b| b as u32).sum();
    block[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
    block
}

fn push_raw_entry(buf: &mut Vec<u8>, name: &str, typeflag: u8, data: &[u8]) {
    buf.extend_from_slice(&raw_header(name, typeflag, data.len() as u64));
    buf.extend_from_slice(data);
    let pad = (512 - (data.len() % 512)) % 512;
    buf.extend(std::iter::repeat_n(0u8, pad));
}

/// Builds an in-memory TAR archive from `(name, typeflag, data)` tuples.
pub(super) fn make_tar(entries: &[(&str, u8, &[u8])]) -> Vec<u8> {
    let mut buf = Vec::new();
    for (name, typeflag, data) in entries {
        push_raw_entry(&mut buf, name, *typeflag, data);
    }
    buf.extend_from_slice(&[0u8; 1024]); // end-of-archive
    buf
}

/// Builds a canonical single-entry archive via `tar::Builder`, mirroring
/// the marshal output shape.
pub(super) fn make_tar_payload(content: &[u8]) -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut buf);
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, ENTRY_NAME, content)
            .unwrap();
        builder.finish().unwrap();
    }
    buf
}
