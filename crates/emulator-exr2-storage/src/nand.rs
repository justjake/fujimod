//! NAND at FFF52000. Register-selected PIO/DMA and completion IRQ80 only.
//! Data/OOB placement, clean ECC and virtual latency are declared inputs;
//! opaque update bytes never select commands or authorize completion.
use fujimod_emulator_devices::{Cx, Device};
use fujimod_emulator_devices::parts::nand::Nand;
use serde::{Deserialize, Serialize};

const BASE: u64 = 0xfff52000;
const OFFSETS: &[usize] = &[
    0, 4, 8, 0x10, 0x18, 0x1c, 0x20, 0x24, 0x28, 0x30, 0x40, 0x44, 0x48, 0x4c, 0x50, 0x54, 0x58,
    0x5c, 0x60, 0x70, 0x74, 0x78, 0x7c, 0x98, 0xc8, 0xe8, 0x804, 0x80c, 0x810, 0x820, 0x824, 0x828,
    0x844, 0x884, 0x88c, 0x890, 0x8a0, 0x8a4, 0x8a8, 0x8c4,
];
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub erase_ticks: u64,
}
#[derive(Clone, Copy, Serialize, Deserialize)]
enum Operation {
    Read {
        count: usize,
        destination: Option<u64>,
        staged: Option<u64>,
    },
    Program {
        row: u32,
        count: usize,
        source: Option<u64>,
        cached: bool,
        spare: [u8; 16],
    },
    Cache {
        sector: usize,
    },
    Erase {
        row: u32,
    },
}
#[derive(Clone, Copy, Serialize, Deserialize)]
struct Pending {
    due: u64,
    core: usize,
    operation: Operation,
}
#[derive(Serialize, Deserialize)]
struct Cache {
    row: u32,
    sector: usize,
    metadata: [u8; 16],
    #[serde(with = "fujimod_emulator_devices::checkpoint::array")]
    data: [u8; 2048],
}
pub struct NandController {
    options: Options,
    medium: Nand,
    regs: Box<[u32]>,
    aperture: [u8; 512],
    scratch: Box<[u8]>,
    pending: Option<Pending>,
    staged: Option<u64>,
    cache: Option<Cache>,
}
impl NandController {
    pub fn new(options: Options, medium: Nand) -> Result<Self, String> {
        if options.erase_ticks == 0 {
            return Err("NAND erase delay must be positive".into());
        }
        let mut regs = vec![0; 0x900 / 4].into_boxed_slice();
        for offset in [0x40, 0x44, 0x48, 0x4c] {
            regs[offset / 4] = u32::MAX;
        }
        Ok(Self {
            options,
            medium,
            regs,
            aperture: [255; 512],
            scratch: vec![0; 0x20000].into_boxed_slice(),
            pending: None,
            staged: None,
            cache: None,
        })
    }
    pub fn medium(&self) -> &Nand {
        &self.medium
    }
    pub fn busy(&self) -> bool {
        self.pending.is_some() || self.cache.is_some()
    }
    pub fn retain(&mut self, medium: Nand) -> Result<(), String> {
        if self.busy() || !self.medium.same_base(&medium) {
            return Err("retained NAND identity/geometry differs or controller is busy".into());
        }
        self.medium = medium;
        Ok(())
    }
    fn get(&self, o: usize) -> u32 {
        self.regs[o / 4]
    }
    fn set(&mut self, o: usize, v: u32) {
        self.regs[o / 4] = v;
    }
    fn metadata(&self) -> [u8; 16] {
        let mut bytes = [0; 16];
        for (i, o) in [0x40, 0x44, 0x48, 0x4c].into_iter().enumerate() {
            bytes[i * 4..i * 4 + 4].copy_from_slice(&self.get(o).to_be_bytes());
        }
        bytes
    }
    fn begin(&mut self, cx: &mut Cx<'_>, operation: Operation, delay: u64) -> Result<(), String> {
        let due = cx
            .now()
            .checked_add(delay)
            .ok_or("NAND deadline overflow")?;
        self.pending = Some(Pending {
            due,
            core: cx.cpu,
            operation,
        });
        self.set(4, self.get(4) | 4);
        self.set(0x54, self.get(0x54) & !1);
        match operation {
            Operation::Read {
                destination: Some(_),
                ..
            } => self.set(0x890, 1),
            Operation::Program {
                source: Some(_), ..
            } => self.set(0x810, 1),
            _ => (),
        }
        Ok(())
    }
    fn read_job(
        &mut self,
        cx: &mut Cx<'_>,
        offset: u64,
        count: usize,
        spare: bool,
        destination: Option<u64>,
        staged: bool,
    ) -> Result<(), String> {
        if count > self.scratch.len() {
            return Err("NAND read exceeds transfer bound".into());
        }
        if spare {
            self.medium.read_spare(
                (offset / self.medium.options().page_bytes as u64) as u32,
                &mut self.scratch[..count],
            )?;
        } else {
            self.medium.read(offset, &mut self.scratch[..count])?;
        }
        self.begin(
            cx,
            Operation::Read {
                count,
                destination,
                staged: staged.then_some(offset),
            },
            2.max(count as u64 / 512),
        )
    }
    fn dma_rx(&self, count: usize, rom: bool, verify: bool) -> Result<u64, String> {
        let config = self.get(0x8a0);
        let app = config & 0x80000000 != 0;
        if self.get(0x884) != 0x10001
            || self.get(0x8a8) < count as u32
            || config & !0x90700000 != 0x1000
            || config & 0x10000000 != 0
            || app && !matches!((config >> 20) & 7, 3 | 4)
            || verify && !app
            || rom && (count != 2048 || config != 0x401000)
            || !rom && self.get(0x8a8) != 0x100000
        {
            return Err("unsupported NAND receive DMA setup or interrupt mode".into());
        }
        Ok(self.get(0x8a4) as u64)
    }
    fn program(
        &mut self,
        cx: &mut Cx<'_>,
        row: u32,
        count: usize,
        cached: bool,
    ) -> Result<(), String> {
        let page = self.medium.options().page_bytes;
        if count == 0
            || count > self.scratch.len()
            || count % page != 0
            || (row as u64) * page as u64 + count as u64 > self.medium.options().bytes
        {
            return Err("NAND program extent exceeds medium".into());
        }
        let source = if cached {
            None
        } else {
            let config = self.get(0x820);
            let source = self.get(0x824) as u64;
            if !matches!(config, 0x80301000 | 0x80401000)
                || self.get(0x804) != 0x10001
                || self.get(0x828) != 0x100000
                || source % if config == 0x80401000 { 64 } else { 32 } != 0
            {
                return Err("unsupported NAND transmit DMA setup".into());
            }
            cx.ram_read(source, &mut self.scratch[..count])?;
            Some(source)
        };
        self.begin(
            cx,
            Operation::Program {
                row,
                count,
                source,
                cached,
                spare: self.metadata(),
            },
            2.max(count as u64 / 512),
        )
    }
    fn pio_program(&mut self, cx: &mut Cx<'_>, command: u32) -> Result<(), String> {
        let column = self.get(0x20);
        let row = self.get(0x24);
        let sector = (column / 0x210) as usize;
        let setups = [
            (0x882062, 0x80000001),
            (0x882004, 1),
            (0x882004, 1),
            (0x802064, 0x00107001),
        ];
        if sector > 3
            || column % 0x210 != 0
            || setups[sector] != (command, self.get(0x1c))
            || self.get(0x5c) != 0x1010
            || self.get(0x18) != 0x8093
            || row as u64 >= self.medium.options().bytes / 2048
        {
            return Err("unsupported NAND PIO page sequence".into());
        }
        let metadata = self.metadata();
        if sector == 0 {
            if self.cache.is_some() {
                return Err("unconfirmed NAND PIO page".into());
            }
            self.cache = Some(Cache {
                row,
                sector: 0,
                metadata,
                data: [0; 2048],
            });
        }
        let cache = self
            .cache
            .as_mut()
            .ok_or("NAND PIO cache has no initial sector")?;
        if cache.row != row || cache.sector != sector || cache.metadata != metadata {
            return Err("NAND PIO row, sector or metadata changed".into());
        }
        cache.data[sector * 512..sector * 512 + 512].copy_from_slice(&self.aperture);
        if sector == 3 {
            self.scratch[..2048].copy_from_slice(&cache.data);
            self.program(cx, row, 2048, true)
        } else {
            self.begin(cx, Operation::Cache { sector }, 2)
        }
    }
    fn start(&mut self, cx: &mut Cx<'_>, command: u32) -> Result<(), String> {
        if self.pending.is_some() {
            return Err("NAND command while busy".into());
        }
        let small = self.get(0) == 4;
        if !matches!(self.get(0), 1 | 4)
            || self.medium.options().page_bytes != if small { 512 } else { 2048 }
        {
            return Err("NAND register mode differs from attached medium geometry".into());
        }
        if !small && matches!(command, 0x882062 | 0x882004 | 0x802064) {
            return self.pio_program(cx, command);
        }
        if self.cache.is_some() {
            return Err("NAND command interrupts unconfirmed PIO page".into());
        }
        if matches!(command, 0x800007 | 0x802007) {
            if self.get(0x1c) != 0xff007000 {
                return Err("unsupported NAND reset setup".into());
            }
            self.staged = None;
            return self.begin(
                cx,
                Operation::Read {
                    count: 0,
                    destination: None,
                    staged: None,
                },
                2,
            );
        }
        if matches!(command, 0x800006 | 0x800046 | 0x802046) {
            let row = if small {
                let packed = self.get(0x20);
                let high = self.get(0x24);
                if packed & 0xff0000ff != 0 || high > 3 {
                    return Err("unsupported small-page NAND erase row".into());
                }
                high << 16 | packed >> 8
            } else {
                if self.get(0x20) != 0 {
                    return Err("unsupported large-page NAND erase column".into());
                }
                self.get(0x24)
            };
            let block_pages = if small { 32 } else { 64 };
            if self.get(0x1c) != 0x60d07000
                || self.get(0x18) != 0x8093
                || self.medium.options().pages_per_block != block_pages
                || row % block_pages != 0
                || row as u64
                    >= self.medium.options().bytes / self.medium.options().page_bytes as u64
            {
                return Err("unsupported NAND erase setup".into());
            }
            return self.begin(cx, Operation::Erase { row }, self.options.erase_ticks);
        }
        if command == if small { 0x802342 } else { 0x802362 } {
            let setup = self.get(0x1c);
            let mut units = (setup & 255) as usize;
            if !small && units == 0 {
                units = 256;
            }
            let row = if small {
                self.get(0x60)
            } else {
                self.get(0x24)
            };
            if setup & !255 != 0x80107000
                || self.get(0x5c) != 0x1010
                || self.get(0x18) != 0x8093
                || small
                    && (units == 0
                        || units > 32
                        || (row % 32) as usize + units > 32
                        || self.get(0x20) != ((row << 8) & 0xffff00)
                        || self.get(0x24) != (row >> 16) & 255)
                || !small && (units % 4 != 0 || self.get(0x20) != 0)
            {
                return Err("unsupported NAND program setup".into());
            }
            return self.program(cx, row, units * 512, false);
        }
        if small {
            return self.small_read(cx, command);
        }
        let row = self.get(0x24);
        let offset = row as u64 * 2048;
        let column = self.get(0x20);
        let setup = self.get(0x1c);
        match command {
            0x800001 | 0x802001 | 0x802041 if setup == 0x300000 && column == 0x200 => {
                self.read_job(cx, offset, 512, true, None, false)
            }
            0x800033 | 0x802033 | 0x8020f3
                if matches!(setup, 0x300000 | 0x300001) && column == 0 =>
            {
                self.read_job(cx, offset, 512, false, None, false)
            }
            0x8013b3 | 0x8033b3 | 0x8023c3 | 0x8023f3
                if setup & !0x1ff == 0x300000 && column == 0 =>
            {
                let mut units = setup & 0x1ff;
                if units == 0 && self.get(0x8a0) & 0x80000000 != 0 {
                    units = 256;
                }
                if units == 0 || units > 256 {
                    return Err("NAND read count outside burst bound".into());
                }
                let count = units as usize * 512;
                let destination = self.dma_rx(
                    count,
                    matches!(command, 0x8013b3 | 0x8033b3),
                    command == 0x8023c3,
                )?;
                self.read_job(cx, offset, count, false, Some(destination), false)
            }
            _ => Err(format!("unsupported NAND command {command:#x} or setup")),
        }
    }
    fn small_read(&mut self, cx: &mut Cx<'_>, command: u32) -> Result<(), String> {
        if matches!(command, 0x8027f3 | 0x8027c3) {
            let row = self.get(0x60);
            let count = self.get(0x1c) as usize * 512;
            if !(512..=32 * 512).contains(&count)
                || self.get(0x20) != ((row << 8) & 0xffff00)
                || self.get(0x24) != (row >> 16) & 255
                || self.get(0x884) != 0x10001
                || !matches!(
                    self.get(0x8a0),
                    0x301000 | 0x401000 | 0x80301000 | 0x80401000
                )
                || self.get(0x8a8) != 0x100000
                || command == 0x8027c3 && self.get(0x8a0) & 0x80000000 == 0
            {
                return Err("unsupported small-page NAND burst setup".into());
            }
            self.staged = None;
            return self.read_job(
                cx,
                row as u64 * 512,
                count,
                false,
                Some(self.get(0x8a4) as u64),
                false,
            );
        }
        if matches!(command, 0x800980 | 0x802980) {
            if self.staged.is_none()
                || self.get(0x58) != 512
                || self.get(0x884) != 0x10001
                || self.get(0x8a0) != 0x401000
                || self.get(0x8a8) < 512
            {
                return Err("small-page NAND DMA requires staged PIO and RX setup".into());
            }
            self.staged = None;
            self.scratch[..512].copy_from_slice(&self.aperture);
            return self.begin(
                cx,
                Operation::Read {
                    count: 512,
                    destination: Some(self.get(0x8a4) as u64),
                    staged: None,
                },
                2,
            );
        }
        let spare = matches!(command, 0x800041 | 0x802041);
        let pio = matches!(command, 0x800073 | 0x802073 | 0x8020f3 | 0x8020c3);
        let packed = self.get(0x20);
        let high = self.get(0x24);
        let expected = if spare {
            0x50000000
        } else if matches!(command, 0x8020f3 | 0x8020c3) {
            1
        } else {
            0
        };
        if (!spare && !pio) || high > 3 || packed & 0xff0000ff != 0 || self.get(0x1c) != expected {
            return Err("unsupported small-page NAND read setup".into());
        }
        self.staged = None;
        self.read_job(
            cx,
            ((high as u64) << 16 | (packed as u64 >> 8)) * 512,
            512,
            spare,
            None,
            pio,
        )
    }
    fn complete(&mut self, cx: &mut Cx<'_>, pending: Pending) -> Result<(), String> {
        match pending.operation {
            Operation::Read {
                count,
                destination,
                staged,
            } => {
                if let Some(address) = destination {
                    cx.ram_write(address, &self.scratch[..count])?;
                    self.set(
                        0x8a4,
                        (address + count as u64)
                            .try_into()
                            .map_err(|_| "NAND DMA address overflow")?,
                    );
                    self.set(0x8a8, self.get(0x8a8) - count as u32);
                    self.set(0x890, 0);
                } else if count != 0 {
                    self.aperture.copy_from_slice(&self.scratch[..512]);
                }
                self.staged = staged;
                self.set(0x28, 0);
            }
            Operation::Cache { sector } => {
                self.cache.as_mut().ok_or("missing NAND PIO cache")?.sector = sector + 1;
                self.set(0x28, 0);
            }
            Operation::Erase { row } => {
                self.medium.erase(row)?;
                self.set(0x28, 0);
            }
            Operation::Program {
                row,
                count,
                source,
                cached,
                spare,
            } => {
                let page = self.medium.options().page_bytes;
                let mut before = [0; 2048];
                let mut oob = [0; 512];
                let mut failed = false;
                for n in 0..count / page {
                    let row = row + n as u32;
                    self.medium
                        .read(row as u64 * page as u64, &mut before[..page])?;
                    let requested = &self.scratch[n * page..(n + 1) * page];
                    for (old, &new) in before[..page].iter_mut().zip(requested) {
                        failed |= new & !*old != 0;
                        *old &= new;
                    }
                    self.medium.read_spare(row, &mut oob[..16])?;
                    for (old, new) in oob[..16].iter_mut().zip(spare) {
                        failed |= new & !*old != 0;
                        *old &= new;
                    }
                    self.medium.program(row, &before[..page], &oob[..16])?;
                }
                if let Some(address) = source {
                    self.set(
                        0x824,
                        (address + count as u64)
                            .try_into()
                            .map_err(|_| "NAND DMA address overflow")?,
                    );
                    self.set(0x828, self.get(0x828) - count as u32);
                    self.set(0x810, 0);
                }
                if cached {
                    self.cache = None;
                }
                self.set(0x28, if failed { 1 << 24 } else { 0 });
            }
        }
        self.pending = None;
        self.set(4, self.get(4) & !4);
        self.set(0x54, self.get(0x54) | 1);
        self.set(0xc8, 0);
        if self.get(0x54) & 8 != 0 {
            cx.raise_irq(pending.core, 80)?;
        }
        Ok(())
    }
}
impl Device for NandController {
    fn nand_store(&self) -> Option<&(dyn fujimod_emulator_devices::capability::NandStore + 'static)> {
        Some(self)
    }
    fn nand_store_mut(&mut self) -> Option<&mut (dyn fujimod_emulator_devices::capability::NandStore + 'static)> {
        Some(self)
    }
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        Ok(fujimod_emulator_devices::checkpoint::Checkpoint::encode(
            self.name(),
            &(
                &self.regs,
                self.aperture.as_slice(),
                &self.scratch,
                &self.pending,
                self.staged,
                &self.cache,
            ),
        )?
        .with_children(vec![self.medium.checkpoint()?]))
    }
    fn validate_checkpoint(&self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        let state: (
            Box<[u32]>,
            Vec<u8>,
            Box<[u8]>,
            Option<Pending>,
            Option<u64>,
            Option<Cache>,
        ) = saved.decode(self.name())?;
        if state.1.len() != 512 || state.0.len() != self.regs.len() {
            return Err("NAND snapshot register/aperture extent differs".into());
        }
        if saved.children().len() != 1 {
            return Err("NAND snapshot medium missing".into());
        }
        let medium = Nand::from_checkpoint(&saved.children()[0])?;
        if medium.options() != self.medium.options() {
            return Err("NAND snapshot geometry differs".into());
        }
        Ok(())
    }
    fn restore_checkpoint(&mut self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        let (regs, aperture, scratch, pending, staged, cache): (
            Box<[u32]>,
            Vec<u8>,
            Box<[u8]>,
            Option<Pending>,
            Option<u64>,
            Option<Cache>,
        ) = saved.decode(self.name())?;
        self.medium = Nand::from_checkpoint(&saved.children()[0])?;
        self.regs = regs;
        self.aperture.copy_from_slice(&aperture);
        self.scratch = scratch;
        self.pending = pending;
        self.staged = staged;
        self.cache = cache;
        Ok(())
    }

    fn fingerprint(&self) -> Result<serde_json::Value, String> {
        Ok(
            serde_json::json!({"medium":self.medium.fingerprint(),"registers":&*self.regs,
            "aperture":fujimod_emulator_devices::fingerprint::bytes(&self.aperture),"scratch":fujimod_emulator_devices::fingerprint::bytes(&self.scratch),
            "pending":self.pending,"staged":self.staged,"cache":self.cache.as_ref().map(|c|
                serde_json::json!({"row":c.row,"sector":c.sector,"metadata":c.metadata,"data":fujimod_emulator_devices::fingerprint::bytes(&c.data)}))}),
        )
    }
    fn name(&self) -> &'static str {
        "nand-controller"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let Some(o) = address
            .checked_sub(BASE)
            .and_then(|o| usize::try_from(o).ok())
        else {
            cx.unmodeled();
            return 0;
        };
        if o >= 0x400 && o + size as usize <= 0x600 && matches!(size, 1 | 2 | 4) {
            let mut bytes = [0; 8];
            bytes[..size as usize]
                .copy_from_slice(&self.aperture[o - 0x400..o - 0x400 + size as usize]);
            return u64::from_le_bytes(bytes);
        }
        if size != 4 || !OFFSETS.contains(&o) {
            cx.unmodeled();
            0
        } else {
            self.get(o) as u64
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Some(o) = address
            .checked_sub(BASE)
            .and_then(|o| usize::try_from(o).ok())
        else {
            cx.unmodeled();
            return;
        };
        if o >= 0x400 && o + size as usize <= 0x600 && matches!(size, 1 | 2 | 4) {
            self.aperture[o - 0x400..o - 0x400 + size as usize]
                .copy_from_slice(&value.to_le_bytes()[..size as usize]);
            return;
        }
        if size != 4 || !OFFSETS.contains(&o) {
            cx.unmodeled();
            return;
        }
        let value = value as u32;
        match o {
            0x810 | 0x890 => cx.fail("NAND DMA activity is read-only"),
            4 => self.set(4, self.get(4) & !(value & !4)),
            0x54 => {
                if value & !0xe0009 != 0 {
                    cx.fail("unsupported NAND completion controls")
                } else {
                    self.set(o, (self.get(o) & 1 & !value) | (value & 0xe0008));
                }
            }
            0x80c | 0x88c => {
                if value & !1 != 0 {
                    cx.fail("unsupported NAND DMA acknowledge")
                } else {
                    self.set(o, self.get(o) & !value)
                }
            }
            0x844 | 0x8c4 if value != 0 => cx.fail("unsupported NAND DMA status control"),
            0 if value != 0
                && value
                    != if self.medium.options().page_bytes == 512 {
                        4
                    } else {
                        1
                    } =>
            {
                cx.fail("NAND geometry differs from attached medium")
            }
            _ => {
                self.set(o, value);
                if o == 8 {
                    if value == 0 {
                        self.pending = None;
                        self.cache = None;
                        self.staged = None;
                        self.set(4, self.get(4) & !4);
                        self.set(0x810, 0);
                        self.set(0x890, 0);
                    } else if value & 0x800000 != 0 {
                        if let Err(error) = self.start(cx, value) {
                            cx.fail(error);
                        }
                    }
                }
            }
        }
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.pending.map(|p| p.due)
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if let Some(p) = self.pending {
            if p.due <= cx.now() {
                if let Err(error) = self.complete(cx, p) {
                    cx.fail(error);
                }
            }
        }
    }
    fn pollable(&self, address: u64, size: u8) -> bool {
        size == 4 && matches!(address, 0xfff52004 | 0xfff52054 | 0xfff52810 | 0xfff52890)
    }
    fn save(&self) -> serde_json::Value {
        serde_json::json!({"registers":&*self.regs,"busy":self.busy(),"deadline":self.pending.map(|p|p.due),"cached_sector":self.cache.as_ref().map(|c|c.sector),"page_bytes":self.medium.options().page_bytes})
    }
}
#[cfg(test)]
mod tests;

impl fujimod_emulator_devices::capability::NandStore for NandController {
    fn medium(&self) -> &fujimod_emulator_devices::parts::nand::Nand {
        NandController::medium(self)
    }
    fn retain(&mut self, medium: fujimod_emulator_devices::parts::nand::Nand) -> std::result::Result<(), String> {
        NandController::retain(self, medium)
    }
}
