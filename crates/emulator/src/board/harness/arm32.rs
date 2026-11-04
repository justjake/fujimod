//! Explicit, MMU-off routine instrumentation. This never selects device behavior.

type Span = (u64, u64);
type Store = (usize, u64, u64, u32, u64);

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MmioAccess {
    pub start: u64,
    pub end: u64,
    pub width: u32,
    pub read: bool,
    pub write: bool,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevicePolicy {
    pub mmio: Vec<MmioAccess>,
    pub model_readable: Vec<Span>,
    pub model_writable: Vec<Span>,
}

pub struct Guard {
    pub core: usize,
    executable: Vec<Span>,
    readable: Vec<Span>,
    writable: Vec<Span>,
    return_address: u64,
    store_limit: usize,
    stores: Vec<Store>,
    model_readable: Vec<Span>,
    model_writable: Vec<Span>,
    model_writes: Vec<(u64, u64)>, // (address, byte count), not half-open spans
    physical_backing: Vec<Span>,
    mmio: Vec<MmioAccess>,
    mmio_accesses: Vec<(u64, u32, bool)>,
    pub returned: bool,
    pub failure: Option<String>,
    accepted: u64,
}

fn contains(spans: &[Span], address: u64, size: u64) -> bool {
    address.checked_add(size).is_some_and(|end| {
        spans
            .iter()
            .any(|&(low, high)| low <= address && end <= high)
    })
}

fn spans(mut values: Vec<Span>, backing: &[Span]) -> Result<Vec<Span>, String> {
    values.sort_unstable();
    if values.len() > 4096
        || values
            .iter()
            .any(|&(low, high)| low >= high || !contains(backing, low, high - low))
        || values.windows(2).any(|p| p[0].1 > p[1].0)
    {
        return Err(
            "Driver guard ranges must be disjoint, nonempty declared physical spans".into(),
        );
    }
    Ok(values)
}

#[cfg(test)]
mod sparse_backing_tests {
    use super::Guard;

    fn guard(
        backing: Vec<(u64, u64)>,
        read: Vec<(u64, u64)>,
        sentinel: u64,
    ) -> Result<Guard, String> {
        Guard::new(
            0,
            vec![(0xff000000, 0xff000010)],
            read,
            vec![(0x1000, 0x1004)],
            sentinel,
            8,
            backing,
        )
    }

    #[test]
    fn sparse_backing_admits_high_code_without_admitting_the_hole() {
        let backing = vec![(0, 0x4000), (0xff000000, 0xff001000)];
        let mut accepted = guard(backing.clone(), vec![(0x1000, 0x1004)], 0x3000).unwrap();
        assert!(accepted.fetch(0, 0xff000000, 4).unwrap());
        assert!(!accepted.fetch(0, 0x3000, 4).unwrap());
        assert!(guard(backing, vec![(0x4000, 0x4004)], 0x3000).is_err());
    }

    #[test]
    fn sparse_backing_rejects_crossing_windows_and_invalid_sentinels() {
        let backing = vec![(0, 0x4000), (0xff000000, 0xff001000)];
        assert!(guard(backing.clone(), vec![(0x3ffc, 0x4004)], 0x3000).is_err());
        assert!(guard(backing.clone(), vec![], 0x5000).is_err());
        assert!(guard(backing.clone(), vec![], 0xff000000).is_err());
        assert!(guard(backing, vec![], 0xffff_fffc).is_err());
    }

    #[test]
    fn sparse_backing_does_not_weaken_code_write_exclusion() {
        assert!(
            Guard::new(
                0,
                vec![(0xff000000, 0xff000010)],
                vec![],
                vec![(0xff000000, 0xff000004)],
                0x3000,
                8,
                vec![(0, 0x4000), (0xff000000, 0xff001000)]
            )
            .is_err()
        );
    }

    #[test]
    fn sparse_backing_rejects_overlapping_or_out_of_address_space_backing() {
        assert!(guard(vec![(0, 0x4000), (0x3000, 0x5000)], vec![], 0x2000).is_err());
        assert!(guard(vec![(0, 0x1_0000_0001)], vec![], 0x3000).is_err());
    }
}

impl Guard {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        core: usize,
        executable: Vec<Span>,
        readable: Vec<Span>,
        writable: Vec<Span>,
        return_address: u64,
        store_limit: usize,
        physical_backing: Vec<Span>,
    ) -> Result<Self, String> {
        let physical_backing = spans(physical_backing, &[(0, 1 << 32)])?;
        let executable = spans(executable, &physical_backing)?;
        let readable = spans(readable, &physical_backing)?;
        let writable = spans(writable, &physical_backing)?;
        if executable.is_empty()
            || store_limit == 0
            || store_limit > 1 << 20
            || return_address & 3 != 0
            || !contains(&physical_backing, return_address, 4)
            || executable
                .iter()
                .any(|&(lo, hi)| return_address >= lo && return_address < hi)
            || writable.iter().any(|&(lo, hi)| {
                executable.iter().any(|&(a, b)| lo < b && a < hi)
                    || (lo < return_address + 4 && return_address < hi)
            })
        {
            return Err(
                "Invalid driver guard executable ranges, return sentinel or store bound".into(),
            );
        }
        Ok(Self {
            core,
            executable,
            readable,
            writable,
            return_address,
            store_limit,
            stores: Vec::new(),
            model_readable: Vec::new(),
            model_writable: Vec::new(),
            model_writes: Vec::new(),
            physical_backing,
            mmio: Vec::new(),
            mmio_accesses: Vec::new(),
            returned: false,
            failure: None,
            accepted: 0,
        })
    }

    pub fn stopped(&self) -> bool {
        self.returned || self.failure.is_some()
    }

    /// Select CPU device access before execution, bounded by actual model windows.
    pub fn with_mmio(
        mut self,
        mut policy: Vec<MmioAccess>,
        windows: Vec<Span>,
    ) -> Result<Self, String> {
        if self.accepted != 0
            || self.stopped()
            || !self.mmio_accesses.is_empty()
            || !self.model_writes.is_empty()
        {
            return Err("MMIO policy must be selected before guarded execution".into());
        }
        let windows = spans(windows, &[(0, 1 << 32)])?;
        policy.sort_by_key(|entry| entry.start);
        if policy.len() > 4096
            || policy.iter().any(|entry| {
                !matches!(entry.width, 1 | 2 | 4)
                    || entry.start >= entry.end
                    || entry.start % u64::from(entry.width.max(1)) != 0
                    || entry.end % u64::from(entry.width.max(1)) != 0
                    || (!entry.read && !entry.write)
                    || !contains(&windows, entry.start, entry.end - entry.start)
                    || self
                        .physical_backing
                        .iter()
                        .any(|&(lo, hi)| entry.start < hi && lo < entry.end)
            })
            || policy.windows(2).any(|pair| pair[0].end > pair[1].start)
        {
            return Err(
                "MMIO policy requires disjoint aligned model-owned ranges outside RAM".into(),
            );
        }
        self.mmio = policy;
        Ok(self)
    }

    fn mmio_permits(&self, address: u64, size: u64, write: bool) -> bool {
        self.mmio.iter().any(|entry| {
            size == u64::from(entry.width)
                && address % size == 0
                && contains(&[(entry.start, entry.end)], address, size)
                && if write { entry.write } else { entry.read }
        })
    }

    /// Called before a modeled read can consume data or a write can alter state.
    pub fn mmio_access(
        &mut self,
        core: usize,
        address: u64,
        size: u32,
        write: bool,
    ) -> Result<(), String> {
        if self.stopped() {
            return Err("Driver guard MMIO access after stop".into());
        }
        let error = if core != self.core || !self.mmio_permits(address, u64::from(size), write) {
            Some(format!(
                "Driver guard rejected MMIO {} at {address:#x} size={size}",
                if write { "write" } else { "read" }
            ))
        } else if self.mmio_accesses.len() >= self.store_limit {
            Some("Driver guard MMIO record bound reached before dispatch".into())
        } else {
            None
        };
        if let Some(error) = error {
            self.failure = Some(error.clone());
            return Err(error);
        }
        self.mmio_accesses.push((address, size, write));
        Ok(())
    }

    /// Narrow device-initiated memory to subsets of the caller's RAM policy.
    /// This does not admit CPU MMIO or attach/enable any device model.
    pub fn with_model_memory(
        mut self,
        readable: Vec<Span>,
        writable: Vec<Span>,
    ) -> Result<Self, String> {
        if self.accepted != 0
            || self.stopped()
            || !self.model_writes.is_empty()
            || !self.mmio_accesses.is_empty()
        {
            return Err("Model memory policy must be selected before guarded execution".into());
        }
        self.model_readable = spans(readable, &self.readable)?;
        self.model_writable = spans(writable, &self.writable)?;
        Ok(self)
    }

    /// Preflight a complete host-model transaction before touching RAM.
    pub fn model_access(&mut self, address: u64, size: u64, write: bool) -> Result<(), String> {
        if self.stopped() {
            return Err("Driver guard model access after stop".into());
        }
        let policy = if write {
            &self.model_writable
        } else {
            &self.model_readable
        };
        let error = if size == 0 || !contains(policy, address, size) {
            Some(format!(
                "Driver guard rejected model {} at {address:#x} size={size}",
                if write { "write" } else { "read" }
            ))
        } else if write && self.model_writes.len() >= self.store_limit {
            Some("Driver guard model write record bound reached before transaction".into())
        } else {
            None
        };
        if let Some(error) = error {
            self.failure = Some(error.clone());
            return Err(error);
        }
        if write {
            self.model_writes.push((address, size));
        }
        Ok(())
    }

    pub fn fetch(&mut self, core: usize, address: u64, size: u32) -> Result<bool, String> {
        if self.stopped() {
            return Ok(false);
        }
        if core != self.core {
            return Err("Driver guard belongs to another core".into());
        }
        if size != 4 || address & 3 != 0 {
            return Err(format!(
                "Driver guard requires aligned A32 fetch at {address:#x}"
            ));
        }
        if address == self.return_address {
            self.returned = true;
            return Ok(false);
        }
        if !contains(&self.executable, address, 4) {
            return Err(format!("Driver guard rejected fetch at {address:#x}"));
        }
        Ok(true)
    }

    pub fn preflight(
        &mut self,
        pc: u64,
        word: u32,
        cpsr: u32,
        reg: impl Fn(usize) -> Result<u32, String>,
    ) -> Result<(), String> {
        if cpsr & (0x20 | 0x200) != 0 || cpsr & 0xc0 != 0xc0 {
            return Err("Driver guard requires little-endian ARM state with IRQ/FIQ masked".into());
        }
        let condition = word >> 28;
        let (n, z, c, v) = (
            cpsr >> 31 != 0,
            cpsr & (1 << 30) != 0,
            cpsr & (1 << 29) != 0,
            cpsr & (1 << 28) != 0,
        );
        let passed = match condition {
            0 => z,
            1 => !z,
            2 => c,
            3 => !c,
            4 => n,
            5 => !n,
            6 => v,
            7 => !v,
            8 => c && !z,
            9 => !c || z,
            10 => n == v,
            11 => n != v,
            12 => !z && n == v,
            13 => z || n != v,
            _ => true,
        };
        if !passed {
            self.accepted += 1;
            return Ok(());
        }
        if condition == 15 {
            if matches!(word, 0xf57ff01f | 0xf57ff04f | 0xf57ff05f | 0xf57ff06f) {
                self.accepted += 1;
                return Ok(());
            }
            return Err(format!(
                "Driver guard unsupported unconditional instruction {word:#010x} at {pc:#x}"
            ));
        }
        let r = |index: usize| {
            if index == 15 {
                Ok((pc as u32).wrapping_add(8))
            } else {
                reg(index)
            }
        };
        let rn = ((word >> 16) & 15) as usize;
        let load = word & (1 << 20) != 0;
        let mut accesses: Vec<(u64, u64, bool)> = Vec::new();
        match (word >> 25) & 7 {
            2 | 3 => {
                if word & (1 << 25) != 0 && word & (1 << 4) != 0 {
                    // A32 register-only extension, byte-order and bit-field
                    // families. The masks keep reserved/media encodings out.
                    let extend = matches!(
                        word & 0x0ff003f0,
                        0x06800070 | 0x06a00070 | 0x06b00070 | 0x06c00070 | 0x06e00070 | 0x06f00070
                    );
                    let reverse = matches!(
                        word & 0x0fff0ff0,
                        0x06bf0f30 | 0x06bf0fb0 | 0x06ff0f30 | 0x06ff0fb0
                    );
                    let bitfield =
                        matches!(word & 0x0fe00070, 0x07a00050 | 0x07e00050 | 0x07c00010);
                    let divide = matches!(word & 0x0ff0f0f0, 0x0710f010 | 0x0730f010);
                    if extend || reverse || bitfield || divide {
                        self.accepted += 1;
                        return Ok(());
                    }
                    return Err(format!(
                        "Driver guard unsupported media instruction {word:#010x} at {pc:#x}"
                    ));
                }
                let offset = if word & (1 << 25) == 0 {
                    word & 0xfff
                } else {
                    let value = r((word & 15) as usize)?;
                    let shift = (word >> 7) & 31;
                    match (word >> 5) & 3 {
                        0 => value << shift,
                        1 => {
                            if shift == 0 {
                                0
                            } else {
                                value >> shift
                            }
                        }
                        2 => {
                            if shift == 0 {
                                ((value as i32) >> 31) as u32
                            } else {
                                ((value as i32) >> shift) as u32
                            }
                        }
                        _ => {
                            if shift == 0 {
                                ((u32::from(c)) << 31) | (value >> 1)
                            } else {
                                value.rotate_right(shift)
                            }
                        }
                    }
                };
                let base = r(rn)?;
                let adjusted = if word & (1 << 23) != 0 {
                    base.wrapping_add(offset)
                } else {
                    base.wrapping_sub(offset)
                };
                let address = if word & (1 << 24) != 0 {
                    adjusted
                } else {
                    base
                };
                accesses.push((
                    address as u64,
                    if word & (1 << 22) != 0 { 1 } else { 4 },
                    !load,
                ));
            }
            4 => {
                let count = (word & 0xffff).count_ones();
                if count == 0 || rn == 15 || word & (1 << 22) != 0 {
                    return Err("Driver guard unsupported block transfer".into());
                }
                let base = r(rn)?;
                let before = word & (1 << 24) != 0;
                let up = word & (1 << 23) != 0;
                let first = if up {
                    base.wrapping_add(if before { 4 } else { 0 })
                } else {
                    base.wrapping_sub(4 * count)
                        .wrapping_add(if before { 0 } else { 4 })
                };
                for index in 0..count {
                    accesses.push((first.wrapping_add(index * 4) as u64, 4, !load));
                }
            }
            0 if word & 0x90 == 0x90 => {
                let kind = (word >> 5) & 3;
                if kind == 0 {
                    let exclusive_load = word & 0x0ff00fff == 0x01900f9f;
                    let exclusive_store = word & 0x0ff00ff0 == 0x01800f90;
                    if exclusive_load || exclusive_store {
                        let rd = ((word >> 12) & 15) as usize;
                        let rt = (word & 15) as usize;
                        let address = r(rn)?;
                        if rn == 15
                            || rd == 15
                            || address & 3 != 0
                            || (exclusive_store && (rt == 15 || rd == rn || rd == rt))
                        {
                            return Err("Driver guard invalid word-exclusive access".into());
                        }
                        // The CPU owns the exclusive monitor. This guard only
                        // admits the complete RAM access before execution.
                        accesses.push((address as u64, 4, exclusive_store));
                    } else if word & 0x0fc000f0 != 0x00000090
                        && word & 0x0f8000f0 != 0x00800090
                        && word & 0x0ff000f0 != 0x00600090
                    {
                        return Err(format!(
                            "Driver guard unsupported atomic/misc instruction {word:#010x} at {pc:#x}"
                        ));
                    }
                } else {
                    let offset = if word & (1 << 22) != 0 {
                        ((word >> 4) & 0xf0) | (word & 15)
                    } else {
                        r((word & 15) as usize)?
                    };
                    let base = r(rn)?;
                    let adjusted = if word & (1 << 23) != 0 {
                        base.wrapping_add(offset)
                    } else {
                        base.wrapping_sub(offset)
                    };
                    let address = if word & (1 << 24) != 0 {
                        adjusted
                    } else {
                        base
                    };
                    let (width, write) = match (kind, load) {
                        (1, _) => (2, !load),
                        (2, true) => (1, false),
                        (3, true) => (2, false),
                        (2, false) => (8, false),
                        (3, false) => (8, true),
                        _ => unreachable!(),
                    };
                    if width == 8 {
                        let rd = (word >> 12) & 15;
                        if rd & 1 != 0 || rd >= 14 {
                            return Err("Driver guard invalid doubleword pair".into());
                        }
                        accesses.push((address as u64, 4, write));
                        accesses.push((address.wrapping_add(4) as u64, 4, write));
                    } else {
                        accesses.push((address as u64, width, write));
                    }
                }
            }
            0 | 1 | 5 => (),
            _ => {
                return Err(format!(
                    "Driver guard unsupported coprocessor/system instruction {word:#010x} at {pc:#x}"
                ));
            }
        }
        let stores = accesses.iter().filter(|a| a.2).count();
        if stores > self.store_limit - self.stores.len() {
            return Err("Driver guard store record bound reached before instruction".into());
        }
        let plain_single = accesses.len() == 1
            && (matches!((word >> 25) & 7, 2 | 3) || word & 0x0e0000f0 == 0x000000b0);
        for (address, size, write) in accesses {
            let allowed = if write {
                &self.writable
            } else {
                &self.readable
            };
            if !contains(allowed, address, size)
                && !(plain_single && self.mmio_permits(address, size, write))
            {
                return Err(format!(
                    "Driver guard rejected {} at {address:#x} size={size} pc={pc:#x}",
                    if write { "write" } else { "read" }
                ));
            }
        }
        self.accepted += 1;
        Ok(())
    }

    pub fn store(&mut self, core: usize, pc: u64, address: u64, size: u32, value: u64) {
        self.stores.push((core, pc, address, size, value));
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            core: self.core,
            executable: self.executable.clone(),
            readable: self.readable.clone(),
            writable: self.writable.clone(),
            return_address: self.return_address,
            store_limit: self.store_limit,
            stores: self.stores.clone(),
            model_readable: self.model_readable.clone(),
            model_writable: self.model_writable.clone(),
            model_writes: self.model_writes.clone(),
            mmio: self.mmio.clone(),
            mmio_accesses: self.mmio_accesses.clone(),
            returned: self.returned,
            failure: self.failure.clone(),
            accepted_instructions: self.accepted,
            scope: if self.mmio.is_empty()
                && self.model_readable.is_empty()
                && self.model_writable.is_empty()
            {
                "MMU-off little-endian A32 integer routine; explicit RAM ranges; no device access"
            } else {
                "MMU-off little-endian A32 integer routine; explicit RAM and device policies"
            },
        }
    }
}

