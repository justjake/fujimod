//! XP4 MOVDC output DMA. The packed422 source and clock are explicit scenario
//! inputs, not an ISP implementation or measured silicon timing.
use fujimod_emulator_imaging::frame;
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Default, Serialize, Deserialize)]
struct State {
    regs: Vec<u32>,
    reset_deadline: Option<u64>,
    frame_deadline: Option<u64>,
    pending: bool,
    frames: u64,
}
pub struct Movdc {
    base: u64,
    irq: u32,
    reset_ticks: u64,
    period_ticks: u64,
    luma: u8,
    state: State,
}
impl Movdc {
    pub fn new(
        base: u64,
        irq: u32,
        reset_ticks: u64,
        period_ticks: u64,
        luma: u8,
    ) -> Result<Self, String> {
        if ![0xfff72000, 0xfff73000].contains(&base)
            || !(32..512).contains(&irq)
            || reset_ticks == 0
            || period_ticks <= reset_ticks
            || !(16..=235).contains(&luma)
        {
            return Err("Invalid XP4 MOVDC wiring or source clock/luma".into());
        }
        Ok(Self {
            base,
            irq,
            reset_ticks,
            period_ticks,
            luma,
            state: State {
                regs: vec![0; 0x10c / 4],
                ..State::default()
            },
        })
    }
    fn r(&self, offset: usize) -> u32 {
        self.state.regs[offset / 4]
    }
    fn active(&self) -> bool {
        self.state.reset_deadline.is_none() && self.r(4) == 1 && self.r(0xd4) & 0x10000000 != 0
    }
    fn irq(&self, cx: &mut Cx<'_>) {
        cx.set_irq(
            self.irq,
            self.state.pending && self.r(0xd4) & 0x10000000 != 0,
        );
    }
    fn arm(&mut self, cx: &mut Cx<'_>) {
        if !self.active() {
            self.state.frame_deadline = None;
        } else if self.state.frame_deadline.is_none() {
            self.state.frame_deadline = cx.now().checked_add(self.period_ticks);
            if self.state.frame_deadline.is_none() {
                cx.fail("XP4 MOVDC deadline overflow");
            }
        }
        self.irq(cx);
    }
    fn produce(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        // 011117ac selects +d4[18:16] by divisibility of the output width
        // by (8 << exponent) for this format. 01f31664/01f31aa8 select
        // +d8[17:16] with table 0140a944 = [3,3,3,2,1,0]. These are
        // horizontal transfer controls, not alternate pixel formats.
        let exponent = (self.r(0xd4) >> 16) & 7;
        let selector = [3, 3, 3, 2, 1, 0].get(exponent as usize);
        let width = (self.r(0x18) & 0x7ff) + 1;
        if self.r(8) != 0x100
            || selector.is_none_or(|selector| self.r(0xd8) != 0x10000002 | selector << 16)
            || self.r(0xd4) & !0x10070000 != 0
            || width % (8 << exponent) != 0
            || self.r(0x18) & !0x07ff07ff != 0
            || self.r(0xc0) > 3
            || self.r(0xc8) != 0
            || self.r(0xcc) != 0
        {
            return Err("Unsupported XP4 MOVDC output mode/size/address".into());
        }
        let size = self.r(0x18);
        let address = (u64::from(self.r(0xc0)) << 32) | u64::from(self.r(0xc4));
        if address == 0 || address & 7 != 0 {
            return Err("XP4 MOVDC needs a nonzero aligned DMA destination".into());
        }
        frame::paint(
            cx,
            address,
            width,
            (size >> 16) + 1,
            self.r(0xd0),
            self.luma,
        )?;
        self.state.frames += 1;
        self.state.pending = true;
        self.irq(cx);
        Ok(())
    }
    fn known(offset: u64) -> bool {
        // This is the DMA front end with an explicit processed-frame input.
        // ISP coefficients, crop/filter controls, watermark fields and +e0
        // counters stay unmodeled and in the discovery ledger.
        matches!(
            offset,
            0 | 4 | 8 | 0x18 | 0xc0 | 0xc4 | 0xc8 | 0xcc | 0xd0 | 0xd4 | 0xd8 | 0xdc
        )
    }
}
impl Device for Movdc {
    fn name(&self) -> &'static str {
        if self.base == 0xfff72000 {
            "xp4-movdc0"
        } else {
            "xp4-movdc1"
        }
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        self.advance(cx);
        let offset = address.wrapping_sub(self.base);
        if size != 4 || !Self::known(offset) {
            cx.unmodeled();
            return 0; // +e0's two 5-bit counters are unresolved.
        }
        match offset {
            0 => u64::from(self.state.reset_deadline.is_none()),
            0xdc => u64::from(self.state.pending) | if self.active() { 0 } else { 0x100 },
            _ => u64::from(self.r(offset as usize)),
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        self.advance(cx);
        let offset = address.wrapping_sub(self.base);
        if size != 4 || !Self::known(offset) {
            cx.unmodeled();
            return;
        }
        let value = value as u32;
        match offset {
            0 => {
                if value != 0 {
                    cx.unmodeled();
                    return;
                }
                self.state.regs[1] = 0;
                self.state.frame_deadline = None;
                self.state.pending = false;
                self.state.reset_deadline = cx.now().checked_add(self.reset_ticks);
                if self.state.reset_deadline.is_none() {
                    cx.fail("XP4 MOVDC reset deadline overflow");
                }
            }
            0xdc => {
                if value & 1 != 0 {
                    self.state.pending = false;
                }
            }
            _ => {
                if offset == 4 && value > 1 {
                    cx.unmodeled();
                    return;
                }
                if offset == 4 && value == 1 && self.state.reset_deadline.is_some() {
                    cx.fail("XP4 MOVDC started during reset");
                    return;
                }
                self.state.regs[offset as usize / 4] = value;
            }
        }
        self.arm(cx);
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if self.state.reset_deadline.is_some_and(|at| at <= cx.now()) {
            self.state.reset_deadline = None;
        }
        while let Some(at) = self.state.frame_deadline.filter(|at| *at <= cx.now()) {
            self.state.frame_deadline = None;
            if let Err(error) = self.produce(cx) {
                cx.fail(error);
                return;
            }
            if self.active() {
                self.state.frame_deadline = at.checked_add(self.period_ticks);
                if self.state.frame_deadline.is_none() {
                    cx.fail("XP4 MOVDC deadline overflow");
                    return;
                }
            }
        }
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        [self.state.reset_deadline, self.state.frame_deadline]
            .into_iter()
            .flatten()
            .min()
    }
    fn save(&self) -> Value {
        serde_json::to_value(&self.state).unwrap()
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let s: State = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        if s.regs.len() != 0x10c / 4
            || s.regs[1] > 1
            || s.frame_deadline.is_some()
                != (s.regs[1] == 1
                    && s.regs[0xd4 / 4] & 0x10000000 != 0
                    && s.reset_deadline.is_none())
        {
            return Err("Invalid XP4 MOVDC checkpoint".into());
        }
        self.state = s;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::{Host, Record};
    struct Clock {
        now: u64,
        ram: Vec<u8>,
        irqs: Vec<(u64, u32, bool)>,
    }
    impl Host for Clock {
        fn now(&self) -> u64 {
            self.now
        }
        fn schedule(&mut self, _: usize, _: u64, _: u64) {
            unreachable!()
        }
        fn set_irq(&mut self, irq: u32, level: bool) {
            self.irqs.push((self.now, irq, level));
        }
        fn raise_irq(&mut self, _: usize, _: u32) -> Result<(), String> {
            unreachable!()
        }
        fn ram_read(&self, _: u64, _: &mut [u8]) -> Result<(), String> {
            unreachable!()
        }
        fn ram_write(&mut self, address: u64, bytes: &[u8]) -> Result<(), String> {
            self.ram
                .get_mut(address as usize..address as usize + bytes.len())
                .ok_or("test DMA bounds")?
                .copy_from_slice(bytes);
            Ok(())
        }
        fn record(&mut self, _: Record) {}
    }
    fn put(d: &mut Movdc, h: &mut Clock, pc: u64, offset: u64, value: u64) {
        let base = d.base;
        let mut cx = Cx::new(h, 3, pc, 0);
        d.write(&mut cx, base + offset, 4, value);
        assert_eq!(cx.refusal(), None);
    }
    fn setup(pc: u64, mode: u64) -> (Movdc, Clock) {
        let mut d = Movdc::new(0xfff73000, 108, 3, 20, 81).unwrap();
        let mut h = Clock {
            now: 10,
            ram: vec![0xaa; 256],
            irqs: vec![],
        };
        put(&mut d, &mut h, pc, 0, 0);
        assert_eq!(d.next_deadline(10), Some(13));
        h.now = 12;
        let before = d.save();
        d.advance(&mut Cx::new(&mut h, 1, pc, 0));
        assert_eq!(d.save(), before);
        h.now = 13;
        d.advance(&mut Cx::new(&mut h, 0, pc, 0));
        for (offset, value) in [
            (8, 0x100),
            (0x18, 0x1001f),
            (0xc4, 64),
            (0xd0, 80),
            (0xd8, mode),
            (0xd4, 0x10020000),
            (4, 1),
        ] {
            put(&mut d, &mut h, pc, offset, value);
        }
        (d, h)
    }
    fn experiment(pc: u64, early: bool) -> (Value, Vec<u8>, Vec<(u64, u32, bool)>) {
        let (mut d, mut h) = setup(pc, 0x10030002);
        assert_eq!(d.next_deadline(13), Some(33));
        if early {
            for at in 14..33 {
                h.now = at;
                let before = d.save();
                d.advance(&mut Cx::new(&mut h, 2, pc, 0));
                assert_eq!(d.save(), before);
            }
        }
        assert!(h.ram.iter().all(|v| *v == 0xaa));
        h.now = 33;
        d.advance(&mut Cx::new(&mut h, 0, pc, 0));
        assert_eq!(&h.ram[64..128], [81, 81, 128, 128].repeat(16));
        assert_eq!(&h.ram[144..208], &h.ram[64..128]);
        assert_eq!(&h.ram[128..144], &[0xaa; 16]);
        assert!(h.irqs.contains(&(33, 108, true)));
        assert_eq!(d.read(&mut Cx::new(&mut h, 0, pc, 0), 0xfff730dc, 4), 1);
        assert!(d.state.pending); // Status reads do not acknowledge.
        put(&mut d, &mut h, pc, 0xdc, 1);
        assert!(!d.state.pending);
        assert_eq!(d.next_deadline(33), Some(53));
        let before = d.save();
        for _ in 0..100 {
            assert_eq!(d.save(), before);
        }
        let mut restored = Movdc::new(0xfff73000, 108, 3, 20, 81).unwrap();
        restored.restore(&before).unwrap();
        assert_eq!(restored.save(), before);
        put(&mut d, &mut h, pc, 0xd4, 0x20000);
        assert_eq!(d.next_deadline(33), None);
        h.now = 100;
        d.advance(&mut Cx::new(&mut h, 1, pc, 0));
        assert_eq!(d.state.frames, 1);
        assert_eq!(d.read(&mut Cx::new(&mut h, 0, pc, 0), 0xfff730dc, 4), 0x100);
        (d.save(), h.ram, h.irqs)
    }
    #[test]
    fn dma_deadline_irq_ack_stop_and_observations_are_program_independent() {
        assert_eq!(experiment(0x1000, false), experiment(0x987654, true));
    }
    #[test]
    fn horizontal_transfer_exponent_and_selector_preserve_packed_rows() {
        for exponent in 0..6 {
            let (mut d, mut h) = setup(0, 0x10030002);
            let width = 8 << exponent;
            let stride = width * 2 + 16;
            h.ram = vec![0xaa; 64 + stride as usize * 2];
            put(&mut d, &mut h, 0, 0x18, 0x10000 | (width - 1));
            put(&mut d, &mut h, 0, 0xd0, stride);
            put(&mut d, &mut h, 0, 0xd4, 0x10000000 | exponent << 16);
            put(
                &mut d,
                &mut h,
                0,
                0xd8,
                0x10000002 | [3, 3, 3, 2, 1, 0][exponent as usize] << 16,
            );
            h.now = 33;
            let mut cx = Cx::new(&mut h, 0, 0, 0);
            d.advance(&mut cx);
            assert_eq!(cx.refusal(), None);
            let row = [81, 81, 128, 128].repeat(width as usize / 2);
            assert_eq!(&h.ram[64..64 + row.len()], row);
            assert_eq!(
                &h.ram[64 + stride as usize..64 + stride as usize + row.len()],
                row
            );
            assert!(h.ram[64 + row.len()..64 + stride as usize]
                .iter()
                .all(|v| *v == 0xaa));
            assert_eq!(d.state.frames, 1);
        }
        for (offset, value) in [(0xd4, 0x10060000), (0xd4, 0x10040000), (0xd8, 0x10010002)] {
            let (mut d, mut h) = setup(0, 0x10030002);
            put(&mut d, &mut h, 0, offset, value);
            h.now = 33;
            let mut cx = Cx::new(&mut h, 0, 0, 0);
            d.advance(&mut cx);
            assert!(cx.refusal().is_some());
            assert!(!d.state.pending);
            assert!(h.ram.iter().all(|v| *v == 0xaa));
        }
    }
    #[test]
    fn unselected_modes_refuse_before_dma_or_completion() {
        let (mut d, mut h) = setup(0, 0x10030001);
        for offset in [0x20, 0x40, 0xe0, 0xe4, 0x100] {
            let before = d.save();
            let mut cx = Cx::new(&mut h, 0, 0, 0);
            d.write(&mut cx, 0xfff73000 + offset, 4, 0x1234);
            assert!(cx.is_unmodeled());
            assert_eq!(d.save(), before);
        }
        h.now = 33;
        let mut cx = Cx::new(&mut h, 0, 0, 0);
        d.advance(&mut cx);
        assert!(cx.refusal().unwrap().contains("output mode"));
        assert_eq!(d.state.frames, 0);
        assert!(!d.state.pending);
        assert!(h.ram.iter().all(|v| *v == 0xaa));
    }
}
