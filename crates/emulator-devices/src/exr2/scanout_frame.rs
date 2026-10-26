//! Scanout frames and their ordered metadata values; no runtime or image-format
//! dependency.
#[derive(Clone, Debug, PartialEq)]
pub enum State {
    None,
    Int(i64),
    Bool(bool),
    Str(String),
    Bytes(Vec<u8>),
    List(Vec<State>),
    Dict(Vec<(String, State)>),
}
impl State {
    pub fn str(s: &str) -> Self {
        Self::Str(s.into())
    }
    pub fn hex(v: u64) -> Self {
        Self::Str(format!("{v:#x}"))
    }
    pub fn dict(items: Vec<(&str, State)>) -> Self {
        Self::Dict(items.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }
    pub fn set(&mut self, key: &str, value: State) {
        let Self::Dict(items) = self else {
            panic!("metadata is not a dict")
        };
        if let Some((_, v)) = items.iter_mut().find(|(k, _)| k == key) {
            *v = value;
        } else {
            items.push((key.into(), value));
        }
    }
    pub fn get(&self, key: &str) -> Option<&State> {
        if let Self::Dict(items) = self {
            items.iter().find(|(k, _)| k == key).map(|(_, v)| v)
        } else {
            None
        }
    }
}
/// SHA-256 uses sha2's CPU-feature-selected ARMv8 backend, matching the
/// stable content hashing on this host.
pub fn sha256(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(data))
}

impl State {
    pub fn json(&self) -> serde_json::Value {
        use serde_json::Value as J;
        match self {
            Self::None => J::Null,
            Self::Int(n) => (*n).into(),
            Self::Bool(b) => (*b).into(),
            Self::Str(s) => s.clone().into(),
            Self::Bytes(v) => serde_json::json!(v),
            Self::List(v) => J::Array(v.iter().map(Self::json).collect()),
            Self::Dict(v) => J::Object(v.iter().map(|(k,v)|(k.clone(),v.json())).collect()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Frame {
    pub raw: Vec<u8>,
    pub rgb: Option<Vec<u8>>,
    pub png: Option<Vec<u8>>,
    pub state: State,
    pub width: usize,
    pub height: usize,
}
