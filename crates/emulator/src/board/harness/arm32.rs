//! Explicit, MMU-off routine instrumentation. This never selects device behavior.

type Span = (u64, u64);
type Store = (usize, u64, u64, u32, u64);

pub struct Guard {
    pub core: usize,
    executable: Vec<Span>,
    readable: Vec<Span>,
    writable: Vec<Span>,
    return_address: u64,
    store_limit: usize,
    stores: Vec<Store>,
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

fn spans(mut values: Vec<Span>, ram_size: u64) -> Result<Vec<Span>, String> {
    values.sort_unstable();
    if values.len() > 4096
        || values
            .iter()
            .any(|&(low, high)| low >= high || high > ram_size)
        || values.windows(2).any(|p| p[0].1 > p[1].0)
    {
        return Err("Driver guard ranges must be disjoint, nonempty physical RAM spans".into());
    }
    Ok(values)
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
        ram_size: u64,
    ) -> Result<Self, String> {
        let executable = spans(executable, ram_size)?;
        let readable = spans(readable, ram_size)?;
        let writable = spans(writable, ram_size)?;
        if executable.is_empty()
            || store_limit == 0
            || store_limit > 1 << 20
            || return_address & 3 != 0
            || return_address > ram_size.saturating_sub(4)
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
            returned: false,
            failure: None,
            accepted: 0,
        })
    }

    pub fn stopped(&self) -> bool {
        self.returned || self.failure.is_some()
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
            if matches!(word, 0xf57ff04f | 0xf57ff05f | 0xf57ff06f) {
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
                    if word & 0x0fc000f0 != 0x00000090
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
        for (address, size, write) in accesses {
            let allowed = if write {
                &self.writable
            } else {
                &self.readable
            };
            if !contains(allowed, address, size) {
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
            returned: self.returned,
            failure: self.failure.clone(),
            accepted_instructions: self.accepted,
            scope: "MMU-off little-endian A32 integer routine; explicit RAM ranges; no devices",
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
    pub returned: bool,
    pub failure: Option<String>,
    pub accepted_instructions: u64,
    pub scope: &'static str,
}
