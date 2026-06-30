use crate::Exchange;
use crate::body::Body;
use crate::error::CamelError;

pub trait DataFormat: Send + Sync + 'static {
    fn name(&self) -> &str;

    fn marshal(&self, body: Body) -> Result<Body, CamelError>;

    fn unmarshal(&self, body: Body) -> Result<Body, CamelError>;

    /// Exchange-aware marshal hook. Default delegates to `marshal`.
    /// Override when the format needs Exchange context (e.g., writing
    /// metadata headers like `CamelCsvHeaderRecord`).
    fn marshal_in_exchange(&self, exchange: &mut Exchange, body: Body) -> Result<Body, CamelError> {
        let _ = exchange;
        self.marshal(body)
    }

    /// Exchange-aware unmarshal hook. Default delegates to `unmarshal`.
    /// Override when the format needs Exchange context (e.g., capturing
    /// metadata headers like `CamelCsvHeaderRecord`).
    fn unmarshal_in_exchange(
        &self,
        exchange: &mut Exchange,
        body: Body,
    ) -> Result<Body, CamelError> {
        let _ = exchange;
        self.unmarshal(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exchange::Exchange;

    struct DummyFormat;

    impl DataFormat for DummyFormat {
        fn name(&self) -> &str {
            "dummy"
        }
        fn marshal(&self, _body: Body) -> Result<Body, CamelError> {
            Ok(Body::Text("marshaled".into()))
        }
        fn unmarshal(&self, _body: Body) -> Result<Body, CamelError> {
            Ok(Body::Text("unmarshaled".into()))
        }
    }

    #[test]
    fn test_default_marshal_in_exchange_delegates() {
        let mut ex = Exchange::default();
        let df = DummyFormat;
        let result = df.marshal_in_exchange(&mut ex, Body::Empty);
        assert_eq!(result.unwrap(), Body::Text("marshaled".into()));
    }

    #[test]
    fn test_default_unmarshal_in_exchange_delegates() {
        let mut ex = Exchange::default();
        let df = DummyFormat;
        let result = df.unmarshal_in_exchange(&mut ex, Body::Empty);
        assert_eq!(result.unwrap(), Body::Text("unmarshaled".into()));
    }
}
