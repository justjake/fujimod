//! RAW conversion (FFF42000), sparse prefilter (FFF1A000), and statistics/gain
//! (FFF1C000). Masks follow the declared register fields. Sparse repair/gain are
//! declared identity approximations; no unselected conversion DMA is synthesized.
use fujimod_emulator_imaging::exr2_isp::dma::physical;
use fujimod_emulator_imaging::exr2_isp::{due, word};
use fujimod_emulator_devices::exr2::irq_routes::Irq;
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use fujimod_emulator_imaging::exr2_isp::options::raw::{Options, RegisterOptions};

#[derive(Serialize, Deserialize)]
enum Pending {
    Reset(u64),
    Work(u64),
}
impl Pending {
    fn deadline(&self) -> u64 {
        match self {
            Self::Reset(t) | Self::Work(t) => *t,
        }
    }
}

pub struct RawConversion {
    registers: Vec<u32>,
}
impl RawConversion {
    pub fn new(_: RegisterOptions, _: ()) -> Result<Self, String> {
        Ok(Self {
            registers: vec![0; 1024],
        })
    }
    fn allowed(o: usize) -> bool {
        matches!(o,0..=0x54|0x60..=0x78|0x80..=0x9c|0x104|0x10c|0x120..=0x144|0x184|0x18c|0x1a0..=0x1c4|0x1f4|0x1f8|0x800..=0x83c)
    }
}
impl Device for RawConversion {
    fujimod_emulator_devices::checkpoint_fields!(registers: Vec<u32>);
    fn fingerprint(&self) -> std::result::Result<serde_json::Value, String> {
        Ok(self.save())
    }

    fn name(&self) -> &'static str {
        "exr2-isp-raw-conversion"
    }
    fn read(&mut self, cx: &mut Cx<'_>, _: u64, _: u8) -> u64 {
        cx.fail("RAW conversion read/status is unmodeled");
        0
    }
    fn write(&mut self, cx: &mut Cx<'_>, a: u64, s: u8, v: u64) {
        let Some(o) = word(cx, a, s, 0xfff42000, 0x1000) else {
            return;
        };
        if !Self::allowed(o) || v > u32::MAX as u64 {
            cx.unmodeled();
            return;
        }
        if (o == 0 && ![0, 1, 0x10000].contains(&v))
            || (o == 4 && v & !0x303 != 0)
            || ([8, 12].contains(&o) && v & !0x1ff != 0)
            || (o == 0x1f8 && v & !0x10000000 != 0)
        {
            cx.fail("RAW conversion control is unsupported");
            return;
        }
        self.registers[o / 4] = v as u32;
    }
    fn save(&self) -> serde_json::Value {
        serde_json::json!(self.registers)
    }
    fn restore(&mut self, v: &serde_json::Value) -> Result<(), String> {
        let r: Vec<u32> = serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
        if r.len() != 1024 {
            return Err("RAW conversion bank size".into());
        }
        self.registers = r;
        Ok(())
    }
}

pub struct Prefilter {
    r: Vec<u32>,
    options: Options,
    irq: Irq,
    pending: Option<Pending>,
}
impl Prefilter {
    pub fn new(options: Options, irq: Irq) -> Result<Self, String> {
        options.validate()?;
        let mut r = vec![0; 0x230 / 4];
        r[0] = 0x10000;
        Ok(Self {
            r,
            options,
            irq,
            pending: None,
        })
    }
    fn valid(o: usize) -> bool {
        o <= 0x20 || (0x30..0x230).contains(&o)
    }
    fn store(&mut self, cx: &mut Cx<'_>, o: usize, v: u32) -> Result<(), String> {
        if o == 0x1c {
            if v & !0x1f != 0 {
                return Err("unknown prefilter ACK".into());
            }
            self.r[6] &= !v;
            return Ok(());
        }
        if self.pending.is_some() {
            return Err("prefilter reconfiguration during pending work".into());
        }
        if o == 0 {
            if v == 0 {
                self.pending = Some(Pending::Reset(due(cx.now(), self.options.reset_ticks)?));
            } else if v & 7 != 0 {
                if v & !0x10107 != 0 || ![1, 2, 3, 4].contains(&(v & 7)) || self.r[2] == 0 {
                    return Err("prefilter start exceeds identity repair law".into());
                }
                physical(self.r[1] as u64, 2)?;
                let count = self.r[4] as usize;
                if count == 0 || count >= 0x40000 {
                    return Err("prefilter correction count is unsupported".into());
                }
                physical(self.r[3] as u64, (count * 34).div_ceil(8))?;
                self.pending = Some(Pending::Work(due(cx.now(), self.options.completion_ticks)?));
            } else if !matches!(v, 0x10000 | 0x10100) {
                return Err("unknown prefilter mode".into());
            }
        } else if o == 0x18 {
            return Err("prefilter status is read-only".into());
        }
        self.r[o / 4] = v;
        Ok(())
    }
}
impl Device for Prefilter {
    fujimod_emulator_devices::checkpoint_fields!(r: Vec<u32>, pending: Option<Pending>);
    fn fingerprint(&self) -> std::result::Result<serde_json::Value, String> {
        Ok(self.save())
    }

