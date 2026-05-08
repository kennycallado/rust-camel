use std::collections::HashMap;

use async_trait::async_trait;
use camel_api::CamelError;
use qdrant_client::Qdrant;
use qdrant_client::qdrant::{
    CreateCollectionBuilder, Distance, PointStruct, SearchPointsBuilder, UpsertPointsBuilder,
    VectorParamsBuilder,
};

use crate::traits::VectorStore;
use crate::types::{VectorHit, VectorItem};

/// Configuration for connecting to a Qdrant instance.
///
/// `url` must point to the **gRPC** port (default: `http://localhost:6334`).
/// The REST port (6333) is NOT supported — qdrant-client uses gRPC transport.
#[derive(Debug, Clone)]
pub struct QdrantConfig {
    pub url: String,
    pub collection: String,
}

/// VectorStore adapter backed by a Qdrant collection.
pub struct QdrantStore {
    pub(crate) config: QdrantConfig,
    client: Qdrant,
}

impl QdrantStore {
    pub fn new(config: QdrantConfig) -> Result<Self, CamelError> {
        let client = Qdrant::from_url(&config.url)
            .build()
            .map_err(|e| CamelError::RouteError(format!("Qdrant client build failed: {e}")))?;
        Ok(Self { config, client })
    }

    async fn ensure_collection(&self, vector_size: u64) -> Result<(), CamelError> {
        let exists = self
            .client
            .collection_exists(&self.config.collection)
            .await
            .map_err(|e| CamelError::RouteError(format!("Qdrant error: {e}")))?;

        if !exists {
            match self
                .client
                .create_collection(
                    CreateCollectionBuilder::new(&self.config.collection)
                        .vectors_config(VectorParamsBuilder::new(vector_size, Distance::Cosine)),
                )
                .await
            {
                Ok(_) => {}
                Err(e) => {
                    let msg = e.to_string().to_lowercase();
                    if !msg.contains("already exists") {
                        return Err(CamelError::RouteError(format!(
                            "Qdrant create collection: {e}"
                        )));
                    }
                }
            }
        }
        Ok(())
    }
}

#[async_trait]
impl VectorStore for QdrantStore {
    async fn upsert(&self, items: Vec<VectorItem>) -> Result<(), CamelError> {
        if items.is_empty() {
            return Ok(());
        }

        let vector_size = items[0].vector.len() as u64;
        self.ensure_collection(vector_size).await?;

        let points: Vec<PointStruct> = items
            .into_iter()
            .map(|item| {
                let payload: HashMap<String, qdrant_client::qdrant::Value> = item
                    .payload
                    .as_object()
                    .map(|obj| {
                        obj.iter()
                            .map(|(k, v)| (k.clone(), json_to_qdrant(v)))
                            .collect()
                    })
                    .unwrap_or_default();

                PointStruct::new(item.id, item.vector, payload)
            })
            .collect();

        self.client
            .upsert_points(UpsertPointsBuilder::new(&self.config.collection, points))
            .await
            .map_err(|e| CamelError::RouteError(format!("Qdrant upsert: {e}")))?;

        Ok(())
    }

    async fn search(&self, query: Vec<f32>, top_k: usize) -> Result<Vec<VectorHit>, CamelError> {
        let results = self
            .client
            .search_points(
                SearchPointsBuilder::new(&self.config.collection, query, top_k as u64)
                    .with_payload(true),
            )
            .await
            .map_err(|e| CamelError::RouteError(format!("Qdrant search: {e}")))?;

        Ok(results
            .result
            .into_iter()
            .map(|hit| VectorHit {
                id: hit
                    .id
                    .and_then(|id| match id.point_id_options {
                        Some(qdrant_client::qdrant::point_id::PointIdOptions::Num(n)) => {
                            Some(n.to_string())
                        }
                        Some(qdrant_client::qdrant::point_id::PointIdOptions::Uuid(s)) => Some(s),
                        None => None,
                    })
                    .unwrap_or_else(|| "unknown".to_string()),
                score: hit.score,
                payload: qdrant_payload_to_json(hit.payload),
            })
            .collect())
    }
}

// ---------------------------------------------------------------------------
// serde_json ↔ qdrant Value conversion helpers
// ---------------------------------------------------------------------------