#[derive(serde::Serialize)]
pub struct Snapshot {
    pub core: usize,
    pub executable: Vec<(u64, u64)>,
    pub readable: Vec<(u64, u64)>,
    pub writable: Vec<(u64, u64)>,
    pub return_address: u64,
    pub store_limit: usize,
    pub stores: Vec<(usize, u64, u64, u32, u64)>,
    pub model_readable: Vec<(u64, u64)>,
    pub model_writable: Vec<(u64, u64)>,
    pub model_writes: Vec<(u64, u64)>,
    pub mmio: Vec<MmioAccess>,
    pub mmio_accesses: Vec<(u64, u32, bool)>,
    pub returned: bool,
    pub failure: Option<String>,
    pub accepted_instructions: u64,
    pub scope: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guard() -> Guard {
        Guard::new(
            0,
            vec![(0x1000, 0x1010)],
            vec![(0x2000, 0x2004)],
            vec![(0x3000, 0x3004)],
            0x4000,
            16,
            vec![(0, 0x5000)],
        )
        .unwrap()
    }

    #[test]
    fn model_memory_is_denied_until_explicitly_narrowed() {
        let mut isolated = guard();
        assert!(isolated.model_access(0x3000, 4, true).is_err());
        assert!(isolated.snapshot().model_writes.is_empty());
        assert!(isolated.failure.is_some());
        assert!(
            guard()
                .with_model_memory(vec![(0x2000, 0x2005)], vec![])
                .is_err()
        );
        assert!(
            guard()
                .with_model_memory(vec![], vec![(0x1000, 0x1004)])
                .is_err()
        );
        assert!(
            guard()
                .with_model_memory(vec![], vec![(0x3000, 0x3005)])
                .is_err()
        );
    }

