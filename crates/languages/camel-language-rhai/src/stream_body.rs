//! Access-aware stream-body refusal (change `language-value-boundary`,
//! task 2.2, boundary B1).
//!
//! A `Body::Stream` binds into the rhai scope as a [`StreamBodyRef`] marker —
//! no stream data and no stream handle, just per-evaluation read/hit
//! counters. The refusal contract:
//!
//! - **Guards** are registered for `to_string`, `to_debug` and the operators
//!   `== != < > + - * /` over `(marker, marker)` and mixed `(marker, T)` /
//!   `(T, marker)` scalar shapes. Each guard increments `hits` by its
//!   marker-operand count and returns the private typed guard sentinel
//!   inside an `ErrorRuntime`.
//! - **Clones count reads**: the marker's manual `Clone` fires for every
//!   materialization (captures, aliases, array nesting, function arguments,
//!   operand and result materialization).
//! - **POST-EVAL RULE** (authoritative): on eval `Ok`, refuse with the
//!   conversion error IFF `reads > hits` — guard failures handled in-script
//!   (try/catch) are forgiven; raw materializations are not. On eval `Err`,
//!   only the typed guard sentinel and the operand-type portions of the
//!   `ErrorFunctionNotFound`/`ErrorIndexingType` FIELDS map to the conversion
//!   error — `ErrorRuntime` payloads and `Display` text are never
//!   text-matched, so `throw "StreamBodyRef"` stays a redacted runtime error.
//!
//! Documented residue: an operation outside the guard surface that is
//! CAUGHT in-script still fails post-eval (rhai exposes no caught-error
//! hook). Documented exceptions (verified reads=0 under the `Simple` exec
//! AST): `type_of(marker)` returns `"StreamBodyRef"`, and a bare discarded
//! statement is optimized away. See `docs/src/languages/rhai.md`.

use camel_language_api::LanguageError;
use rhai::{Engine, EvalAltResult};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Private guard-sentinel payload. Every registered guard returns this typed
/// value inside an `ErrorRuntime`; [`map_stream_refusal`] recognizes it by
/// TYPE, never by rendered text.
///
/// The type is never registered on the engine and is not constructible from a
/// script, so no `throw` value can imitate it — a user `throw` of any string,
/// including the literal `"STREAM_SENTINEL"` once used as the sentinel, stays
/// a redacted runtime error.
#[derive(Debug, Clone, Copy)]
struct StreamSentinel;

/// Registered name of the marker type (`type_of(marker)` returns this).
pub(crate) const TYPE_NAME: &str = "StreamBodyRef";

/// Generic source label for every stream-body refusal.
const SOURCE_TYPE: &str = "Body::Stream";

/// The stream-body marker bound as the `body` scope variable.
///
/// Holds no stream data and no stream handle. The manual `Clone` increments
/// [`Self::reads`]: every clone is a materializing read of the stream body.
#[derive(Debug)]
pub(crate) struct StreamBodyRef {
    reads: Arc<AtomicU64>,
    hits: Arc<AtomicU64>,
}

impl StreamBodyRef {
    /// A fresh marker with zeroed counters — one per evaluation.
    pub(crate) fn new() -> Self {
        Self {
            reads: Arc::new(AtomicU64::new(0)),
            hits: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Record one guard hit (one marker operand served by a guard).
    fn hit(&self) {
        self.hits.fetch_add(1, Ordering::Relaxed);
    }
}

impl Clone for StreamBodyRef {
    fn clone(&self) -> Self {
        // Materializing read: capture, alias, block local, array nesting,
        // function argument, operand or result materialization.
        self.reads.fetch_add(1, Ordering::Relaxed);
        Self {
            reads: Arc::clone(&self.reads),
            hits: Arc::clone(&self.hits),
        }
    }
}

/// Handles to a marker's counters, taken BEFORE the marker enters the scope
/// so the post-eval rule can observe the evaluation without cloning the
/// marker (plain `Arc` clones never touch the counters).
#[derive(Debug, Clone)]
pub(crate) struct StreamCounters {
    reads: Arc<AtomicU64>,
    hits: Arc<AtomicU64>,
}

impl StreamCounters {
    /// Borrow the counters of a marker-typed `Dynamic` without cloning the
    /// marker (`None` for any other value).
    pub(crate) fn of_dynamic(d: &rhai::Dynamic) -> Option<Self> {
        let marker = d.read_lock::<StreamBodyRef>()?;
        Some(Self {
            reads: Arc::clone(&marker.reads),
            hits: Arc::clone(&marker.hits),
        })
    }

    /// POST-EVAL RULE: refuse IFF the script materialized the stream more
    /// often than registered guards (i.e. caught-in-script guard failures)
    /// forgave it.
    pub(crate) fn refuse_if_unforgiven(&self, target: &str) -> Result<(), LanguageError> {
        if self.reads.load(Ordering::Relaxed) > self.hits.load(Ordering::Relaxed) {
            Err(conversion_error(target))
        } else {
            Ok(())
        }
    }
}

/// The typed stream-body conversion refusal.
pub(crate) fn conversion_error(target: &str) -> LanguageError {
    LanguageError::ConversionError {
        source_type: SOURCE_TYPE.to_string(),
        target: target.to_string(),
    }
}

/// The guard-sentinel error returned by every registered guard.
fn sentinel_error() -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(
        rhai::Dynamic::from(StreamSentinel),
        rhai::Position::NONE,
    ))
}

