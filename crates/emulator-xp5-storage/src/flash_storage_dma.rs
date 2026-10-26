//! Eight-channel flash-storage/general DMA.
use crate::flash_storage::{self, Shared};
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
pub const BASE: u64 = 0xff939000;
#[derive(Default, Serialize, Deserialize)]
struct State {
    channels: [[u32; 14]; 8],
    generation: [u64; 8],
    start: u32,
    busy: u32,
    enabled: u32,
    pending: u32,
    global: u32,
    mode: u32,
    #[serde(default)]
    ignored_disabled_requests: u64,
}
pub struct Dma {
    state: State,
    flash_storage: Shared,
}
impl Dma {
    pub fn new(flash_storage: Shared) -> Self {
        Self {
            state: State::default(),
            flash_storage,
        }
    }
    fn touches_flash(&self, n: usize) -> bool {
        let r = &self.state.channels[n];
        let src = r[2] as u64 | ((r[1] & 7) as u64) << 32;
        let dst = r[7] as u64 | ((r[6] & 7) as u64) << 32;
        (flash_storage::BASE..flash_storage::BASE + flash_storage::SIZE).contains(&src)
            || (flash_storage::BASE..flash_storage::BASE + flash_storage::SIZE).contains(&dst)
    }
    fn channel(o: u64) -> Option<(usize, usize)> {
        if (0x100..0x900).contains(&o) && o & 3 == 0 && o % 0x100 <= 0x34 {
            Some(((o / 0x100 - 1) as usize, (o % 0x100 / 4) as usize))
        } else {
            None
        }
    }
    fn irq(&self, cx: &mut Cx<'_>) {
        cx.set_irq(225, self.state.pending != 0);
    }
    fn copy(&mut self, cx: &mut Cx<'_>, n: usize) -> Result<(), String> {
        let r = &mut self.state.channels[n];
        if r[0] != 0 || r[12] != 2 || r[13] != 0 || r[11] != if n == 0 { 3 } else { 5 } {
            return Err("unsupported DMA channel mode".into());
        }
        let src = r[2] as u64 | ((r[1] & 7) as u64) << 32;
        let dst = r[7] as u64 | ((r[6] & 7) as u64) << 32;
        let (sr, sc, ss) = (r[4] as usize, r[5] as usize, r[3] as u64);
        let (dr, dc, ds) = (r[9] as usize, r[10] as usize, r[8] as u64);
        let total = sr.checked_mul(sc).ok_or("DMA length overflow")?;
        if total == 0
            || total > flash_storage::SIZE as usize
            || sr > 0x1ffff
            || dr > 0x1ffff
            || sc > 0xffff
            || dc > 0xffff
            || dr.checked_mul(dc) != Some(total)
        {
            return Err("invalid DMA row geometry".into());
        }
        let mut bytes = Vec::with_capacity(total);
        for row in 0..sc {
            let a = src
                .checked_add(row as u64 * ss)
                .ok_or("DMA source overflow")?;
            let b = if (flash_storage::BASE..flash_storage::BASE + flash_storage::SIZE).contains(&a)
            {
                self.flash_storage
                    .lock()
                    .unwrap()
                    .read_device(a - flash_storage::BASE, sr)?
            } else {
                cx.ram_bytes(a, sr)?
            };
            bytes.extend_from_slice(&b);
        }
        if (flash_storage::BASE..flash_storage::BASE + flash_storage::SIZE).contains(&dst) {
            if ds != dr as u64 {
                return Err("noncontiguous flash storage program DMA".into());
            }
            let mut s = self.flash_storage.lock().unwrap();
            if !s.session.program_mode {
                return Err("DMA to flash storage without controller program mode".into());
            }
            s.program(dst - flash_storage::BASE, &bytes)?;
        } else {
            for row in 0..dc {
                let a = dst
                    .checked_add(row as u64 * ds)
                    .ok_or("DMA destination overflow")?;
                cx.ram_write(a, &bytes[row * dr..(row + 1) * dr])?;
            }
        }
        let src = src + sc as u64 * ss;
        let dst = dst + dc as u64 * ds;
        r[1] = (src >> 32) as u32;
        r[2] = src as u32;
        r[6] = (dst >> 32) as u32;
        r[7] = dst as u32;
        Ok(())
    }
}
impl Device for Dma {
    fn name(&self) -> &'static str {
        "flash-storage-dma"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, n: u8) -> u64 {
        let o = address.wrapping_sub(BASE);
        if n != 4 {
            cx.unmodeled();
            return 0;
        }
        if let Some((c, r)) = Self::channel(o) {
            return self.state.channels[c][r] as u64;
        }
        match o {
            0 => 1,
            4 => self.state.start as u64,
            8 => self.state.busy as u64,
            0xc => self.state.mode as u64,
            0x10 => self.state.global as u64,
            0x14 => {
                if self.state.busy == 0 {
                    3
                } else {
                    0
                }
            }
            0x18 | 0x1c => self.state.pending as u64,
            0x20 => self.state.enabled as u64,
            _ => {
                cx.unmodeled();
                0
            }
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, n: u8, v: u64) {
        let o = address.wrapping_sub(BASE);
        if n != 4 {
            cx.unmodeled();
            return;
        }
        let v = v as u32;
        if let Some((c, r)) = Self::channel(o) {
            if self.state.busy & (1 << c) != 0 {
                cx.fail("reconfiguring active DMA");
                return;
            }
            self.state.channels[c][r] = v;
            return;
        }
        match o {
            0 if v == 0 => {
                for g in &mut self.state.generation {
                    *g += 1;
                }
                let generation = self.state.generation;
                self.state = State {
                    generation,
                    ..State::default()
                };
                self.irq(cx);
            }
            4 => {
                if v & !255 != 0 {
                    cx.fail("invalid DMA start bits");
                    return;
                }
                // 1486f18 disables a channel at+20 before RMW-clearing+4.
                // Its saved start word may replay a different core's stopped
                // bit. Disabled requests are inert, not malformed DMA submits.
                let disabled = v & !self.state.enabled;
                if disabled != 0 {
                    self.state.ignored_disabled_requests += 1;
                    if self.state.ignored_disabled_requests <= 8 {
                        cx.log(&format!("flash storage DMA ignored disabled requests{disabled:#x}, enabled{:#x}, vcpu{}", self.state.enabled, cx.cpu));
                    }
                }
                let v = v & self.state.enabled;
                for c in 0..8 {
                    let bit = 1 << c;
                    if v & bit != 0 && self.state.start & bit == 0 {
                        if self.state.global != 3 {
                            cx.fail(format!("DMA start during global drain: ch{c}, requests{v:#x}, start{:#x}, enabled{:#x}", self.state.start, self.state.enabled));
                            return;
                        }
                        self.state.busy |= bit;
                        if self.touches_flash(c) {
                            self.flash_storage
                                .lock()
                                .unwrap()
                                .session
                                .controller_complete = false;
                        }
                        self.state.generation[c] += 1;
                        // Timing is an explicit functional approximation, not measured silicon latency.
                        cx.schedule(cx.now() + 1000, (self.state.generation[c] << 3) | c as u64);
                    } else if v & bit == 0 && self.state.start & bit != 0 {
                        self.state.busy &= !bit;
                        self.state.generation[c] += 1;
                    }
                }
                self.state.start = v;
            }
            0xc if v <= 1 => self.state.mode = v,
            0x10 if v == 0 || v == 3 => self.state.global = v,
            0x1c => {
                self.state.pending &= !(v & (v >> 16) & 255);
                self.irq(cx);
            }
            0x20 => {
                let mask = (v >> 16) & 255;
                let old = self.state.enabled;
                self.state.enabled = (old & !mask) | (v & mask);
                let disabled = old & !self.state.enabled;
                self.state.start &= !disabled;
                self.state.busy &= !disabled;
                for c in 0..8 {
                    if disabled & (1 << c) != 0 {
                        self.state.generation[c] += 1;
                    }
                }
            }
            _ => cx.unmodeled(),
        }
    }
    fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
        let n = (token & 7) as usize;
        if self.state.busy & (1 << n) == 0 || self.state.generation[n] != token >> 3 {
            return;
        }
        let touches_flash = self.touches_flash(n);
        if let Err(e) = self.copy(cx, n) {
            cx.fail(e);
            return;
        }
        // The real aperture DMA also completes the serial-controller path
        // polled/ACKed by14721dc. Never notify on cancellation or copy failure.
        if touches_flash {
            self.flash_storage
                .lock()
                .unwrap()
                .session
                .controller_complete = true;
        }
        self.state.busy &= !(1 << n);
        self.state.pending |= 1 << n;
        self.irq(cx);
    }
    fn save(&self) -> Value {
        serde_json::to_value(&self.state).unwrap()
    }
    fn restore(&mut self, v: &Value) -> Result<(), String> {
        self.state = serde_json::from_value(v.clone())
            .map_err(|e| format!("invalid DMA checkpoint: {e}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::{bus::TestBus, recording::RecordingHost};
    fn wr(bus: &mut TestBus, h: &mut RecordingHost, o: u64, v: u64) {
        bus.write(&mut Cx::new(h, 0, 0, 0), 0xff939000 + o, 4, v)
            .unwrap();
    }
    fn rd(bus: &mut TestBus, h: &mut RecordingHost, o: u64) -> u64 {
        bus.read(&mut Cx::new(h, 0, 0, 0), 0xff939000 + o, 4)
            .unwrap()
    }
    #[test]
    fn asynchronous_rows_cancel_and_level_ack() {
        let storage = std::sync::Arc::new(std::sync::Mutex::new(
            flash_storage::Storage::new((0..32).collect()).unwrap(),
        ));
        let mut bus = TestBus::default();
        bus.register(0xff939000, 0x1000, Box::new(Dma::new(storage.clone())))
            .unwrap();
        let mut h = RecordingHost {
            ram: vec![0xff; 64],
            events: vec![],
            irq: false,
            ..RecordingHost::default()
        };
        // Three 4-byte source rows with gaps, repacked into two 6-byte destination rows.
        for (o, v) in [
            (0x108, flash_storage::BASE),
            (0x10c, 8),
            (0x110, 4),
            (0x114, 3),
            (0x11c, 16),
            (0x120, 10),
            (0x124, 6),
            (0x128, 2),
            (0x12c, 3),
            (0x130, 2),
            (0x10, 3),
            (0x20, 0x10001),
            (4, 1),
        ] {
            wr(&mut bus, &mut h, o, v);
        }
        assert_eq!(rd(&mut bus, &mut h, 8), 1);
        assert_eq!(h.ram[16], 0xff);
        let (d, at, t) = h.events[0];
        h.now = at;
        wr(&mut bus, &mut h, 4, 0); // Stop makes the first deadline stale.
        wr(&mut bus, &mut h, 4, 1);
        bus.event(&mut Cx::new(&mut h, 0, 0, 0), d, t).unwrap();
        assert_eq!(h.ram[16], 0xff);
        let saved = bus.save();
        let (d, at, t) = h.events[1];
        h.now = at;
        bus.event(&mut Cx::new(&mut h, 0, 0, 0), d, t).unwrap();
        assert_eq!(&h.ram[16..22], &[0, 1, 2, 3, 8, 9]);
        assert_eq!(&h.ram[26..32], &[10, 11, 16, 17, 18, 19]);
        assert_eq!(storage.lock().unwrap().session.reads.bytes, 12);
        assert_eq!(
            storage.lock().unwrap().session.reads.ranges,
            [(0, 4), (8, 12), (16, 20)]
        );
        assert_eq!(rd(&mut bus, &mut h, 8), 0);
        assert_eq!(rd(&mut bus, &mut h, 0x18), 1);
        assert!(h.irq);
        wr(&mut bus, &mut h, 0x1c, 1); // High-half mask is required.
        assert!(h.irq);
        wr(&mut bus, &mut h, 0x1c, 0x10001);
        assert!(!h.irq);
        bus.restore(&saved).unwrap();
        assert_eq!(rd(&mut bus, &mut h, 8), 1);
        bus.event(&mut Cx::new(&mut h, 0, 0, 0), d, t).unwrap();
        assert!(h.irq);
    }
    fn setup_two_channels(bus: &mut TestBus, h: &mut RecordingHost) {
        for c in 0..2 {
            let b = 0x100 + c * 0x100;
            for (o, v) in [
                (8, flash_storage::BASE + c * 4),
                (0xc, 4),
                (0x10, 4),
                (0x14, 1),
                (0x1c, 16 + c * 8),
                (0x20, 4),
                (0x24, 4),
                (0x28, 1),
                (0x2c, if c == 0 { 3 } else { 5 }),
                (0x30, 2),
            ] {
                wr(bus, h, b + o, v);
            }
        }
        wr(bus, h, 0x10, 3);
    }
    #[test]
    fn two_core_stop_rmw_replay_does_not_restart_disabled_channels() {
        let storage = std::sync::Arc::new(std::sync::Mutex::new(
            flash_storage::Storage::new((0..32).collect()).unwrap(),
        ));
        let mut bus = TestBus::default();
        bus.register(0xff939000, 0x1000, Box::new(Dma::new(storage.clone())))
            .unwrap();
        let mut h = RecordingHost {
            ram: vec![0xff; 64],
            events: vec![],
            irq: false,
            ..RecordingHost::default()
        };
        setup_two_channels(&mut bus, &mut h);
        wr(&mut bus, &mut h, 0x20, 0x30003);
        wr(&mut bus, &mut h, 4, 3);
        for (d, at, t) in h.events.clone() {
            h.now = at;
            bus.event(&mut Cx::new(&mut h, 0, 0, 0), d, t).unwrap();
        }
        assert_eq!(&h.ram[16..20], &[0, 1, 2, 3]);
        assert_eq!(&h.ram[24..28], &[4, 5, 6, 7]);
        let old_start = rd(&mut bus, &mut h, 4);
        // Core1 saves a shared start word in1486f18, then core0 stops its
        // channel before that old word is written back. Original model fails
        // on the replayed channel0 bit despite this being a stop operation.
        bus.write(&mut Cx::new(&mut h, 1, 0x9ed60, 0), 0xff939020, 4, 0x20000)
            .unwrap();
        wr(&mut bus, &mut h, 0x20, 0x10000);
        wr(&mut bus, &mut h, 4, old_start & !1);
        bus.write(
            &mut Cx::new(&mut h, 1, 0x9ed60, 0),
            0xff939004,
            4,
            old_start & !2,
        )
        .unwrap();
        assert_eq!(rd(&mut bus, &mut h, 4), 0);
        assert_eq!(rd(&mut bus, &mut h, 8), 0);
        assert_eq!(h.events.len(), 2);
        assert!(storage.lock().unwrap().session.controller_complete);
        // A stale disabled bit must not suppress a concurrent valid start.
        wr(&mut bus, &mut h, 0x1c, 0x30003);
        wr(&mut bus, &mut h, 0x20, 0x10001);
        wr(&mut bus, &mut h, 4, 3);
        assert_eq!(rd(&mut bus, &mut h, 4), 1);
        assert_eq!(rd(&mut bus, &mut h, 8), 1);
        assert_eq!(h.events.len(), 3);
        let (d, at, t) = h.events[2];
        h.now = at;
        bus.event(&mut Cx::new(&mut h, 0, 0, 0), d, t).unwrap();
        assert_eq!(&h.ram[20..24], &[4, 5, 6, 7]);
        assert_eq!(rd(&mut bus, &mut h, 0x18), 1);
        assert!(h.irq);
    }
    #[test]
    fn disable_cancels_deadline_and_stale_start_is_not_latched() {
        let storage = std::sync::Arc::new(std::sync::Mutex::new(
            flash_storage::Storage::new((0..32).collect()).unwrap(),
        ));
        let mut bus = TestBus::default();
        bus.register(0xff939000, 0x1000, Box::new(Dma::new(storage.clone())))
            .unwrap();
        let mut h = RecordingHost {
            ram: vec![0xff; 64],
            events: vec![],
            irq: false,
            ..RecordingHost::default()
        };
        setup_two_channels(&mut bus, &mut h);
        wr(&mut bus, &mut h, 0x20, 0x10001);
        wr(&mut bus, &mut h, 4, 1);
        let (d, at, t) = h.events[0];
        h.now = at;
        wr(&mut bus, &mut h, 0x20, 0x10000);
        wr(&mut bus, &mut h, 4, 1);
        bus.event(&mut Cx::new(&mut h, 0, 0, 0), d, t).unwrap();
        assert_eq!(&h.ram[16..20], &[0xff; 4]);
        assert!(!h.irq);
        assert!(!storage.lock().unwrap().session.controller_complete);
        wr(&mut bus, &mut h, 0x20, 0x10001);
        assert_eq!(h.events.len(), 1);
        assert_eq!(rd(&mut bus, &mut h, 4), 0);
        wr(&mut bus, &mut h, 4, 1);
        let saved = bus.save();
        bus.restore(&saved).unwrap();
        let (d, at, t) = h.events[1];
        h.now = at;
        bus.event(&mut Cx::new(&mut h, 0, 0, 0), d, t).unwrap();
        assert_eq!(&h.ram[16..20], &[0, 1, 2, 3]);
        assert!(h.irq);
        assert!(storage.lock().unwrap().session.controller_complete);
    }
}
