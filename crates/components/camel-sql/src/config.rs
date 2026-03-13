use camel_api::CamelError;

#[derive(Debug, Clone)]
pub struct SqlConfig;

impl SqlConfig {
    pub fn from_uri(_uri: &str) -> Result<Self, CamelError> {
        todo!()
    }
}
