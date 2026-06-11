use bytes::Bytes;
use camel_api::{
    Body, CamelError, Exchange, StreamMetadata, StreamSplitConfig, StreamSplitFormat,
    fragment_exchange,
};
use futures::Stream;
use std::pin::Pin;

pub mod chunks;
pub mod lines;
pub mod ndjson;

pub const CAMEL_STREAM_ORIGIN: &str = "CamelStreamOrigin";
pub const CAMEL_STREAM_SOURCE_CONTENT_TYPE: &str = "CamelStreamSourceContentType";
pub const CAMEL_STREAM_OFFSET: &str = "CamelStreamOffset";
pub const CAMEL_STREAM_BATCH_SIZE: &str = "CamelStreamBatchSize";

pub struct StreamSplitInput {
    pub parent: Exchange,
    pub stream: Pin<Box<dyn Stream<Item = Result<Bytes, CamelError>> + Send>>,
    pub metadata: StreamMetadata,
}

pub trait StreamSplitCodec: Send + Sync {
    fn split(
        &self,
        input: StreamSplitInput,
        config: StreamSplitConfig,
    ) -> Pin<Box<dyn Stream<Item = Result<Exchange, CamelError>> + Send>>;
}

pub fn resolve_format(
    format: &StreamSplitFormat,
    metadata: &StreamMetadata,
) -> Result<StreamSplitFormat, CamelError> {
    match format {
        StreamSplitFormat::Auto => {
            let ct = metadata
                .content_type
                .as_deref()
                .unwrap_or("")
                .to_lowercase();
            let ct = ct.split(';').next().unwrap_or("").trim();
            match ct {
                "application/x-ndjson" => Ok(StreamSplitFormat::Ndjson),
                "text/plain" => Ok(StreamSplitFormat::Lines),
                "application/octet-stream" => Ok(StreamSplitFormat::Chunks),
                "" => Err(CamelError::Config(
                    "stream split format=Auto but stream has no content_type".into(),
                )),
                other => Err(CamelError::Config(format!(
                    "stream split format=Auto: unknown content type '{}'",
                    other
                ))),
            }
        }
        other => Ok(other.clone()),
    }
}

pub fn resolve_codec(format: &StreamSplitFormat) -> Box<dyn StreamSplitCodec> {
    match format {
        StreamSplitFormat::Ndjson => Box::new(ndjson::NdjsonCodec),
        StreamSplitFormat::Lines => Box::new(lines::LinesCodec),
        StreamSplitFormat::Chunks => Box::new(chunks::ChunksCodec),
        StreamSplitFormat::Auto => unreachable!("resolve_format must be called first"),
    }
}

pub fn fragment_stream_exchange(parent: &Exchange, body: Body) -> Exchange {
    let mut ex = fragment_exchange(parent, body);
    ex.input.headers.remove("Content-Length");
    ex.input.headers.remove("Content-Type");
    ex
}
