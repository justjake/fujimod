//! Photo sync frames and their data-channel message encoding.
//!
//! Each message contains a length-delimited protobuf frame. Chunk messages
//! append the exact number of raw file bytes stated in the chunk header.

use prost::Message;

include!(concat!(env!("OUT_DIR"), "/fujimod.sync.v1.rs"));

/// Encode one complete data-channel message.
pub fn encode(frame: &Frame, payload: &[u8]) -> Result<Vec<u8>, &'static str> {
    check_payload(frame, payload)?;
    let mut message = Vec::with_capacity(frame.encoded_len() + payload.len() + 10);
    frame.encode_length_delimited(&mut message).map_err(|_| "encode failed")?;
    message.extend_from_slice(payload);
    Ok(message)
}

/// Decode a complete message and return its raw file bytes.
pub fn decode(message: &[u8]) -> Result<(Frame, &[u8]), &'static str> {
    let mut remaining = message;
    let frame = Frame::decode_length_delimited(&mut remaining).map_err(|_| "invalid frame")?;
    check_payload(&frame, remaining)?;
    Ok((frame, remaining))
}

fn check_payload(frame: &Frame, payload: &[u8]) -> Result<(), &'static str> {
    match &frame.body {
        Some(frame::Body::Chunk(chunk)) if chunk.length as usize == payload.len() => Ok(()),
        Some(frame::Body::Chunk(_)) => Err("chunk length does not match payload"),
        Some(_) if payload.is_empty() => Ok(()),
        Some(_) => Err("unexpected payload"),
        None => Err("missing frame body"),
    }
}