    fn mmio_policy() -> MmioAccess {
        MmioAccess {
            start: 0xfffd8000,
            end: 0xfffd8004,
            width: 4,
            read: true,
            write: true,
        }
    }

    fn mmio_guard() -> Guard {
        guard()
            .with_mmio(vec![mmio_policy()], vec![(0xfffd8000, 0xfffd8100)])
            .unwrap()
    }

    #[test]
    fn mmio_policy_rejects_ram_holes_overlap_width_and_unowned_ranges() {
        let mut invalid = Vec::new();
        for width in [0, 3, 8] {
            invalid.push(MmioAccess {
                width,
                ..mmio_policy()
            });
        }
        invalid.push(MmioAccess {
            start: 0xfffd8001,
            ..mmio_policy()
        });
        invalid.push(MmioAccess {
            end: 0xfffd8003,
            ..mmio_policy()
        });
        invalid.push(MmioAccess {
            read: false,
            write: false,
            ..mmio_policy()
        });
        invalid.push(MmioAccess {
            start: 0xfffd8100,
            end: 0xfffd8104,
            ..mmio_policy()
        });
        invalid.push(MmioAccess {
            start: 0x2000,
            end: 0x2004,
            ..mmio_policy()
        });
        for entry in invalid {
            assert!(
                guard()
                    .with_mmio(
                        vec![entry],
                        vec![(0x2000, 0x2004), (0xfffd8000, 0xfffd8100)]
                    )
                    .is_err()
            );
        }
        assert!(
            guard()
                .with_mmio(
                    vec![mmio_policy(), mmio_policy()],
                    vec![(0xfffd8000, 0xfffd8100)]
                )
                .is_err()
        );
        assert!(
            guard()
                .with_mmio(vec![mmio_policy()], vec![(0, 1u64 << 33)])
                .is_err()
        );
    }

