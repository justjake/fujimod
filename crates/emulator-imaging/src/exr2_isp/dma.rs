//! Shared image DMA: bounded, nonoverlapping addressed rows, then status,
//! then the physical IRQ. No host read, digest or instruction drives effects.
use super::due;
use fujimod_emulator_devices::exr2::irq_routes::Irq;
use fujimod_emulator_devices::Cx;
use serde::{Deserialize, Serialize};

/// An explicit transfer clock, independent of the external frame cadence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Clock {
    Fixed { ticks: u64 },
    Throughput { bytes_per_tick: u64 },
}
impl Clock {
    pub fn delay(self, bytes: usize) -> Result<u64, String> {
        let ticks = match self {
            Self::Fixed { ticks } => ticks,
            Self::Throughput { bytes_per_tick } if bytes_per_tick != 0 => {
                (bytes as u64).div_ceil(bytes_per_tick).max(1)
            }
            _ => 0,
        };
        if ticks == 0 {
            Err("image DMA requires a positive transfer clock".into())
        } else {
            Ok(ticks)
        }
    }
}

pub const MAX_BYTES: usize = 64 * 1024 * 1024;

/// Normalize the uncached DRAM alias within its 30-bit aperture and reject
/// vector/ROM space. The runtime bus enforces the selected body's backing;
/// a shared image engine must not impose a smaller body's DRAM population.
pub fn physical(address: u64, count: usize) -> Result<u64, String> {
    let start = match address {
        0x40000000..0x80000000 => address - 0x40000000,
        _ => address,
    };
    if count == 0
        || count > MAX_BYTES
        || start < 0x4000
        || start
            .checked_add(count as u64)
            .is_none_or(|end| end > 0x40000000)
    {
        return Err("image DMA span escapes bounded DRAM".into());
    }
    Ok(start)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Raster {
    pub address: u64,
    pub width: usize,
    pub height: usize,
    pub pitch: usize,
}
impl Raster {
    pub fn validate(self, row_bytes: usize) -> Result<(), String> {
        if self.width == 0
            || self.width > 8192
            || self.height == 0
            || self.height > 8192
            || row_bytes == 0
            || self.pitch < row_bytes
            || self.pitch > MAX_BYTES
        {
            return Err("image DMA raster geometry is unsupported".into());
        }
        physical(
            self.address,
            self.pitch
                .checked_mul(self.height)
                .ok_or("image DMA size overflow")?,
        )?;
        Ok(())
    }
    pub fn overlaps(self, other: Self) -> Result<bool, String> {
        let a = physical(self.address, self.pitch * self.height)?;
        let b = physical(other.address, other.pitch * other.height)?;
        Ok(
            a < b + (other.pitch * other.height) as u64
                && b < a + (self.pitch * self.height) as u64,
        )
    }
    /// Compare addressed active bytes, excluding pitch padding on both sides.
    pub fn active_rows_overlap(
        self,
        row_bytes: usize,
        other: Self,
        other_row_bytes: usize,
    ) -> Result<bool, String> {
        self.validate(row_bytes)?;
        other.validate(other_row_bytes)?;
        let a = physical(self.address, self.pitch * self.height)?;
        let b = physical(other.address, other.pitch * other.height)?;
        let (mut i, mut j) = (0, 0);
        while i < self.height && j < other.height {
            let start_a = a + (i * self.pitch) as u64;
            let start_b = b + (j * other.pitch) as u64;
            if start_a < start_b + other_row_bytes as u64 && start_b < start_a + row_bytes as u64 {
                return Ok(true);
            }
            if start_a + row_bytes as u64 <= start_b {
                i += 1;
            } else {
                j += 1;
            }
        }
        Ok(false)
    }
}

/// Reused output storage. The owning model prepares pixels at START, writes
/// active rows only at `deadline`, publishes status, then calls `interrupt`.
#[derive(Default)]
pub struct Transfer {
    pub pixels: fujimod_emulator_devices::work::Buffer,
    raster: Option<Raster>,
    row_bytes: usize,
    deadline: Option<u64>,
    // Host-only: variable-size pure work resolves the ordinary DMA layout on
    // inspection/completion. A checkpoint stores that materialized layout.
    variable: bool,
}
impl Serialize for Transfer {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Snapshot<'a> {
            #[serde(with = "fujimod_emulator_devices::checkpoint::bytes")]
            pixels: &'a [u8],
            raster: Option<Raster>,
            row_bytes: usize,
            deadline: Option<u64>,
            #[serde(skip_serializing_if = "Option::is_none")]
            failure: Option<&'a str>,
        }
        let (raster, row_bytes) = self.layout();
        Snapshot {
            pixels: &self.pixels,
            raster,
            row_bytes,
            deadline: self.deadline,
            failure: self.pixels.failure(),
        }
        .serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for Transfer {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Snapshot {
            #[serde(with = "fujimod_emulator_devices::checkpoint::bytes")]
            pixels: Vec<u8>,
            raster: Option<Raster>,
            row_bytes: usize,
            deadline: Option<u64>,
            failure: Option<String>,
        }
        let saved = Snapshot::deserialize(deserializer)?;
        Ok(Self {
            pixels: fujimod_emulator_devices::work::Buffer::restored(saved.pixels, saved.failure),
            raster: saved.raster,
            row_bytes: saved.row_bytes,
            deadline: saved.deadline,
            variable: false,
        })
    }
}
impl Transfer {
    pub fn fingerprint(&self) -> serde_json::Value {
        let pixels = &self.pixels;
        let (raster, row_bytes) = self.layout();
        let deadline = self.deadline;
        let mut value = serde_json::json!({"pixels":fujimod_emulator_devices::fingerprint::bytes(pixels),"raster":raster,
            "row_bytes":row_bytes,"deadline":deadline});
        if let Some(failure) = pixels.failure() {
            value["failure"] = failure.into();
        }
        value
    }
    pub fn prepare(
        &mut self,
        now: u64,
        delay: u64,
        raster: Raster,
        row_bytes: usize,
    ) -> Result<(), String> {
        self.reserve(now, delay, raster, row_bytes)?;
        self.pixels.resize(row_bytes * raster.height, 0);
        Ok(())
    }
    /// Fix the guest destination/deadline while leaving staging allocation and
    /// initialization to the pure owned computation.
    pub fn prepare_deferred(
        &mut self,
        now: u64,
        delay: u64,
        raster: Raster,
        row_bytes: usize,
    ) -> Result<(), String> {
        self.reserve(now, delay, raster, row_bytes)?;
        self.pixels.clear();
        Ok(())
    }
    fn reserve(
        &mut self,
        now: u64,
        delay: u64,
        raster: Raster,
        row_bytes: usize,
    ) -> Result<(), String> {
        if self.deadline.is_some() {
            return Err("image DMA is already pending".into());
        }
        raster.validate(row_bytes)?;
        let deadline = due(now, delay)?;
        self.raster = Some(raster);
        self.row_bytes = row_bytes;
        self.deadline = Some(deadline);
        self.variable = false;
        Ok(())
    }
    /// Reserve a bounded destination while a codec derives its actual byte
    /// count. Allocation, computation and length resolution stay off START.
    pub fn prepare_variable(
        &mut self,
        now: u64,
        delay: u64,
        address: u64,
        capacity: usize,
    ) -> Result<(), String> {
        let raster = Raster {
            address,
            width: 1,
            height: 1,
            pitch: capacity,
        };
        self.reserve(now, delay, raster, capacity)?;
        self.pixels.clear();
        self.variable = true;
        Ok(())
    }
    fn layout(&self) -> (Option<Raster>, usize) {
        if self.variable {
            let size = self.pixels.len();
            (self.raster.map(|r| Raster { pitch: size, ..r }), size)
        } else {
            (self.raster, self.row_bytes)
        }
    }
    /// Validate owned staging and addressed rows before accepting a checkpoint.
    pub fn validate(&self) -> Result<(), String> {
        let (raster, row_bytes) = self.layout();
        match (self.deadline, raster) {
            (Some(_), Some(raster)) => {
                raster.validate(row_bytes)?;
                if self.pixels.len() != row_bytes * raster.height {
                    return Err("image DMA staging size differs from raster".into());
                }
            }
            (None, None)
                if row_bytes == 0 && self.pixels.is_empty() && self.pixels.failure().is_none() => {}
            _ => return Err("inconsistent image DMA staging/deadline".into()),
        }
        Ok(())
    }
    pub fn deadline(&self) -> Option<u64> {
        self.deadline
    }
    pub fn ready(&self, now: u64) -> bool {
        self.deadline.is_some_and(|at| now >= at)
    }
    /// Join pure work and return its modeled failure without writing RAM.
    pub fn check_result(&self) -> Result<(), String> {
        self.pixels.check()
    }
    pub fn cancel(&mut self) {
        self.deadline = None;
        self.raster = None;
        self.row_bytes = 0;
        self.variable = false;
        // Completed bytes live in guest RAM. Keep allocation capacity for the
        // next transfer without retaining a second full image in saved state.
        self.pixels.clear();
    }
    pub fn write(&mut self, cx: &mut Cx<'_>) -> Result<bool, String> {
        if !self.ready(cx.now()) {
            return Ok(false);
        }
        self.check_result()?;
        let (raster, row_bytes) = self.layout();
        let r = raster.ok_or("image DMA has no destination")?;
        if self.variable && row_bytes > self.row_bytes {
            return Err("image DMA result exceeds reserved capacity".into());
        }
        // Every physical row span and nonoverlap were checked at preparation;
        // the runtime bus additionally enforces the selected body backing.
        if row_bytes == r.pitch {
            write(cx, r.address, &self.pixels)?;
        } else {
            for y in 0..r.height {
                write(
                    cx,
                    r.address + (y * r.pitch) as u64,
                    &self.pixels[y * row_bytes..(y + 1) * row_bytes],
                )?;
            }
        }
        self.cancel();
        Ok(true)
    }
    pub fn interrupt(cx: &mut Cx<'_>, irq: Irq) {
        if let Err(e) = irq.raise(cx) {
            cx.fail(e);
        }
    }
}

