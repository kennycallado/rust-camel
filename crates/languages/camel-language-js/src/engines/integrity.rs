//! Integrity baseline for the worker's stable realm — capture and
//! verification of the named integrity-set roots.
//!
//! The worker verifies, after every evaluation, the own keys (including
//! symbol keys), property descriptors, prototype identity, and expected
//! value identity of a named root set against a baseline captured right
//! after the initial `camel`/`console` install on a fresh realm. On any
//! drift the worker recycles the whole realm — it never attempts
//! restoration.
//!
//! Named root set: the `globalThis` baseline own keys, the `eval`
//! function, and the prototypes of `Object`, `Array`, and `Function`.
//! The `String`/`Number`/`Boolean` prototype trio was the declared shrink
//! reserve and HAS been dropped: the pre-shrink detector measured ~47 µs
//! (46.8–47.0 across runs) per `integrity_check` in the release profile —
//! 1.9× the entire 25 µs per-eval budget — conclusively meeting the
//! shrink condition of the js-engine-cache spec (Requirement:
//! Integrity-set recycling), which is mandatory, not discretionary.

use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;

use boa_engine::property::PropertyDescriptor;
use boa_engine::property::PropertyKey;
use boa_engine::{Context, JsObject, JsValue, js_string};

/// Snapshot of one named integrity-set root object, captured on a pristine
/// realm and re-verified after every evaluation.
struct RootSnapshot {
    /// Diagnostic name (e.g. `"eval"`, `"Object.prototype"`).
    name: &'static str,
    /// The captured root object; its identity anchors the check (intrinsics
    /// are compared through this handle, not by name lookup).
    object: JsObject,
    /// Baseline own keys (string AND symbol keys).
    keys: Vec<PropertyKey>,
    /// Identity-relevant baseline values per key, positionally parallel to
    /// `keys` (data descriptor: the value; accessor: the get/set
    /// functions). Compared with [`JsValue::strict_equals`], never with
    /// structural equality — object identity is the point.
    identity: Vec<Vec<JsValue>>,
    /// `DefaultHasher` fold over each key's own-property descriptor fields:
    /// descriptor presence, kind, enumerable/configurable/writable flags,
    /// and the strict-equality identity results against `identity`
    /// (trivially all `true` at capture time, so any later value
    /// replacement folds a different hash).
    descriptors_hash: u64,
    /// Baseline `[[Prototype]]`, identity-compared via [`JsObject::equals`].
    proto: Option<JsObject>,
}