    #[test]
    fn single_mmio_is_preflighted_but_exclusives_and_multi_accesses_are_not_admitted() {
        let reg = |_| Ok(0xfffd8000);
        assert!(guard().preflight(0x1000, 0xe5901000, 0xd3, reg).is_err());
        assert!(
            mmio_guard()
                .preflight(0x1000, 0xe5901000, 0xd3, reg)
                .is_ok()
        );
        assert!(
            mmio_guard()
                .preflight(0x1000, 0xe5801000, 0xd3, reg)
                .is_ok()
        );
        for word in [0xe5d01000, 0xe1932f9f, 0xe1832f91, 0xe8900006] {
            assert!(mmio_guard().preflight(0x1000, word, 0xd3, reg).is_err());
        }
        let mut multiple = guard()
            .with_mmio(
                vec![MmioAccess {
                    end: 0xfffd8010,
                    ..mmio_policy()
                }],
                vec![(0xfffd8000, 0xfffd8100)],
            )
            .unwrap();
        assert!(multiple.preflight(0x1000, 0xe8900006, 0xd3, reg).is_err());
        for (width, words) in [
            (1u32, [0xe5d01000, 0xe5c01000]),
            (2, [0xe1d010b0, 0xe1c010b0]),
        ] {
            for word in words {
                let mut narrow = guard()
                    .with_mmio(
                        vec![MmioAccess {
                            end: 0xfffd8000 + u64::from(width),
                            width,
                            ..mmio_policy()
                        }],
                        vec![(0xfffd8000, 0xfffd8100)],
                    )
                    .unwrap();
                assert!(narrow.preflight(0x1000, word, 0xd3, reg).is_ok());
                assert!(mmio_guard().preflight(0x1000, word, 0xd3, reg).is_err());
            }
        }
        let mut readonly = guard()
            .with_mmio(
                vec![MmioAccess {
                    write: false,
                    ..mmio_policy()
                }],
                vec![(0xfffd8000, 0xfffd8100)],
            )
            .unwrap();
        assert!(readonly.preflight(0x1000, 0xe5801000, 0xd3, reg).is_err());
    }

