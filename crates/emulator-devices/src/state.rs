//! Diagnostic leaves for observational machine comparison. Binary content stays
//! in the typed checkpoint; JSON retains only its extent.

pub fn bytes(bytes: &[u8]) -> serde_json::Value {
    serde_json::json!({"bytes":bytes.len()})
}

/// Report the sensor word extent; the words stay in the typed checkpoint.
pub fn words16(words: &[u16]) -> serde_json::Value {
    serde_json::json!({"words":words.len()})
}

/// Report the reconstructed float extent; the bits stay in the typed checkpoint.
pub fn float32s(values: impl IntoIterator<Item = f32>) -> serde_json::Value {
    let mut count = 0;
    for _ in values {
        count += 1;
    }
    serde_json::json!({"floats":count})
}
