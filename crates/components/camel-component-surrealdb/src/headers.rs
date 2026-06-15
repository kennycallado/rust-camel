//! Header constant definitions for the SurrealDB component.

/// Inline SurrealQL for the `query` operation.
/// Value: string (SurrealQL text).
pub const QUERY: &str = "CamelSurrealDbQuery";

/// JSON params for query parameter binding (`$name` syntax).
/// Value: JSON object map of param name → value.
pub const PARAMS: &str = "CamelSurrealDbParams";

/// Action type set by the live consumer on each notification.
/// Value: `"CREATE"` | `"UPDATE"` | `"DELETE"`.
pub const ACTION: &str = "CamelSurrealDbAction";

/// Table name set by the live consumer.
/// Value: string (table name).
pub const TABLE: &str = "CamelSurrealDbTable";

/// Raw `Vec<f32>` vector for `search` operation (alternative to body).
/// Value: JSON array of floats.
pub const VECTOR: &str = "CamelSurrealDbVector";

/// Record ID set by the producer for update/upsert/delete/patch operations
/// (when the target RecordId can be constructed from URI config). Not set for
/// create/vector/relate (server-generated IDs — read `body.id` from the JSON
/// response; for relate, `body.id` is the edge record's id, not the source node).
/// Value: string (`table:id`).
pub const RECORD_ID: &str = "CamelSurrealDbRecordId";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_constants_are_prefixed() {
        assert!(QUERY.starts_with("CamelSurrealDb"));
        assert!(PARAMS.starts_with("CamelSurrealDb"));
        assert!(ACTION.starts_with("CamelSurrealDb"));
        assert!(TABLE.starts_with("CamelSurrealDb"));
        assert!(VECTOR.starts_with("CamelSurrealDb"));
        assert!(RECORD_ID.starts_with("CamelSurrealDb"));
    }

    #[test]
    fn header_constants_are_unique() {
        let all = [QUERY, PARAMS, ACTION, TABLE, VECTOR, RECORD_ID];
        let mut sorted = all;
        sorted.sort();
        for i in 1..sorted.len() {
            assert_ne!(sorted[i], sorted[i - 1], "duplicate header constant");
        }
    }
}
