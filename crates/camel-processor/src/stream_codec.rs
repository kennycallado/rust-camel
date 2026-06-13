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
                "application/zip" | "application/x-zip-compressed" => Err(CamelError::Config(
                    "stream split format=Auto: ZIP archives require explicit stream.format: zip"
                        .into(),
                )),
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

#[derive(Debug)]
pub enum ArchiveSplitKind {
    Zip,
}

pub enum ResolvedStreamSplit {
    Incremental(Box<dyn StreamSplitCodec>),
    MaterializedArchive(ArchiveSplitKind),
}

pub fn resolve_incremental_codec(
    format: &StreamSplitFormat,
) -> Result<Box<dyn StreamSplitCodec>, CamelError> {
    match format {
        StreamSplitFormat::Ndjson => Ok(Box::new(ndjson::NdjsonCodec)),
        StreamSplitFormat::Lines => Ok(Box::new(lines::LinesCodec)),
        StreamSplitFormat::Chunks => Ok(Box::new(chunks::ChunksCodec)),
        StreamSplitFormat::Zip => Err(CamelError::Config(
            "Zip is a materialized archive format, not an incremental codec".into(),
        )),
        StreamSplitFormat::Auto => Err(CamelError::Config(
            "resolve_incremental_codec requires a resolved format, not Auto".into(),
        )),
    }
}

pub fn resolve_split(
    format: &StreamSplitFormat,
    metadata: &StreamMetadata,
) -> Result<ResolvedStreamSplit, CamelError> {
    let resolved = resolve_format(format, metadata)?;
    match resolved {
        StreamSplitFormat::Zip => Ok(ResolvedStreamSplit::MaterializedArchive(
            ArchiveSplitKind::Zip,
        )),
        _ => Ok(ResolvedStreamSplit::Incremental(resolve_incremental_codec(
            &resolved,
        )?)),
    }
}

pub fn fragment_stream_exchange(parent: &Exchange, body: Body) -> Exchange {
    let mut ex = fragment_exchange(parent, body);
    ex.input.headers.remove("Content-Length");
    ex.input.headers.remove("Content-Type");
    ex
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::StreamMetadata;

    fn default_metadata() -> StreamMetadata {
        StreamMetadata::default()
    }

    #[test]
    fn test_resolve_split_ndjson_is_incremental() {
        let result = resolve_split(&StreamSplitFormat::Ndjson, &default_metadata()).unwrap();
        assert!(matches!(result, ResolvedStreamSplit::Incremental(_)));
    }

    #[test]
    fn test_resolve_split_zip_is_materialized() {
        let result = resolve_split(&StreamSplitFormat::Zip, &default_metadata()).unwrap();
        assert!(matches!(
            result,
            ResolvedStreamSplit::MaterializedArchive(ArchiveSplitKind::Zip)
        ));
    }

    #[test]
    fn test_resolve_incremental_codec_returns_codec_for_ndjson() {
        let result = resolve_incremental_codec(&StreamSplitFormat::Ndjson);
        assert!(result.is_ok());
    }

    #[test]
    fn test_resolve_incremental_codec_rejects_zip() {
        let result = resolve_incremental_codec(&StreamSplitFormat::Zip);
        assert!(result.is_err());
    }

    #[test]
    fn test_auto_application_zip_suggests_explicit_format() {
        let meta = StreamMetadata {
            content_type: Some("application/zip".to_string()),
            ..Default::default()
        };
        let result = resolve_split(&StreamSplitFormat::Auto, &meta);
        let msg = match result {
            Err(e) => e.to_string(),
            Ok(_) => panic!("expected Err for Auto + application/zip"),
        };
        assert!(
            msg.contains("format: zip"),
            "expected suggestion in error, got: {msg}"
        );
    }
}