    #[test]
    fn mmio_dispatch_checks_direction_width_core_extent_and_sticky_denial() {
        let mut allowed = mmio_guard();
        allowed.mmio_access(0, 0xfffd8000, 4, false).unwrap();
        allowed.mmio_access(0, 0xfffd8000, 4, true).unwrap();
        assert_eq!(
            allowed.snapshot().mmio_accesses,
            vec![(0xfffd8000, 4, false), (0xfffd8000, 4, true)]
        );
        for (core, address, width) in [
            (1, 0xfffd8000, 4),
            (0, 0xfffd8000, 0),
            (0, 0xfffd8000, 1),
            (0, 0xfffd8001, 4),
            (0, 0xfffd8004, 4),
            (0, u64::MAX, 4),
        ] {
            let mut denied = mmio_guard();
            assert!(denied.mmio_access(core, address, width, false).is_err());
            assert!(denied.snapshot().mmio_accesses.is_empty());
            assert!(denied.mmio_access(0, 0xfffd8000, 4, true).is_err());
        }
        let mut writeonly = guard()
            .with_mmio(
                vec![MmioAccess {
                    read: false,
                    ..mmio_policy()
                }],
                vec![(0xfffd8000, 0xfffd8100)],
            )
            .unwrap();
        assert!(writeonly.mmio_access(0, 0xfffd8000, 4, false).is_err());
    }

