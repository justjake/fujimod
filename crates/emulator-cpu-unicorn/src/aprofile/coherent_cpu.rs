//! The processor side of [`CoherentRanges`]: stores and loads inside the
//! declared ranges feed the model, and data-cache maintenance instructions
//! (CP15 c7) drive its clean and invalidate operations.
//!
//! Opt-in through [`Core::set_coherence`]. Without a domain nothing is
//! hooked and no instruction is observed. Processors of one cluster attach
//! the same [`CoherenceDomain`]: the cluster's caches are one coherent
//! processor-side cache, and each processor hooks the declared ranges. Everything here follows from the
//! declared ranges and the instruction stream; no program counter or guest
//! global is consulted.
//!
//! Ranges are physical. A maintenance operation names a virtual address, which
//! is taken as the physical address, so declared ranges must be mapped
//! identically (virtual equals physical) in this version.
//!
//! With the MMU on, an access inside a declared range first looks up its
//! short-descriptor memory attributes (see `translation_attributes`). A
//! store through a non-cacheable mapping does not dirty a line and a load
//! through one ignores stale copies. The result is kept per 4 KiB page until
//! a translation-affecting CP15 write (c1, c2, c3, c8, c13).
use super::coherence::{CoherentRanges, Patch};
use super::translation_attributes::Walk;
use super::*;
use crate::memory::HashMap;

/// Line size and physical `[begin, end)` ranges of device-visible memory.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CoherenceOptions {
    pub line_bytes: u32,
    pub ranges: Vec<(u64, u64)>,
}

struct Shared {
    model: CoherentRanges,
    ranges: Vec<(u64, u64)>,
    /// Processor-view bytes to put back after a load saw stale bytes.
    restore: Vec<Patch>,
}

/// The coherent state of one cluster: shared by every attached processor.
#[derive(Clone)]
pub struct CoherenceDomain(Rc<RefCell<Shared>>);
impl CoherenceDomain {
    pub fn new(options: &CoherenceOptions) -> Result<Self, String> {
        Ok(Self(Rc::new(RefCell::new(Shared {
            model: CoherentRanges::new(options.line_bytes, &options.ranges)?,
            ranges: options.ranges.clone(),
            restore: Vec::new(),
        }))))
    }
}

/// One processor's attachment to a domain.
pub(super) struct Coherent {
    domain: CoherenceDomain,
    hooks: Vec<HookId>,
    /// Whether the mapping of a 4 KiB virtual page is cacheable.
    cacheable: HashMap<u32, bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Maintenance {
    Clean,
    Invalidate,
    CleanInvalidate,
}

/// An `MCR p15, 0, Rt, c7, ...` data-cache operation: what it does and
/// whether Rt is a set/way value instead of an address.
pub(super) fn maintenance(word: u32) -> Option<(Maintenance, bool)> {
    if word & 0x0fff_0f10 != 0x0e07_0f10 {
        return None;
    }
    let crm = (word & 15) as u8;
    let opc2 = ((word >> 5) & 7) as u8;
    Some(match (crm, opc2) {
        (6, 1) => (Maintenance::Invalidate, false),
        (10, 1) | (11, 1) => (Maintenance::Clean, false),
        (14, 1) => (Maintenance::CleanInvalidate, false),
        (6, 2) => (Maintenance::Invalidate, true),
        (10, 2) => (Maintenance::Clean, true),
        (14, 2) => (Maintenance::CleanInvalidate, true),
        _ => return None,
    })
}

/// An `MCR p15` write that can change a translation or its attributes.
pub(super) fn translation_change(word: u32) -> bool {
    word & 0x0f10_0f10 == 0x0e00_0f10 && matches!((word >> 16) & 15, 1 | 2 | 3 | 8 | 13)
}

fn condition(cond: u32, cpsr: u32) -> bool {
    let (n, z, c, v) = (
        cpsr >> 31 & 1 != 0,
        cpsr >> 30 & 1 != 0,
        cpsr >> 29 & 1 != 0,
        cpsr >> 28 & 1 != 0,
    );
    match cond {
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
    }
}

impl Execution {
    pub(super) fn is_coherent(&self) -> bool {
        self.coherent.get()
    }

