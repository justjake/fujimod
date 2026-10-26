//! Firmware identities: a firmware id and the SHA-256 digest of its update
//! file, which service and camera definitions qualify against.

/// A firmware identity that a definition or a camera does not satisfy.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum IdentityError {
    #[error("firmware identity fields must be nonempty")]
    EmptyIdentity,
    #[error("firmware digest must contain exactly 64 hexadecimal characters")]
    InvalidDigest,
    #[error("definition does not match the selected firmware id and digest")]
    FirmwareMismatch,
}

/// SHA-256 identity, not an address or command selector.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Digest([u8; 32]);
impl Digest {
    pub fn parse(hex: &str) -> Result<Self, IdentityError> {
        if hex.len() != 64 || !hex.as_bytes().iter().all(u8::is_ascii_hexdigit) {
            return Err(IdentityError::InvalidDigest);
        }
        let mut bytes = [0; 32];
        for (index, value) in bytes.iter_mut().enumerate() {
            *value =
                u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).expect("validated hex pair");
        }
        Ok(Self(bytes))
    }
    pub fn bytes(self) -> [u8; 32] {
        self.0
    }
    pub fn hex(self) -> String {
        self.0.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FirmwareIdentity {
    id: String,
    digest: Digest,
}
impl FirmwareIdentity {
    pub fn new(id: impl Into<String>, sha256: &str) -> Result<Self, IdentityError> {
        let id = id.into();
        if id.trim().is_empty() {
            return Err(IdentityError::EmptyIdentity);
        }
        Ok(Self {
            id,
            digest: Digest::parse(sha256)?,
        })
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn digest(&self) -> Digest {
        self.digest
    }
}
