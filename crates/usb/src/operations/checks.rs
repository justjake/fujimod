//! Region checks: read a range and compare it with an expected SHA-256.
use crate::{
    Result,
    ff80::{
        Session,
        commands::memory::{DebugMemoryLimits, MemoryRange},
    },
    operations::memory::read_range,
};
use sha2::{Digest, Sha256};

/// Hash bytes already received through a qualified transport.
pub fn sha256_bytes(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// SHA-256 of the bytes currently in `range`.
pub async fn region_sha256(
    session: &mut Session,
    range: MemoryRange,
    limits: DebugMemoryLimits,
) -> Result<[u8; 32]> {
    let bytes = read_range(session, range, limits).await?;
    Ok(Sha256::digest(&bytes).into())
}
/// True when `range` currently hashes to `expected`.
pub async fn region_matches(
    session: &mut Session,
    range: MemoryRange,
    limits: DebugMemoryLimits,
    expected: [u8; 32],
) -> Result<bool> {
    Ok(region_sha256(session, range, limits).await? == expected)
}
