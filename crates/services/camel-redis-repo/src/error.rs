//! Error mapping from `redis::RedisError` to `CamelError`.

use camel_api::CamelError;

/// Maps a transport or command failure from the `redis` client to a
/// `CamelError::Io`, preserving the failure text for triage.
pub fn to_camel_error(err: redis::RedisError) -> CamelError {
    CamelError::Io(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::to_camel_error;
    use camel_api::CamelError;

    #[test]
    fn to_camel_error_maps_to_io() {
        let err = to_camel_error(redis::RedisError::from((
            redis::ErrorKind::Io,
            "connection reset by peer",
        )));
        assert!(matches!(err, CamelError::Io(_)));
        assert_eq!(err.classify(), "io");
    }
}