    fn put_back(&self, patches: &[Patch]) -> Result<(), String> {
        for p in patches {
            self.cpu.write_memory(p.address, &p.bytes)?;
        }
        Ok(())
    }

    fn flush_restore(&self, shared: &mut Shared) -> Result<(), String> {
        let pending = std::mem::take(&mut shared.restore);
        self.put_back(&pending)
    }

    fn cp15(&self, crn: u32, opc2: u32) -> Result<u32, String> {
        Ok(self.cpu.coprocessor(
            arm::Coprocessor {
                cp: 15,
                opc1: 0,
                crn,
                crm: 0,
                opc2,
                secure: 0,
                wide: false,
            },
            None,
        )? as u32)
    }

    /// Whether the mapping of `address` is cacheable, for the declared range
    /// being modeled. Without the MMU everything is.
    fn cacheable_page(&self, pages: &mut HashMap<u32, bool>, address: u64) -> Result<bool, String> {
        let va = address as u32;
        if let Some(&known) = pages.get(&(va >> 12)) {
            return Ok(known);
        }
        let sctlr = self.cp15(1, 0)?;
        let cacheable = if sctlr & 1 == 0 {
            true
        } else {
            let walk = Walk {
                ttbr0: self.cp15(2, 0)?,
                ttbr1: self.cp15(2, 1)?,
                ttbcr: self.cp15(2, 2)?,
                tex_remap: sctlr & (1 << 28) != 0,
            };
            walk.cacheable(va, |physical| {
                let mut word = [0; 4];
                self.cpu.read_memory(physical, &mut word)?;
                Ok(u32::from_le_bytes(word))
            })?
        };
        pages.insert(va >> 12, cacheable);
        Ok(cacheable)
    }

    /// A load or store inside a declared range, before it takes effect.
    pub(super) fn coherent_access(
        &self,
        access: Access,
        address: u64,
        size: usize,
    ) -> Result<(), String> {
        let mut guard = self.coherence.borrow_mut();
        let Some(core) = guard.as_mut() else {
            return Ok(());
        };
        let mut shared = core.domain.0.borrow_mut();
        self.flush_restore(&mut shared)?;
        if access != Access::Fetch && !self.cacheable_page(&mut core.cacheable, address)? {
            return Ok(());
        }
        let mut bytes = vec![0; size];
        match access {
            Access::Write => {
                self.cpu.read_memory(address, &mut bytes)?;
                shared.model.cpu_store(address, size, &bytes);
            }
            Access::Read => {
                if let Some(stale) = shared.model.cpu_load(address, size) {
                    self.cpu.read_memory(address, &mut bytes)?;
                    shared.restore.push(Patch {
                        address,
                        bytes: bytes.clone(),
                    });
                    stale.apply(address, &mut bytes);
                    self.cpu.write_memory(address, &bytes)?;
                }
            }
            Access::Fetch => {}
        }
        Ok(())
    }