fn json_to_qdrant(v: &serde_json::Value) -> qdrant_client::qdrant::Value {
    use qdrant_client::qdrant::value::Kind;
    let kind = match v {
        serde_json::Value::Null => Kind::NullValue(0),
        serde_json::Value::Bool(b) => Kind::BoolValue(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Kind::IntegerValue(i)
            } else {
                Kind::DoubleValue(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::String(s) => Kind::StringValue(s.clone()),
        serde_json::Value::Array(arr) => Kind::ListValue(qdrant_client::qdrant::ListValue {
            values: arr.iter().map(json_to_qdrant).collect(),
        }),
        serde_json::Value::Object(obj) => Kind::StructValue(qdrant_client::qdrant::Struct {
            fields: obj
                .iter()
                .map(|(k, v)| (k.clone(), json_to_qdrant(v)))
                .collect(),
        }),
    };
    qdrant_client::qdrant::Value { kind: Some(kind) }
}

fn qdrant_payload_to_json(
    payload: HashMap<String, qdrant_client::qdrant::Value>,
) -> serde_json::Value {
    let map: serde_json::Map<String, serde_json::Value> = payload
        .into_iter()
        .map(|(k, v)| (k, qdrant_value_to_json(v)))
        .collect();
    serde_json::Value::Object(map)
}

fn qdrant_value_to_json(v: qdrant_client::qdrant::Value) -> serde_json::Value {
    use qdrant_client::qdrant::value::Kind;
    match v.kind {
        None | Some(Kind::NullValue(_)) => serde_json::Value::Null,
        Some(Kind::BoolValue(b)) => serde_json::Value::Bool(b),
        Some(Kind::IntegerValue(i)) => serde_json::Value::Number(i.into()),
        Some(Kind::DoubleValue(f)) => serde_json::Number::from_f64(f)
            .map_or(serde_json::Value::Null, serde_json::Value::Number),
        Some(Kind::StringValue(s)) => serde_json::Value::String(s),
        Some(Kind::ListValue(l)) => {
            serde_json::Value::Array(l.values.into_iter().map(qdrant_value_to_json).collect())
        }
        Some(Kind::StructValue(s)) => {
            let map: serde_json::Map<String, serde_json::Value> = s
                .fields
                .into_iter()
                .map(|(k, v)| (k, qdrant_value_to_json(v)))
                .collect();
            serde_json::Value::Object(map)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qdrant_config_stores_collection() {
        let cfg = QdrantConfig {
            url: "http://localhost:6334".into(),
            collection: "docs".into(),
        };
        assert_eq!(cfg.collection, "docs");
        assert_eq!(cfg.url, "http://localhost:6334");
    }

    #[test]
    fn json_to_qdrant_string() {
        let v = serde_json::Value::String("hello".into());
        let qv = json_to_qdrant(&v);
        match qv.kind {
            Some(qdrant_client::qdrant::value::Kind::StringValue(s)) => assert_eq!(s, "hello"),
            _ => panic!("expected StringValue"),
        }
    }

    #[test]
    fn json_to_qdrant_number() {
        let v = serde_json::json!(42);
        let qv = json_to_qdrant(&v);
        match qv.kind {
            Some(qdrant_client::qdrant::value::Kind::IntegerValue(i)) => assert_eq!(i, 42),
            _ => panic!("expected IntegerValue"),
        }
    }

    #[test]
    fn json_to_qdrant_null() {
        let qv = json_to_qdrant(&serde_json::Value::Null);
        assert!(matches!(
            qv.kind,
            Some(qdrant_client::qdrant::value::Kind::NullValue(_))
        ));
    }

    #[test]
    fn json_to_qdrant_bool() {
        let qv = json_to_qdrant(&serde_json::Value::Bool(true));
        assert!(matches!(
            qv.kind,
            Some(qdrant_client::qdrant::value::Kind::BoolValue(true))
        ));
    }

    #[test]
    fn qdrant_value_to_json_roundtrip() {
        let original = serde_json::json!({
            "text": "hello",
            "count": 42,
            "active": true,
            "nested": {"key": "val"}
        });
        let qv = json_to_qdrant(&original);
        let back = qdrant_value_to_json(qv);
        assert_eq!(original, back);
    }
}
