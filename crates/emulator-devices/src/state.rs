//! Direct state leaves for observational machine comparison. Bytes, words and
//! float bit patterns are retained verbatim; no digest is computed.
use base64::{Engine as _, engine::general_purpose::STANDARD};

pub fn bytes(bytes: &[u8]) -> serde_json::Value {
    serde_json::json!({"bytes":bytes.len(), "base64":STANDARD.encode(bytes)})
}

/// Retain sensor words in canonical little-endian order.
pub fn words16(words: &[u16]) -> serde_json::Value {
    let mut bytes = Vec::with_capacity(words.len() * 2);
    for word in words {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    serde_json::json!({"words":words.len(),"base64":STANDARD.encode(&bytes)})
}

/// Retain reconstructed 32-bit floats in canonical little-endian order,
/// independent of the sensor's storage representation.
pub fn float32s(values: impl IntoIterator<Item = f32>) -> serde_json::Value {
    let mut bytes = Vec::new();
    let mut count = 0;
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
        count += 1;
    }
    serde_json::json!({"floats":count,"base64":STANDARD.encode(&bytes)})
}