    #[test]
    fn mmio_records_are_bounded_and_policies_cannot_change_after_access() {
        let mut bounded = mmio_guard();
        for _ in 0..16 {
            bounded.mmio_access(0, 0xfffd8000, 4, false).unwrap();
        }
        assert!(bounded.mmio_access(0, 0xfffd8000, 4, false).is_err());
        assert_eq!(bounded.snapshot().mmio_accesses.len(), 16);
        let mut used = mmio_guard();
        used.mmio_access(0, 0xfffd8000, 4, true).unwrap();
        assert!(used.with_model_memory(vec![], vec![]).is_err());
        let mut returned = mmio_guard();
        returned.fetch(0, 0x4000, 4).unwrap();
        assert!(returned.mmio_access(0, 0xfffd8000, 4, false).is_err());
    }

    #[test]
    fn model_transactions_preserve_direction_extent_and_sticky_failure() {
        let configured = || {
            guard()
                .with_model_memory(vec![(0x2000, 0x2004)], vec![(0x3000, 0x3004)])
                .unwrap()
        };
        let mut admitted = configured();
        admitted.model_access(0x2000, 4, false).unwrap();
        admitted.model_access(0x3000, 4, true).unwrap();
        assert_eq!(admitted.snapshot().model_writes, vec![(0x3000, 4)]);
        assert!(admitted.snapshot().stores.is_empty());
        for (address, size, write) in [
            (0x3000, 4, false),
            (0x2000, 4, true),
            (0x3001, 4, true),
            (0x3000, 0, true),
            (0x3000, u64::MAX, true),
        ] {
            let mut denied = configured();
            assert!(denied.model_access(address, size, write).is_err());
            assert!(denied.snapshot().model_writes.is_empty());
            assert!(denied.model_access(0x3000, 4, true).is_err());
        }
    }