    fn name(&self) -> &'static str {
        "exr2-isp-prefilter"
    }
    fn read(&mut self, cx: &mut Cx<'_>, a: u64, s: u8) -> u64 {
        let Some(o) = word(cx, a, s, 0xfff1a000, 0x230) else {
            return 0;
        };
        if !Self::valid(o) {
            cx.unmodeled();
            return 0;
        }
        self.r[o / 4] as u64
    }
    fn write(&mut self, cx: &mut Cx<'_>, a: u64, s: u8, v: u64) {
        let Some(o) = word(cx, a, s, 0xfff1a000, 0x230) else {
            return;
        };
        if !Self::valid(o) || v > u32::MAX as u64 {
            cx.unmodeled();
            return;
        }
        if let Err(e) = self.store(cx, o, v as u32) {
            cx.fail(e);
        }
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.pending.as_ref().map(Pending::deadline)
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if self.next_deadline(cx.now()).is_none_or(|d| cx.now() < d) {
            return;
        }
        match self.pending.take().unwrap() {
            Pending::Reset(_) => self.r[0] = 0x10000,
            Pending::Work(_) => {
                self.r[6] |= 1;
                self.r[0] &= !7;
                if let Err(e) = self.irq.raise(cx) {
                    cx.fail(e);
                }
            }
        }
    }
    fn event(&mut self, cx: &mut Cx<'_>, _: u64) {
        self.advance(cx)
    }
    fn save(&self) -> serde_json::Value {
        serde_json::json!((&self.r, &self.pending))
    }
    fn restore(&mut self, v: &serde_json::Value) -> Result<(), String> {
        let (r, p): (Vec<u32>, Option<Pending>) =
            serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
        if r.len() != self.r.len() {
            return Err("prefilter bank size".into());
        }
        self.r = r;
        self.pending = p;
        Ok(())
    }
}