/// Physical bounds used to reject overlap before issuing a multi-span job.
pub fn span(address: u64, count: usize) -> Result<std::ops::Range<u64>, String> {
    let start = physical(address, count)?;
    Ok(start..start + count as u64)
}
/// Write one preflighted span. Device status and IRQ publication follow all
/// writes in the owning controller's completion, including a paired batch.
pub fn write(cx: &mut Cx<'_>, address: u64, bytes: &[u8]) -> Result<(), String> {
    physical(address, bytes.len())?;
    cx.ram_write(address, bytes)
}

#[cfg(test)]
mod fingerprint_tests {
    use super::*;
    #[test]
    fn active_row_aliases_distinguish_interleaved_padding_and_later_row_collisions() {
        let source = Raster {
            address: 0x10000,
            width: 368,
            height: 240,
            pitch: 1440,
        };
        let output = Raster {
            address: 0x400102e0,
            width: 192,
            height: 120,
            pitch: 1440,
        };
        assert!(source.overlaps(output).unwrap());
        assert!(!source.active_rows_overlap(736, output, 384).unwrap());
        assert!(!output.active_rows_overlap(384, source, 736).unwrap());
        assert!(
            source
                .active_rows_overlap(
                    736,
                    Raster {
                        address: output.address - 1,
                        ..output
                    },
                    384
                )
                .unwrap()
        );
        // First rows are disjoint; different pitches collide on a later row.
        assert!(
            source
                .active_rows_overlap(
                    736,
                    Raster {
                        pitch: 1424,
                        ..output
                    },
                    384
                )
                .unwrap()
        );
        assert!(
            source
                .active_rows_overlap(
                    736,
                    Raster {
                        height: 0,
                        ..output
                    },
                    384
                )
                .is_err()
        );
        assert!(source.active_rows_overlap(1441, output, 384).is_err());
    }
    #[test]
    fn pending_image_identity_is_bounded_and_preserves_payload_and_deadline() {
        let mut transfer = Transfer::default();
        transfer
            .prepare(
                10,
                4,
                Raster {
                    address: 0x10000,
                    width: 1024,
                    height: 2048,
                    pitch: 2048,
                },
                2048,
            )
            .unwrap();
        transfer.pixels.fill(0xa5);
        let identity = transfer.fingerprint();
        assert!(serde_json::to_vec(&identity).unwrap().len() < 500);
        let saved = fujimod_emulator_devices::checkpoint::Checkpoint::encode("transfer", &transfer).unwrap();
        let mut restored: Transfer = saved.decode("transfer").unwrap();
        assert_eq!(restored.fingerprint(), identity);
        *restored.pixels.last_mut().unwrap() ^= 1;
        assert_ne!(restored.fingerprint(), identity);
        assert_eq!(*transfer.pixels.last().unwrap(), 0xa5);
        restored = saved.decode("transfer").unwrap();
        restored.deadline = Some(15);
        assert_ne!(restored.fingerprint(), identity);
        assert_eq!(transfer.fingerprint(), identity);
    }
}
