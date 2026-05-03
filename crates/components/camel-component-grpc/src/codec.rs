use bytes::{Buf, BufMut};
use tonic::Status;
use tonic::codec::{Codec, DecodeBuf, Decoder, EncodeBuf, Encoder};

pub struct RawBytesCodec;

impl Codec for RawBytesCodec {
    type Encode = Vec<u8>;
    type Decode = Vec<u8>;
    type Encoder = RawBytesEncoder;
    type Decoder = RawBytesDecoder;

    fn encoder(&mut self) -> Self::Encoder {
        RawBytesEncoder
    }

    fn decoder(&mut self) -> Self::Decoder {
        RawBytesDecoder
    }
}

pub struct RawBytesEncoder;

impl Encoder for RawBytesEncoder {
    type Item = Vec<u8>;
    type Error = Status;

    fn encode(&mut self, item: Self::Item, buf: &mut EncodeBuf<'_>) -> Result<(), Self::Error> {
        buf.put_slice(&item);
        Ok(())
    }
}

pub struct RawBytesDecoder;

impl Decoder for RawBytesDecoder {
    type Item = Vec<u8>;
    type Error = Status;

    fn decode(&mut self, buf: &mut DecodeBuf<'_>) -> Result<Option<Self::Item>, Self::Error> {
        if buf.remaining() == 0 {
            return Ok(None);
        }
        let data = buf.copy_to_bytes(buf.remaining()).to_vec();
        Ok(Some(data))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_raw_bytes_codec_constructs() {
        let mut codec = RawBytesCodec;
        let _encoder = codec.encoder();
        let _decoder = codec.decoder();
    }

    #[test]
    fn test_codec_type_aliases() {
        let mut codec = RawBytesCodec;
        let encoder: RawBytesEncoder = codec.encoder();
        let decoder: RawBytesDecoder = codec.decoder();
        let _ = (encoder, decoder);
    }

    #[test]
    fn test_encoder_is_unit_struct() {
        let e1 = RawBytesEncoder;
        let e2 = RawBytesEncoder;
        let _ = (e1, e2);
    }

    #[test]
    fn test_decoder_is_unit_struct() {
        let d1 = RawBytesDecoder;
        let d2 = RawBytesDecoder;
        let _ = (d1, d2);
    }

    #[test]
    fn test_codec_encoder_decoder_associated_types() {
        fn assert_codec<C: Codec>()
        where
            C::Encode: Clone,
            C::Decode: Clone,
        {
        }
        assert_codec::<RawBytesCodec>();
    }
}
