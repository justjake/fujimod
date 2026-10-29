//! Checked construction and memory ownership for every execution backend.
use super::{Board, InterruptMap, bus::Bus, set};
use fujimod_emulator_cpu::{Cpu, Model, Register, memory::Backing, timeline::Ratio};
use fujimod_emulator_cpu_unicorn::Library;
use std::ptr::NonNull;

#[derive(Clone, Copy)]
pub enum Engine {
    Unicorn,
    Hypervisor,
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Processor {
    pub model: Model,
    pub registers: Vec<(Register, u64)>,
    pub released: bool,
    pub mprofile: Option<super::mprofile::Config>,
    pub rprofile: Option<fujimod_emulator_cpu_unicorn::rprofile::core::Options>,
}
impl Processor {
    /// Smallest page the processor backend maps.
    pub fn mapping_granule(&self) -> u64 {
        if self.model == Model::ArmV8A {
            0x4000
        } else {
            0x1000
        }
    }
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ram {
    pub base: u64,
    pub size: usize,
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryMap {
    pub core: usize,
    pub address: u64,
    pub physical: u64,
    pub size: usize,
    pub permissions: u32,
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Peripheral {
    pub core: usize,
    pub address: u64,
    pub physical: u64,
    pub size: usize,
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Graph {
    pub processors: Vec<Processor>,
    pub ram: Vec<Ram>,
    pub dma_mappings: Vec<super::memory::BusMapping>,
    pub decoders: Vec<super::decoder::Decoder>,
    pub mappings: Vec<MemoryMap>,
    pub peripherals: Vec<Peripheral>,
    pub instructions_per_tick: Ratio,
}
/// Host code cache per processor. An X70 walkthrough keeps about 63 MiB of
/// translated code in use. At 64 MiB the cache still filled and flushed, while
/// 128 MiB holds that working set about twice over. On native hosts this buffer
/// is a reservation whose pages are committed only as code is generated.
pub const TRANSLATION_CACHE_BYTES: u32 = 128 << 20;

pub struct Options {
    pub engine: Engine,
    translation_cache_size_bytes: Option<u32>,
}

impl Options {
    pub fn new(engine: Engine) -> Self {
        Self {
            engine,
            translation_cache_size_bytes: None,
        }
    }

    /// Set a smaller cache for focused tests.
    pub fn with_translation_cache_size_bytes(mut self, bytes: u32) -> Self {
        self.translation_cache_size_bytes = Some(bytes);
        self
    }
}

/// Browser code cache per processor. Emscripten serves it from Wasm heap that
/// is committed when allocated and never returned, and a phone tab is killed
/// once its memory passes a limit far below a desktop's. Before the 128 MiB
/// native size the browser ran with 16 MiB per processor; two X70 processors at
/// 128 MiB grew the tab by 224 MiB and iPhone Safari killed it after boot.
const BROWSER_TRANSLATION_CACHE_BYTES: u32 = 32 << 20;
const BROWSER_TRANSLATION_CACHE_HEADROOM_BYTES: u64 = 64 << 20;

/// Refuse construction when the remaining Emscripten heap cannot hold every
/// processor's browser cache plus headroom.
pub fn browser_translation_cache_size_bytes(
    remaining_heap_bytes: u64,
    processors: usize,
) -> Result<u32, String> {
    if processors == 0 {
        return Err("browser translation cache sizing requires a processor".into());
    }
    let required = u64::from(BROWSER_TRANSLATION_CACHE_BYTES)
        .checked_mul(processors as u64)
        .and_then(|total| total.checked_add(BROWSER_TRANSLATION_CACHE_HEADROOM_BYTES))
        .ok_or("browser translation cache size overflows")?;
    if remaining_heap_bytes < required {
        return Err(format!(
            "browser heap cannot fit the 32 MiB translation cache for each of {processors} processors plus 64 MiB of headroom"
        ));
    }
    Ok(BROWSER_TRANSLATION_CACHE_BYTES)
}

pub fn translation_cache_size_bytes(processors: usize) -> Result<u32, String> {
    if processors == 0 {
        return Ok(TRANSLATION_CACHE_BYTES);
    }
    #[cfg(target_os = "emscripten")]
    {
        browser_translation_cache_size_bytes(
            fujimod_emulator_cpu_unicorn::remaining_heap_bytes()?,
            processors,
        )
    }
    #[cfg(not(target_os = "emscripten"))]
    {
        let _ = processors;
        Ok(TRANSLATION_CACHE_BYTES)
    }
}
pub(super) struct Engines {
    unicorn: Option<Library>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    hypervisor: Option<fujimod_emulator_cpu_hvf::Vm>,
}
impl Engines {
    fn new(graph: &Graph, options: &Options) -> Result<Self, String> {
        if matches!(options.engine, Engine::Hypervisor)
            && graph
                .processors
                .iter()
                .any(|p| !matches!(p.model, Model::ArmV8A | Model::CortexM4))
        {
            return Err(
                "Hypervisor boards support AArch64 cores and Unicorn M-profile companions".into(),
            );
        }
        let need_uc = graph
            .processors
            .iter()
            .any(|p| matches!(options.engine, Engine::Unicorn) || p.model != Model::ArmV8A);
        let need_hvf = graph
            .processors
            .iter()
            .any(|p| matches!(options.engine, Engine::Hypervisor) && p.model == Model::ArmV8A);
        let unicorn = if need_uc {
            Some(Library::linked()?)
        } else {
            None
        };
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let hypervisor = if need_hvf {
            Some(fujimod_emulator_cpu_hvf::Vm::new()?)
        } else {
            None
        };
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        if need_hvf {
            return Err("Hypervisor engine requires macOS ARM64".into());
        }
        Ok(Self {
            unicorn,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            hypervisor,
        })
    }
    fn cpu(
        &self,
        processor: &Processor,
        options: &Options,
        translation_cache_size_bytes: u32,
        memory: &fujimod_emulator_cpu_unicorn::memory::Memory,
    ) -> Result<Box<dyn Cpu>, String> {
        let model = processor.model;
        if model == Model::ArmV8A && matches!(options.engine, Engine::Hypervisor) {
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            return Ok(Box::new(
                self.hypervisor
                    .as_ref()
                    .ok_or("Hypervisor engine was not constructed")?
                    .cpu(model)?,
            ));
            #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
            return Err("Hypervisor engine requires macOS ARM64".into());
        }
        let raw = self
            .unicorn
            .as_ref()
            .ok_or("Unicorn engine was not constructed")?
            .open(model, translation_cache_size_bytes)?;
        if model == Model::ArmV8A {
            return Ok(Box::new(fujimod_emulator_cpu_unicorn::aprofile::Core::new(
                raw,
                memory.clone(),
            )?));
        }
        if let Some(config) = processor.rprofile {
            Ok(Box::new(
                fujimod_emulator_cpu_unicorn::rprofile::core::Core::new(
                    raw,
                    config,
                    fujimod_emulator_cpu_unicorn::rprofile::core::Wiring {
                        memory: memory.clone(),
                    },
                )?,
            ))
        } else {
            Ok(Box::new(raw))
        }
    }
}
impl Graph {
    pub(super) fn validate_memory_map(
        &self,
        a: &MemoryMap,
        spans: &[(u64, u64)],
    ) -> Result<(), String> {
        let grain = self
            .processors
            .get(a.core)
            .map_or(0x4000, Processor::mapping_granule);
        if a.core >= self.processors.len()
            || a.size == 0
            || !a.address.is_multiple_of(grain)
            || !a.physical.is_multiple_of(grain)
            || !a.size.is_multiple_of(grain as usize)
            || a.permissions & !7 != 0
        {
            return Err("invalid RAM mapping".into());
        }
        let end = a
            .physical
            .checked_add(a.size as u64)
            .ok_or("RAM mapping overflow")?;
        a.address
            .checked_add(a.size as u64)
            .ok_or("RAM mapping address overflow")?;
        if self.processors[a.core].model == Model::CortexR5F
            && (end > 1 << 32 || a.address + a.size as u64 > 1 << 32)
        {
            return Err("R-profile memory mapping exceeds 32 bits".into());
        }
        if !spans.iter().any(|&(lo, hi)| a.physical >= lo && end <= hi) {
            return Err("RAM mapping has no physical backing".into());
        }
        Ok(())
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.processors.is_empty() || !self.processors.iter().any(|p| p.released) {
            return Err("board has no released processor".into());
        }
        if self
            .processors
            .iter()
            .any(|p| (p.model == Model::CortexM4) != p.mprofile.is_some())
        {
            return Err("M-profile processor/controller configuration must match".into());
        }
        if self
            .processors
            .iter()
            .any(|p| (p.model == Model::CortexR5F) != p.rprofile.is_some())
        {
            return Err("R-profile processor/backend configuration must match".into());
        }
        let mut spans = vec![];
        for r in &self.ram {
            let end = r
                .base
                .checked_add(r.size as u64)
                .ok_or("RAM range overflow")?;
            if r.size == 0 || r.base % 0x1000 != 0 || r.size % 0x1000 != 0 {
                return Err("invalid physical RAM geometry".into());
            }
            spans.push((r.base, end));
        }
        spans.sort();
        if spans.windows(2).any(|p| p[0].1 > p[1].0) {
            return Err("overlapping physical RAM".into());
        }
        super::memory::validate_mappings(&self.dma_mappings, &spans)?;
        for a in &self.mappings {
            self.validate_memory_map(a, &spans)?;
        }
        for (i, p) in self.peripherals.iter().enumerate() {
            if p.core >= self.processors.len() || p.size == 0 {
                return Err("invalid peripheral mapping".into());
            }
            let end = p
                .address
                .checked_add(p.size as u64)
                .ok_or("peripheral map overflow")?;
            p.physical
                .checked_add(p.size as u64)
                .ok_or("peripheral physical range overflow")?;
            if self
                .mappings
                .iter()
                .filter(|m| m.core == p.core)
                .any(|m| p.address < m.address + m.size as u64 && end > m.address)
            {
                return Err("peripheral mapping overlaps RAM".into());
            }
            if self.peripherals[..i].iter().any(|q| {
                q.core == p.core && q.address < end && p.address < q.address + q.size as u64
            }) {
                return Err("overlapping peripheral mappings".into());
            }
            if self.dma_mappings.iter().any(|m| {
                p.physical < m.address + m.size as u64 && m.address < p.physical + p.size as u64
            }) {
                return Err("peripheral physical range overlaps RAM".into());
            }
        }
        for (i, a) in self.mappings.iter().enumerate() {
            if self.mappings[..i].iter().any(|b| {
                a.core == b.core
                    && a.address < b.address + b.size as u64
                    && b.address < a.address + a.size as u64
            }) {
                return Err("overlapping processor RAM mappings".into());
            }
        }
        self.validate_decoders(&spans)?;
        Ok(())
    }
    /// Native engines map peripheral pages; byte windows still dispatch through
    /// the bus, which refuses every hole outside a declared device window.
    fn peripheral_pages(&self) -> Result<Vec<Peripheral>, String> {
        let mut pages = Vec::<Peripheral>::new();
        for p in &self.peripherals {
            let grain = self.processors[p.core].mapping_granule();
            let address = p.address & !(grain - 1);
            let end = (p.address + p.size as u64)
                .checked_add(grain - 1)
                .ok_or("MMIO page envelope overflow")?
                & !(grain - 1);
            let physical = p
                .physical
                .checked_sub(p.address - address)
                .ok_or("MMIO page identity underflow")?;
            if self
                .mappings
                .iter()
                .any(|m| m.core == p.core && m.address < end && address < m.address + m.size as u64)
            {
                return Err("MMIO page envelope overlaps RAM".into());
            }
            pages.push(Peripheral {
                core: p.core,
                address,
                physical,
                size: (end - address) as usize,
            });
        }
        pages.sort_by_key(|p| (p.core, p.address));
        let mut merged = Vec::<Peripheral>::new();
        for p in pages {
            if let Some(last) = merged.last_mut()
                && last.core == p.core
                && p.address <= last.address + last.size as u64
            {
                let offset = p.address - last.address;
                if last.physical.checked_add(offset) == Some(p.physical) {
                    last.size = last.size.max(
                        (offset as usize)
                            .checked_add(p.size)
                            .ok_or("MMIO page size overflow")?,
                    );
                } else if p.address < last.address + last.size as u64 {
                    return Err("MMIO page has conflicting physical identities".into());
                } else {
                    merged.push(p);
                }
            } else {
                merged.push(p);
            }
        }
        Ok(merged)
    }
    pub fn build(
        self,
        options: Options,
        bus: Bus,
        interrupts: Option<InterruptMap>,
    ) -> Result<Board, String> {
        self.validate()?;
        let peripheral_pages = self.peripheral_pages()?;
        // Validate maps before creating a process-wide native VM.
        let ram = super::memory::Memory::new(
            self.ram
                .iter()
                .map(|r| Ok((r.base, Backing::aligned_zeros(r.size, 0x4000)?)))
                .collect::<Result<Vec<_>, String>>()?,
            self.dma_mappings.clone(),
        )?;
        let unicorn_processors = self
            .processors
            .iter()
            .filter(|processor| {
                matches!(options.engine, Engine::Unicorn) || processor.model != Model::ArmV8A
            })
            .count();
        let translation_cache_size_bytes = match options.translation_cache_size_bytes {
            Some(bytes) => bytes,
            None => translation_cache_size_bytes(unicorn_processors)?,
        };
        let engines = Engines::new(&self, &options)?;
        let memory = fujimod_emulator_cpu_unicorn::memory::Memory::default();
        let cpus = self
            .processors
            .iter()
            .map(|p| engines.cpu(p, &options, translation_cache_size_bytes, &memory))
            .collect::<Result<Vec<_>, _>>()?;
        for (core, cpu) in cpus.iter().enumerate() {
            for a in self.mappings.iter().filter(|a| a.core == core) {
                let (base, b) = ram
                    .regions()
                    .iter()
                    .find(|(base, b)| {
                        a.physical >= *base && a.physical + a.size as u64 <= *base + b.len() as u64
                    })
                    .ok_or("RAM mapping backing disappeared")?;
                let pointer = unsafe { b.as_ptr().add((a.physical - base) as usize) };
                unsafe {
                    cpu.map_physical(
                        a.address,
                        a.physical,
                        a.size,
                        a.permissions,
                        NonNull::new(pointer).ok_or("null RAM mapping")?,
                    )
                    .map_err(|error| {
                        format!("CPU{core} RAM {:#x}+{:#x}: {error}", a.address, a.size)
                    })?;
                }
            }
            for &(r, value) in &self.processors[core].registers {
                set(&**cpu, r, value)
                    .map_err(|error| format!("CPU{core} initialize {r:?}={value:#x}: {error}"))?;
            }
        }
        let released = self.processors.iter().map(|p| p.released).collect();
        let mut board = Board::new(
            cpus,
            bus,
            self.instructions_per_tick,
            interrupts,
            released,
            ram,
        )?;
        board.translation_cache_size_bytes = translation_cache_size_bytes;
        board.state.borrow_mut().decoders = super::decoder::Decoders::new(self.decoders);
        board.mappings = self.mappings.clone();
        for (core, p) in self.processors.iter().enumerate() {
            if let Some(config) = p.mprofile {
                let views = self
                    .mappings
                    .iter()
                    .filter(|m| m.core == core)
                    .map(|m| super::mprofile::MemoryView {
                        address: m.address,
                        physical: m.physical,
                        size: m.size,
                    })
                    .collect();
                board.configure_mprofile(core, config, views)?;
            }
        }
        for p in peripheral_pages {
            board
                .map_mmio(p.core, p.address, p.size, p.physical)
                .map_err(|error| {
                    format!("CPU{} MMIO {:#x}+{:#x}: {error}", p.core, p.address, p.size)
                })?;
        }
        board.retained_engines = Some(engines);
        Ok(board)
    }
}

#[cfg(test)]
mod translation_cache_tests {
    use super::browser_translation_cache_size_bytes;

    #[test]
    fn browser_cache_is_fixed_and_refuses_a_full_heap() {
        let mib: u64 = 1 << 20;
        assert_eq!(
            browser_translation_cache_size_bytes(1024 * mib, 2).unwrap(),
            32 << 20
        );
        assert_eq!(
            browser_translation_cache_size_bytes((64 + 32 + 32) * mib, 2).unwrap(),
            32 << 20
        );
        assert!(
            browser_translation_cache_size_bytes((64 + 63) * mib, 2)
                .unwrap_err()
                .contains("32 MiB")
        );
        assert!(browser_translation_cache_size_bytes((64 + 64) * mib, 0).is_err());
    }
}

#[cfg(test)]
mod peripheral_page_tests {
    use super::*;
    fn graph() -> Graph {
        Graph {
            processors: vec![Processor {
                model: Model::ArmV8A,
                registers: vec![],
                released: true,
                mprofile: None,
                rprofile: None,
            }],
            ram: vec![Ram {
                base: 0x10000,
                size: 0x4000,
            }],
            mappings: vec![MemoryMap {
                core: 0,
                address: 0x10000,
                physical: 0x10000,
                size: 0x4000,
                permissions: 7,
            }],
            dma_mappings: vec![],
            decoders: vec![],
            instructions_per_tick: Ratio::new(1, 1).unwrap(),
            peripherals: vec![
                Peripheral {
                    core: 0,
                    address: 0x20004,
                    physical: 0x20004,
                    size: 4,
                },
                Peripheral {
                    core: 0,
                    address: 0x20100,
                    physical: 0x20100,
                    size: 4,
                },
            ],
        }
    }
    #[test]
    fn sparse_registers_share_a_native_page() {
        let pages = graph().peripheral_pages().unwrap();
        assert_eq!(pages.len(), 1);
        assert_eq!(
            (pages[0].address, pages[0].physical, pages[0].size),
            (0x20000, 0x20000, 0x4000)
        );
    }
    #[test]
    fn conflicting_page_identities_and_ram_overlap_refuse() {
        let mut graph = graph();
        graph.peripherals[1].physical = 0x30100;
        assert!(
            graph
                .peripheral_pages()
                .err()
                .unwrap()
                .contains("conflicting")
        );
        graph.peripherals[1].address = 0x10004;
        graph.peripherals[1].physical = 0x10004;
        assert!(
            graph
                .peripheral_pages()
                .err()
                .unwrap()
                .contains("overlaps RAM")
        );
    }
    #[test]
    fn adjacent_pages_can_have_different_physical_identities() {
        let mut graph = graph();
        graph.peripherals[1].address = 0x24000;
        graph.peripherals[1].physical = 0x30000;
        assert_eq!(graph.peripheral_pages().unwrap().len(), 2);
    }
}
