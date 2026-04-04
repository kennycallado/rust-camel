use crate::body::Body;
use crate::error::CamelError;

pub trait DataFormat: Send + Sync + 'static {
    fn name(&self) -> &str;
    fn marshal(&self, body: Body) -> Result<Body, CamelError>;
    fn unmarshal(&self, body: Body) -> Result<Body, CamelError>;
}
