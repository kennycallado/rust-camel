//! JSON object decoder that keeps wire order and rejects duplicate keys.

use std::collections::HashSet;
use std::fmt;
use std::marker::PhantomData;

use serde::Deserialize;
use serde::de::{Deserializer, Error, MapAccess, Visitor};

/// Object entries in wire order. Duplicate keys fail to decode.
#[derive(Debug)]
pub(crate) struct DupCheckedMap<V>(pub(crate) Vec<DupEntry<V>>);

/// One key/value pair of a [`DupCheckedMap`].
#[derive(Debug)]
pub(crate) struct DupEntry<V> {
    pub(crate) key: String,
    pub(crate) value: V,
}

struct DupMapVisitor<V>(PhantomData<V>);

impl<'de, V: Deserialize<'de>> Visitor<'de> for DupMapVisitor<V> {
    type Value = DupCheckedMap<V>;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a JSON object with unique keys")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut seen: HashSet<String> = HashSet::new();
        let mut entries = Vec::with_capacity(map.size_hint().unwrap_or(0));
        while let Some((key, value)) = map.next_entry::<String, V>()? {
            if !seen.insert(key.clone()) {
                return Err(A::Error::custom(format!("duplicate key '{key}'")));
            }
            entries.push(DupEntry { key, value });
        }
        Ok(DupCheckedMap(entries))
    }
}

impl<'de, V: Deserialize<'de>> Deserialize<'de> for DupCheckedMap<V> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(DupMapVisitor(PhantomData))
    }
}

#[cfg(test)]
mod tests {
    use super::DupCheckedMap;

    #[test]
    fn dup_map_preserves_wire_order() {
        let map = serde_json::from_str::<DupCheckedMap<f64>>(r#"{"b":1.0,"a":2.0}"#)
            .expect("object with unique keys should decode");
        let keys: Vec<&str> = map.0.iter().map(|e| e.key.as_str()).collect();
        let values: Vec<f64> = map.0.iter().map(|e| e.value).collect();
        assert_eq!(keys, ["b", "a"]);
        assert_eq!(values, [1.0, 2.0]);
    }

    #[test]
    fn dup_map_rejects_duplicate_key() {
        let err = serde_json::from_str::<DupCheckedMap<f64>>(r#"{"a":1.0,"a":2.0}"#)
            .expect_err("duplicate key must be rejected");
        assert!(
            err.to_string().contains("duplicate key 'a'"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn dup_map_rejects_non_object() {
        assert!(serde_json::from_str::<DupCheckedMap<f64>>("[1,2]").is_err());
    }
}
