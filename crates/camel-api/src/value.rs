/// Value type alias for dynamic header/property values.
pub type Value = serde_json::Value;

/// Headers type alias.
pub type Headers = std::collections::HashMap<String, Value>;

/// Total order over two `serde_json::Value`s.
///
/// Tier: Null < Bool < Number < String < Array < Object.
/// NaN sorts AFTER all real numbers (NaN-last), consistent with
/// `camel_processor::sort::SortKey`.
///
/// Shared by batch/stream resequencing policies (I3).
pub fn cmp_values(a: &Value, b: &Value) -> std::cmp::Ordering {
    use serde_json::Value as J;
    match (a, b) {
        (J::Null, J::Null) => std::cmp::Ordering::Equal,
        (J::Null, _) => std::cmp::Ordering::Less,
        (_, J::Null) => std::cmp::Ordering::Greater,
        (J::Bool(va), J::Bool(vb)) => va.cmp(vb),
        (J::Bool(_), _) => std::cmp::Ordering::Less,
        (_, J::Bool(_)) => std::cmp::Ordering::Greater,
        (J::Number(va), J::Number(vb)) => {
            let af = va.as_f64().unwrap_or(f64::NAN);
            let bf = vb.as_f64().unwrap_or(f64::NAN);
            // NaN-last: NaN > all real numbers; NaN == NaN
            af.partial_cmp(&bf)
                .unwrap_or_else(|| af.is_nan().cmp(&bf.is_nan()))
        }
        (J::Number(_), _) => std::cmp::Ordering::Less,
        (_, J::Number(_)) => std::cmp::Ordering::Greater,
        (J::String(sa), J::String(sb)) => sa.cmp(sb),
        (J::Array(va), J::Array(vb)) => serde_json::to_string(va)
            .unwrap_or_default()
            .cmp(&serde_json::to_string(vb).unwrap_or_default()),
        (J::Object(va), J::Object(vb)) => serde_json::to_string(va)
            .unwrap_or_default()
            .cmp(&serde_json::to_string(vb).unwrap_or_default()),
        (J::Array(_), _) => std::cmp::Ordering::Less,
        (_, J::Array(_)) => std::cmp::Ordering::Greater,
        (J::Object(_), _) => std::cmp::Ordering::Less,
        (_, J::Object(_)) => std::cmp::Ordering::Greater,
    }
}