    #[test]
    fn model_write_records_are_bounded_and_stop_at_return() {
        let mut bounded = guard()
            .with_model_memory(vec![], vec![(0x3000, 0x3004)])
            .unwrap();
        for _ in 0..16 {
            bounded.model_access(0x3000, 4, true).unwrap();
        }
        assert!(bounded.model_access(0x3000, 4, true).is_err());
        assert_eq!(bounded.snapshot().model_writes.len(), 16);
        let mut returned = guard()
            .with_model_memory(vec![], vec![(0x3000, 0x3004)])
            .unwrap();
        assert!(!returned.fetch(0, 0x4000, 4).unwrap());
        assert!(returned.model_access(0x3000, 4, true).is_err());
        assert!(returned.snapshot().model_writes.is_empty());
    }

    #[test]
    fn model_policy_cannot_change_after_execution_or_failure() {
        let mut running = guard();
        running
            .preflight(0x1000, 0xe1a00000, 0xd3, |_| Ok(0))
            .unwrap();
        assert!(
            running
                .with_model_memory(vec![], vec![(0x3000, 0x3004)])
                .is_err()
        );
        let mut failed = guard();
        assert!(failed.model_access(0x3000, 4, true).is_err());
        assert!(
            failed
                .with_model_memory(vec![], vec![(0x3000, 0x3004)])
                .is_err()
        );
    }

    #[test]
    fn word_exclusives_check_complete_ram_access_and_register_constraints() {
        let reg = |address| move |_index| Ok(address);
        assert!(
            guard()
                .preflight(0x1000, 0xe1932f9f, 0xd3, reg(0x2000))
                .is_ok()
        );
        assert!(
            guard()
                .preflight(0x1000, 0xe1832f91, 0xd3, reg(0x3000))
                .is_ok()
        );
        assert!(
            guard()
                .preflight(0x1000, 0xe1932f9f, 0xd3, reg(0x2004))
                .is_err()
        );
        assert!(
            guard()
                .preflight(0x1000, 0xe1832f91, 0xd3, reg(0x2000))
                .is_err()
        );
        assert!(
            guard()
                .preflight(0x1000, 0xe1832f91, 0xd3, reg(0x3001))
                .is_err()
        );
        for word in [0xe19f2f9f, 0xe193ff9f, 0xe1833f91, 0xe1831f91, 0xe1832f9f] {
            assert!(guard().preflight(0x1000, word, 0xd3, reg(0x3000)).is_err());
        }
        assert!(guard().preflight(0x1000, 0xf57ff01f, 0xd3, reg(0)).is_ok());
    }
}
