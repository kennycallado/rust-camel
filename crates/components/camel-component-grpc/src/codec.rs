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
