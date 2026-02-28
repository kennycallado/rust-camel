pub mod body;
pub mod error;
pub mod exchange;
pub mod message;
pub mod processor;
pub mod value;

// Re-export core types at crate root for convenience.
pub use body::Body;
pub use error::CamelError;
pub use exchange::{Exchange, ExchangePattern};
pub use message::Message;
pub use processor::{BoxProcessor, IdentityProcessor, Processor, ProcessorFn};
pub use value::{Headers, Value};
