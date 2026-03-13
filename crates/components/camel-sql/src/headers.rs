/// Header for the SQL query to execute (overrides URI query).
pub const QUERY: &str = "CamelSql.Query";

/// Header containing the number of rows returned by a SELECT query.
pub const ROW_COUNT: &str = "CamelSql.RowCount";

/// Header containing the number of rows affected by an INSERT/UPDATE/DELETE.
pub const UPDATE_COUNT: &str = "CamelSql.UpdateCount";

/// Header containing parameters for SQL query binding.
pub const PARAMETERS: &str = "CamelSql.Parameters";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_header_constants_follow_camel_convention() {
        assert!(QUERY.starts_with("CamelSql."));
        assert!(ROW_COUNT.starts_with("CamelSql."));
        assert!(UPDATE_COUNT.starts_with("CamelSql."));
        assert!(PARAMETERS.starts_with("CamelSql."));
    }

    #[test]
    fn test_header_values() {
        assert_eq!(QUERY, "CamelSql.Query");
        assert_eq!(ROW_COUNT, "CamelSql.RowCount");
        assert_eq!(UPDATE_COUNT, "CamelSql.UpdateCount");
        assert_eq!(PARAMETERS, "CamelSql.Parameters");
    }
}