    /// An observed instruction: apply a data-cache maintenance operation.
    pub(super) fn coherent_instruction(&self, address: u64) -> Result<(), String> {
        let mut guard = self.coherence.borrow_mut();
        let Some(core) = guard.as_mut() else {
            return Ok(());
        };
        let mut shared = core.domain.0.borrow_mut();
        self.flush_restore(&mut shared)?;
        let mut bytes = [0; 4];
        self.cpu.read_virtual(address, Access::Fetch, &mut bytes)?;
        let word = u32::from_le_bytes(bytes);
        if translation_change(word) {
            core.cacheable.clear();
            return Ok(());
        }
        let Some((operation, by_way)) = maintenance(word) else {
            return Ok(());
        };
        let mut cpsr = [0; 8];
        self.cpu.read_register(arm::CPSR, &mut cpsr)?;
        if !condition(word >> 28, u64::from_le_bytes(cpsr) as u32) {
            return Ok(());
        }
        let model = &mut shared.model;
        let patches = if by_way {
            match operation {
                Maintenance::Clean => model.clean_all(),
                Maintenance::Invalidate => model.invalidate_all(),
                Maintenance::CleanInvalidate => model.clean_invalidate_all(),
            }
        } else {
            let mut value = [0; 8];
            self.cpu
                .read_register(arm::CORE[(word >> 12 & 15) as usize], &mut value)?;
            let mva = u64::from_le_bytes(value) as u32 as u64;
            match operation {
                Maintenance::Clean => model.clean_line(mva),
                Maintenance::Invalidate => model.invalidate_line(mva),
                Maintenance::CleanInvalidate => model.clean_invalidate_line(mva),
            }
        };
        self.put_back(&patches)
    }
}

unsafe fn access(access: Access, address: u64, size: i32, _: i64, data: *mut c_void) {
    let e = unsafe { &*data.cast::<Execution>() };
    e.guarded(|| e.coherent_access(access, address, size as usize));
}

impl Core {
    /// Attach this processor to a coherence domain over device-visible
    /// physical ranges (`None` detaches it). Needs a stopped ARMv7-A
    /// processor; declared ranges must be identity mapped.
    pub fn set_coherence(&self, domain: Option<&CoherenceDomain>) -> Result<(), String> {
        let e = &self.execution;
        if e.active.get() || !e.a32 {
            return Err("coherent ranges need a stopped ARMv7-A processor".into());
        }
        if let Some(old) = e.coherence.borrow_mut().take() {
            for hook in old.hooks {
                e.cpu.remove_hook(hook)?;
            }
        }
        e.coherent.set(false);
        if let Some(domain) = domain {
            let data = Rc::as_ptr(e).cast_mut().cast();
            let mut hooks = Vec::new();
            for &(begin, end) in &domain.0.borrow().ranges {
                hooks.push(unsafe {
                    e.cpu.install_hook(Hook::Memory {
                        range: HookRange::Inclusive {
                            begin,
                            end: end - 1,
                        },
                        read: true,
                        write: true,
                        callback: access,
                        data,
                    })
                }?);
            }
            *e.coherence.borrow_mut() = Some(Coherent {
                domain: domain.clone(),
                hooks,
                cacheable: HashMap::default(),
            });
            e.coherent.set(true);
        }
        // Maintenance instructions are observed only while attached.
        e.cpu.invalidate_code(None)
    }

    /// What a bus master reading `buffer.len()` bytes at `address` sees.
    pub fn coherent_device_read(&self, address: u64, buffer: &mut [u8]) -> Result<(), String> {
        let e = &self.execution;
        let guard = e.coherence.borrow();
        let mut shared = guard.as_ref().map(|c| c.domain.0.borrow_mut());
        if let Some(shared) = shared.as_mut() {
            e.flush_restore(shared)?;
        }
        e.cpu.read_memory(address, buffer)?;
        if let Some(shared) = shared.as_ref() {
            shared.model.device_read(address, buffer);
        }
        Ok(())
    }

    /// A bus master writing `bytes` at `address`.
    pub fn coherent_device_write(&self, address: u64, bytes: &[u8]) -> Result<(), String> {
        let e = &self.execution;
        let guard = e.coherence.borrow();
        if let Some(core) = guard.as_ref() {
            let mut shared = core.domain.0.borrow_mut();
            e.flush_restore(&mut shared)?;
            let mut before = vec![0; bytes.len()];
            e.cpu.read_memory(address, &mut before)?;
            shared.model.device_write(address, bytes, &before);
        }
        e.cpu.write_memory(address, bytes)
    }
}