impl RootSnapshot {
    /// Capture the snapshot of `object`'s CURRENT state.
    fn capture(name: &'static str, object: JsObject, ctx: &mut Context) -> Self {
        // `own_property_keys` can only fail through exotic (proxy) internal
        // methods; the roots are never proxies. An empty key set would make
        // every later check report drift and force a recycle.
        let keys = object.own_property_keys(ctx).unwrap_or_default();
        let identity = keys
            .iter()
            .map(|key| {
                object
                    .borrow()
                    .properties()
                    .get(key)
                    .map(|desc| identity_values(&desc))
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>();
        let descriptors_hash = descriptors_hash(&object, &keys, &identity);
        let proto = object.prototype();
        Self {
            name,
            object,
            keys,
            identity,
            descriptors_hash,
            proto,
        }
    }

    /// Recompute every snapshot field over the object's CURRENT state and
    /// compare against the baseline. `false` on any difference.
    fn matches(&self, ctx: &mut Context) -> bool {
        let Ok(keys) = self.object.own_property_keys(ctx) else {
            return false;
        };
        if !key_sets_equal(&keys, &self.keys) {
            return false;
        }
        if !protos_equal(self.object.prototype(), self.proto.as_ref()) {
            return false;
        }
        descriptors_hash(&self.object, &self.keys, &self.identity) == self.descriptors_hash
    }
}

/// Integrity baseline captured once per realm generation — AFTER the
/// initial `camel`/`console` install — and verified by
/// [`IntegrityBaseline::verify`] after every evaluation.
pub(super) struct IntegrityBaseline {
    /// Own keys of the global object. `camel`/`console` stay in this set
    /// (their deletion counts as drift) but their VALUES are excluded from
    /// identity comparison: they are re-installed fresh every eval by
    /// design, so their presence and pristinity are enforced by
    /// install-verify, not by this detector.
    global_keys: Vec<PropertyKey>,
    /// Named roots: the `eval` function and the prototypes of `Object`,
    /// `Array`, and `Function` (Requirement-4 shrunk set — see the module
    /// documentation for the measured basis).
    roots: Vec<RootSnapshot>,
}

impl IntegrityBaseline {
    /// The empty baseline (degenerate capture-failure state; forces drift
    /// detection and a recycle on the next check).
    pub(super) fn empty() -> Self {
        Self {
            global_keys: Vec::new(),
            roots: Vec::new(),
        }
    }

    /// Capture the baseline for the CURRENTLY ACTIVE realm. Soundness: must
    /// run after the initial `camel`/`console` install so those keys are
    /// part of `global_keys` — the per-eval reinstall then only replaces
    /// their VALUES, which are excluded from identity comparison anyway.
    pub(super) fn capture(ctx: &mut Context) -> Self {
        let global = ctx.global_object();
        let global_keys = global.own_property_keys(ctx).unwrap_or_default();

        // The `eval` function: the value under the own key `"eval"`.
        let eval_object = global
            .get(js_string!("eval"), ctx)
            .ok()
            .and_then(|v| v.as_object());

        let mut roots = Vec::with_capacity(4);
        if let Some(eval_object) = eval_object {
            roots.push(RootSnapshot::capture("eval", eval_object, ctx));
        }
        // Requirement-4 shrink applied: the `String`/`Number`/`Boolean`
        // prototype trio (the declared reserve) is NOT captured — the
        // pre-shrink detector cost (~47 µs per check, release profile)
        // exceeded the entire 25 µs per-eval budget. See the module
        // documentation.
        let constructors = ctx.intrinsics().constructors();
        for (name, proto) in [
            ("Object.prototype", constructors.object().prototype()),
            ("Array.prototype", constructors.array().prototype()),
            ("Function.prototype", constructors.function().prototype()),
        ] {
            roots.push(RootSnapshot::capture(name, proto, ctx));
        }

        Self { global_keys, roots }
    }

    /// The captured `eval` function object (identity anchor for
    /// install-verify and the integrity check). `None` only on the
    /// degenerate empty-baseline path.
    pub(super) fn eval_object(&self) -> Option<&JsObject> {
        self.roots
            .iter()
            .find(|root| root.name == "eval")
            .map(|root| &root.object)
    }

    /// Whether `key` is part of the baseline global-key set (the scrub
    /// deletes everything else).
    pub(super) fn contains_global_key(&self, key: &PropertyKey) -> bool {
        self.global_keys.contains(key)
    }

    /// Verify the named integrity set against the baseline: the global
    /// object's own-key set (including symbol keys) and `eval` value
    /// identity, plus each root's key set, descriptor hash (kind/flags/
    /// value identity), and prototype identity. Any difference → `false`.
    pub(super) fn verify(&self, ctx: &mut Context) -> bool {
        let global = ctx.global_object();
        let keys = global.own_property_keys(ctx).unwrap_or_default();
        if !key_sets_equal(&keys, &self.global_keys) {
            return false;
        }
        let Some(eval_object) = self.eval_object() else {
            return false;
        };
        let eval_object = eval_object.clone();
        match global.get(js_string!("eval"), ctx) {
            Ok(value) if value.strict_equals(&JsValue::from(eval_object)) => {}
            _ => return false,
        }
        for root in &self.roots {
            if !root.matches(ctx) {
                tracing::debug!(root = root.name, "JS realm integrity drift detected");
                return false;
            }
        }
        true
    }
}

/// Set equality (order-insensitive) for property-key lists. Key order from
/// `own_property_keys` is deterministic per shape, but set comparison keeps
/// the check independent of engine insertion-order details.
fn key_sets_equal(current: &[PropertyKey], baseline: &[PropertyKey]) -> bool {
    let current: HashSet<&PropertyKey> = current.iter().collect();
    let baseline: HashSet<&PropertyKey> = baseline.iter().collect();
    current == baseline
}

/// Identity comparison of a `[[Prototype]]` pair.
fn protos_equal(current: Option<JsObject>, baseline: Option<&JsObject>) -> bool {
    match (current, baseline) {
        (Some(current), Some(baseline)) => JsObject::equals(&current, baseline),
        (None, None) => true,
        _ => false,
    }
}

/// The identity-relevant values of a descriptor: data → `[value]`,
/// accessor → `[get, set]` (absent fields skipped), generic → `[]`.
fn identity_values(desc: &PropertyDescriptor) -> Vec<JsValue> {
    let mut out = Vec::new();
    if desc.is_data_descriptor() {
        if let Some(value) = desc.value() {
            out.push(value.clone());
        }
    } else if desc.is_accessor_descriptor() {
        if let Some(get) = desc.get() {
            out.push(get.clone());
        }
        if let Some(set) = desc.set() {
            out.push(set.clone());
        }
    }
    out
}

/// Fold `object`'s descriptors for `keys` into a hash. For each key
/// (positionally): descriptor presence, kind tag, the
/// enumerable/configurable/writable flags, and the strict-equality identity
/// of each identity-relevant value against `baseline_identity[i][j]`. At
/// capture time the baseline IS the current state, so every identity folds
/// `true`; later divergence (changed value, replaced function, flipped
/// flag, vanished descriptor) folds a different hash.
fn descriptors_hash(
    object: &JsObject,
    keys: &[PropertyKey],
    baseline_identity: &[Vec<JsValue>],
) -> u64 {
    let mut hasher = DefaultHasher::new();
    for (i, key) in keys.iter().enumerate() {
        key.hash(&mut hasher);
        // `PropertyMap::get` returns an owned descriptor; the borrow on the
        // object ends with this statement.
        let desc = object.borrow().properties().get(key);
        let Some(desc) = desc else {
            // Descriptor vanished since capture: drift.
            false.hash(&mut hasher);
            continue;
        };
        true.hash(&mut hasher);
        let kind = if desc.is_data_descriptor() {
            0u8
        } else if desc.is_accessor_descriptor() {
            1u8
        } else {
            2u8
        };
        kind.hash(&mut hasher);
        desc.enumerable().hash(&mut hasher);
        desc.configurable().hash(&mut hasher);
        desc.writable().hash(&mut hasher);
        let ids = identity_values(&desc);
        ids.len().hash(&mut hasher);
        for (j, value) in ids.iter().enumerate() {
            let same = baseline_identity
                .get(i)
                .and_then(|row| row.get(j))
                .is_some_and(|baseline| value.strict_equals(baseline));
            same.hash(&mut hasher);
        }
    }
    hasher.finish()
}
