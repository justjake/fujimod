//! Region checks: read a range and compare it with an expected SHA-256.
use crate::{
    Error, Result,
    ff80::{
        Session,
        commands::memory::{Address, DebugMemoryLimits, MemoryRange},
    },
    operations::memory::read_range,
};
use sha2::{Digest, Sha256};
use std::num::NonZeroU32;

#[derive(Clone)]
pub struct Sha256Check {
    pub range: MemoryRange,
    pub expected: [u8; 32],
}
impl Sha256Check {
    pub fn from_hex(address: u32, size: u32, expected: &str) -> Result<Self> {
        if expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::InvalidLength);
        }
        let mut digest = [0u8; 32];
        for (i, byte) in digest.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&expected[2 * i..2 * i + 2], 16)
                .map_err(|_| Error::InvalidLength)?;
        }
        Ok(Self {
            range: MemoryRange::new(
                Address(address),
                NonZeroU32::new(size).ok_or(Error::InvalidLength)?,
            )?,
            expected: digest,
        })
    }
}
struct Group {
    start: u32,
    end: u64,
    members: Vec<usize>,
}
fn groups(checks: &[Sha256Check]) -> Vec<Group> {
    let mut order: Vec<_> = (0..checks.len()).collect();
    order.sort_by_key(|&i| checks[i].range.address().0);
    let mut groups: Vec<Group> = Vec::new();
    for i in order {
        let r = checks[i].range;
        let start = r.address().0;
        let end = u64::from(start) + u64::from(r.length().get());
        if let Some(group) = groups.last_mut() {
            let combined = group.end.max(end);
            if u64::from(start) <= group.end + 32768 && combined - u64::from(group.start) <= 1 << 20
            {
                group.end = combined;
                group.members.push(i);
                continue;
            }
        }
        groups.push(Group {
            start,
            end,
            members: vec![i],
        });
    }
    // A large admitted group pays qualification once; the qualified transport
    // can then choose cheap reads for the remaining, smaller groups.
    groups.sort_by_key(|g| std::cmp::Reverse(g.end - u64::from(g.start)));
    groups
}
/// Verify every digest in the current session. Nearby spans may share one bulk
/// read, only when the supplied transport admits the entire enclosing range.
/// CPU fallback reads exactly the original spans, never their intervening gaps.
pub async fn verify_sha256(
    session: &mut Session,
    limits: DebugMemoryLimits,
    checks: &[Sha256Check],
    mut io: Option<&mut dyn super::card::BufferIo>,
) -> Result<()> {
    for group in groups(checks) {
        let size = (group.end - u64::from(group.start))
            .try_into()
            .map_err(|_| Error::InvalidLength)?;
        let packed = group
            .members
            .iter()
            .map(|&i| u64::from(checks[i].range.length().get()))
            .sum::<u64>()
            .min(u64::from(size)) as u32;
        // Gaps must not create an artificial CPU cost advantage: bulk must be
        // preferred for the packed checked bytes as well as the enclosing read.
        if io
            .as_ref()
            .is_some_and(|io| io.admits(group.start, size) && io.admits(group.start, packed))
        {
            let raw = io
                .as_mut()
                .unwrap()
                .read(session, group.start, size)
                .await?;
            if raw.len() != size as usize {
                return Err(Error::InvalidLength);
            }
            for index in group.members {
                let check = &checks[index];
                let start = (check.range.address().0 - group.start) as usize;
                if sha256_bytes(&raw[start..start + check.range.length().get() as usize])
                    != check.expected
                {
                    return Err(Error::Card(format!(
                        "Code differs at {:#x}",
                        check.range.address().0
                    )));
                }
            }
        } else {
            let ranges: Vec<_> = group.members.iter().map(|&i| checks[i].range).collect();
            let raw = super::memory::read_ranges(session, &ranges, limits).await?;
            for (index, bytes) in group.members.into_iter().zip(raw) {
                let check = &checks[index];
                if sha256_bytes(&bytes) != check.expected {
                    return Err(Error::Card(format!(
                        "Code differs at {:#x}",
                        check.range.address().0
                    )));
                }
            }
        }
    }
    Ok(())
}

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
#[cfg(test)]
mod tests;