pub struct RawStatistics {
    r: Vec<u32>,
    results: Vec<u32>,
    options: Options,
    irq: Irq,
    pending: Option<Pending>,
    row: Arc<[u8]>,
}
impl RawStatistics {
    pub fn new(options: Options, irq: Irq) -> Result<Self, String> {
        options.validate()?;
        let mut r = vec![0; 1024];
        r[0] = 1;
        Ok(Self {
            r,
            results: vec![0; 1024],
            options,
            irq,
            pending: None,
            row: vec![0; 16384].into(),
        })
    }
    fn statistics(&mut self, cx: &Cx<'_>) -> Result<(), String> {
        let r = &self.r;
        let (x0, y0) = ((r[6] & 8191) as usize, (r[6] >> 16 & 8191) as usize);
        let (x1, y1) = ((r[7] & 8191) as usize, (r[7] >> 16 & 8191) as usize);
        if r[5] > 8192 {
            return Err("RAW statistics row pitch is unsupported".into());
        }
        let pitch = r[5] as usize * 2;
        if x0 > x1 || x1 >= r[5] as usize || y0 > y1 || y1 >= 8192 || pitch > 16384 {
            return Err("RAW statistics ROI exceeds U16 raster".into());
        }
        physical(r[2] as u64, pitch * (y1 + 1))?;
        self.row = vec![0; pitch * (y1 + 1)].into();
        cx.ram_read(r[2] as u64, Arc::get_mut(&mut self.row).unwrap())?;
        self.results.fill(0);
        for cy in 0..6 {
            for col in 0..6 {
                let xa = x0 + col * (x1 - x0 + 1) / 6;
                let xb = (x0 + (col + 1) * (x1 - x0 + 1) / 6).max(xa + 1);
                let ya = y0 + cy * (y1 - y0 + 1) / 6;
                let yb = (y0 + (cy + 1) * (y1 - y0 + 1) / 6).max(ya + 1);
                let mut sum = 0u64;
                for y in ya..yb {
                    for x in xa..xb {
                        let i = y * pitch + x * 2;
                        sum += u16::from_le_bytes([self.row[i], self.row[i + 1]]).min(4095) as u64;
                    }
                }
                let n = (xb - xa) * (yb - ya);
                let count = n.min(0x3fff) as u32;
                let total = (sum / n as u64 * count as u64).min(0x1ffffff) as u32;
                let index = cy * 6 + col;
                for o in [
                    0x600, 0x690, 0x7b0, 0x840, 0x960, 0x9f0, 0xb10, 0xba0, 0xcc0, 0xd50, 0xe60,
                    0xef0,
                ] {
                    self.results[o / 4 + index] = total;
                }
                for o in [0x720, 0x8d0, 0xa80, 0xc30] {
                    self.results[o / 4 + index] = count;
                }
            }
        }
        Ok(())
    }
    fn store(&mut self, cx: &Cx<'_>, o: usize, v: u32) -> Result<(), String> {
        if o == 0xc8 {
            if v != 1 {
                return Err("unknown RAW statistics ACK".into());
            }
            self.r[o / 4] = 0;
            return Ok(());
        }
        if self.pending.is_some() {
            return Err("RAW statistics reconfiguration while busy".into());
        }
        if o == 0 {
            if v != 0 {
                return Err("unknown RAW statistics reset".into());
            }
            self.r[1] = 0;
            self.pending = Some(Pending::Reset(due(cx.now(), self.options.reset_ticks)?));
        } else if o == 4 && (2..=5).contains(&(v & 7)) {
            if self.r[0xcc / 4] != 1 {
                return Err("RAW statistics started before enable".into());
            }
            self.results.fill(0);
            if [2, 4].contains(&(v & 7)) {
                self.statistics(cx)?;
            }
            // Gain-only modes preserve previous result banks.
            else {
                self.results[0x600 / 4..].copy_from_slice(&self.r[0x600 / 4..]);
            }
            self.pending = Some(Pending::Work(due(cx.now(), self.options.completion_ticks)?));
        } else if o >= 0x600 {
            return Err("RAW statistics results are read-only".into());
        }
        self.r[o / 4] = v;
        Ok(())
    }
}
impl Device for RawStatistics {
    fujimod_emulator_devices::checkpoint_fields!(
        r: Vec<u32>,
        results: Vec<u32>,
        pending: Option<Pending>,
        ; blobs: [row]
    );
    fn fingerprint(&self) -> std::result::Result<serde_json::Value, String> {
        Ok(self.save())
    }

    fn name(&self) -> &'static str {
        "exr2-isp-raw-statistics"
    }
    fn read(&mut self, cx: &mut Cx<'_>, a: u64, s: u8) -> u64 {
        word(cx, a, s, 0xfff1c000, 0x1000).map_or(0, |o| self.r[o / 4] as u64)
    }
    fn write(&mut self, cx: &mut Cx<'_>, a: u64, s: u8, v: u64) {
        let Some(o) = word(cx, a, s, 0xfff1c000, 0x1000) else {
            return;
        };
        if v > u32::MAX as u64 {
            cx.unmodeled();
            return;
        }
        if let Err(e) = self.store(cx, o, v as u32) {
            cx.fail(e);
        }
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.pending.as_ref().map(Pending::deadline)
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if self.next_deadline(cx.now()).is_none_or(|d| cx.now() < d) {
            return;
        }
        match self.pending.take().unwrap() {
            Pending::Reset(_) => self.r[0] = 1,
            Pending::Work(_) => {
                self.r[0x600 / 4..].copy_from_slice(&self.results[0x600 / 4..]);
                self.r[0xc8 / 4] = 1;
                if let Err(e) = self.irq.raise(cx) {
                    cx.fail(e);
                }
            }
        }
    }
    fn event(&mut self, cx: &mut Cx<'_>, _: u64) {
        self.advance(cx)
    }
    fn save(&self) -> serde_json::Value {
        serde_json::json!((&self.r, &self.results, &self.pending))
    }
    fn restore(&mut self, v: &serde_json::Value) -> Result<(), String> {
        let (r, results, p): (Vec<u32>, Vec<u32>, Option<Pending>) =
            serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
        if r.len() != 1024 || results.len() != 1024 {
            return Err("RAW statistics bank size".into());
        }
        self.r = r;
        self.results = results;
        self.pending = p;
        Ok(())
    }
}