/// Whether an `ErrorFunctionNotFound` signature names the marker as an
/// OPERAND TYPE.
///
/// The signature has the shape `callee (Type, Type, ...)` generated by rhai's
/// `gen_fn_call_signature`. Only the parenthesized operand-type list is
/// inspected; each token must equal [`TYPE_NAME`] exactly after stripping an
/// optional `&`/`&mut` reference marker. This rejects both a callee name that
/// merely contains the marker name (for example `not_StreamBodyRef_fn()`)
/// and a longer user type name such as `StreamBodyRefFoo`.
fn signature_mentions_marker(sig: &str) -> bool {
    let Some(open) = sig.find('(') else {
        return false;
    };
    let Some(close) = sig.rfind(')') else {
        return false;
    };
    if close <= open {
        return false;
    }
    sig[open + 1..close].split(',').any(|token| {
        let token = token.trim();
        let token = token
            .strip_prefix("&mut ")
            .or_else(|| token.strip_prefix('&'))
            .unwrap_or(token);
        token == TYPE_NAME
    })
}

/// Stream-aware eval error mapping: the typed guard sentinel and the
/// operand-type portions of the `ErrorFunctionNotFound`/`ErrorIndexingType`
/// fields naming the marker map to the conversion error; everything else
/// falls through to the task 1.7 mapper.
///
/// `ErrorRuntime` payloads and all free `Display` text are NEVER text-matched
/// — the sentinel is a private type, and a thrown string that happens to
/// contain `StreamBodyRef` stays class `runtime`, redacted.
pub(crate) fn map_stream_refusal(e: &EvalAltResult, target: &str) -> Option<LanguageError> {
    let mentions_marker = match e.unwrap_inner() {
        // (i) the dedicated guard sentinel — recognized by private TYPE only.
        EvalAltResult::ErrorRuntime(payload, _) => payload.is::<StreamSentinel>(),
        // (ii) structured variant TYPE fields only (never Display text):
        // the operand-type portion of the function signature / the indexer
        // type list.
        EvalAltResult::ErrorFunctionNotFound(sig, _) => signature_mentions_marker(sig),
        EvalAltResult::ErrorIndexingType(types, _) => types.contains(TYPE_NAME),
        _ => false,
    };
    mentions_marker.then(|| conversion_error(target))
}

/// Register the marker type and its refusal guards on an engine.
///
/// Guards are registered by-value so the engine's operand materialization is
/// visible in `reads`; each guard then increments `hits` by its marker-
/// operand count (unary +1; binary +1 per marker operand) and returns the
/// sentinel. This enumerated set IS the registered guard surface: anything
/// else fails with a structured engine error (`ErrorFunctionNotFound` /
/// `ErrorIndexingType`) classified by the type-field rule above.
pub(crate) fn register_stream_guards(engine: &mut Engine) {
    engine.register_type_with_name::<StreamBodyRef>(TYPE_NAME);

    // Unary materializing methods: the receiver clone counts the read, the
    // guard counts the hit, the sentinel refuses.
    engine.register_fn(
        "to_string",
        |m: StreamBodyRef| -> Result<rhai::Dynamic, Box<EvalAltResult>> {
            m.hit();
            Err(sentinel_error())
        },
    );
    engine.register_fn(
        "to_debug",
        |m: StreamBodyRef| -> Result<rhai::Dynamic, Box<EvalAltResult>> {
            m.hit();
            Err(sentinel_error())
        },
    );

    fn register_pair_guards<T: rhai::Variant + Clone>(engine: &mut Engine, op: &str) {
        engine.register_fn(
            op,
            move |a: StreamBodyRef, _b: T| -> Result<rhai::Dynamic, Box<EvalAltResult>> {
                a.hit();
                Err(sentinel_error())
            },
        );
        engine.register_fn(
            op,
            move |_a: T, b: StreamBodyRef| -> Result<rhai::Dynamic, Box<EvalAltResult>> {
                b.hit();
                Err(sentinel_error())
            },
        );
    }

    const OPS: [&str; 8] = ["==", "!=", "<", ">", "+", "-", "*", "/"];
    for op in OPS {
        // (marker, marker): hit by marker-operand count (two).
        engine.register_fn(
            op,
            |a: StreamBodyRef, b: StreamBodyRef| -> Result<rhai::Dynamic, Box<EvalAltResult>> {
                a.hit();
                b.hit();
                Err(sentinel_error())
            },
        );
        register_pair_guards::<i64>(engine, op);
        register_pair_guards::<f64>(engine, op);
        register_pair_guards::<String>(engine, op);
        register_pair_guards::<bool>(engine, op);
    }
}
