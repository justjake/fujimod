//! FFF480 reset, interrupt status and explicitly selected image arithmetic.
//! The selected pixel laws are declared approximations; other operations refuse.
use fujimod_emulator_imaging::exr2_isp::dma::{Raster, Transfer, physical};
use fujimod_emulator_devices::exr2::irq_routes::Irq;
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use fujimod_emulator_imaging::exr2_isp::options::combination::{Options, Arithmetic, BASE};
mod linear;
mod masked;
mod pyramid;
struct Prepared {
    pub destination: Raster,
    pub pixels: Vec<u8>,
    pub bytes: usize,
}

fn read_rows(cx: &Cx<'_>, raster: Raster, row_bytes: usize) -> Result<Vec<u8>, String> {
    raster.validate(row_bytes)?;
    let mut pixels = vec![0; row_bytes * raster.height];
    for y in 0..raster.height {
        cx.ram_read(
            physical(raster.address + (y * raster.pitch) as u64, row_bytes)?,
            &mut pixels[y * row_bytes..(y + 1) * row_bytes],
        )?;
    }
    Ok(pixels)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    registers: BTreeMap<u16, u32>,
    reset_due: Option<u64>,
    resets: u64,
    dma: Transfer,
    interrupt_asserted: bool,
}
pub struct Combination {
    options: Options,
    irq: Irq,
    state: State,
}
impl Combination {
    pub fn new(options: Options, irq: Irq) -> Result<Self, String> {
        if options.reset_ticks == 0 || irq.core > 1 || irq.line >= 128 {
            return Err("invalid image combination reset delay or physical IRQ".into());
        }
        options.clock.delay(1)?;
        Ok(Self {
            options,
            irq,
            state: State {
                registers: BTreeMap::from([(0, 1)]),
                reset_due: None,
                resets: 0,
                dma: Transfer::default(),
                interrupt_asserted: false,
            },
        })
    }
    fn offset(address: u64, size: u8) -> Option<u16> {
        let o = address.checked_sub(BASE)?;
        (size == 4
            && o & 3 == 0
            && (matches!(o, 0 | 4 | 0x60 | 0x64 | 0xc0 | 0xc4 | 0xc8 | 0xd0 | 0xd4)
                || (0x10..0x5c).contains(&o)
                || (0x70..0xbc).contains(&o)
                || (0x100..0x500).contains(&o)))
        .then_some(o as u16)
    }
    fn word(&self, o: u16) -> u32 {
        self.state.registers.get(&o).copied().unwrap_or(0)
    }
    fn sync_irq(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        let pending = self.word(0xc0) & self.word(0xc8) != 0;
        if pending && !self.state.interrupt_asserted {
            self.irq.raise(cx)?;
        }
        self.state.interrupt_asserted = pending;
        Ok(())
    }
    fn start(&mut self, cx: &Cx<'_>, control: u32) -> Result<(), String> {
        if self.state.reset_due.is_some() || self.state.dma.deadline().is_some() {
            return Err("combination start while reset or DMA is pending".into());
        }
        if self.options.arithmetic != Arithmetic::PackedImageUnmeasured
            || !matches!(control & 7, 2..=4)
        {
            return Err(format!(
                "image combination FFF48004 operation {} has no raster/DMA law",
                control & 7
            ));
        }
        let word = |o| if o == 4 { control } else { self.word(o) };
        let prepared = match control & 7 {
            2 => pyramid::prepare(cx, word),
            3 => masked::prepare(cx, word),
            4 => linear::prepare(cx, word),
            _ => unreachable!(),
        }?;
        let row_bytes = prepared.destination.width * 2;
        let delay = self.options.clock.delay(prepared.bytes)?;
        self.state
            .dma
            .prepare(cx.now(), delay, prepared.destination, row_bytes)?;
        self.state.dma.pixels = prepared.pixels.into();
        Ok(())
    }
    fn validate_state(&self, state: State) -> Result<State, String> {
        state.dma.validate()?;
        if state.registers.iter().any(|(o, v)| {
            Self::offset(BASE + u64::from(*o), 4).is_none()
                || match o {
                    0 => *v > 1,
                    4 => *v & !0x3377 != 0,
                    0xc4 => *v != 0,
                    0xc0 | 0xc8 => *v & !0x331 != 0,
                    _ => false,
                }
        }) || state.reset_due.is_some()
            && (state.resets == u64::MAX
                || state.registers.get(&0) != Some(&0)
                || state.dma.deadline().is_some())
            || state.dma.deadline().is_some()
                && (self.options.arithmetic != Arithmetic::PackedImageUnmeasured
                    || !matches!(state.registers.get(&4).copied().unwrap_or(0) & 7, 2..=4))
            || state.interrupt_asserted
                != (state.registers.get(&0xc0).copied().unwrap_or(0)
                    & state.registers.get(&0xc8).copied().unwrap_or(0)
                    != 0)
        {
            return Err("invalid image combination register/deadline state".into());
        }
        Ok(state)
    }
}
impl Device for Combination {
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        fujimod_emulator_devices::checkpoint::Checkpoint::encode(self.name(), &self.state)
    }
    fn validate_checkpoint(
        &self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        self.validate_state(saved.decode(self.name())?).map(|_| ())
    }
    fn restore_checkpoint(
        &mut self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        self.state = self.validate_state(saved.decode(self.name())?)?;
        Ok(())
    }
    fn fingerprint(&self) -> Result<Value, String> {
        Ok(
            json!({"options":self.options,"irq":self.irq,"registers":self.state.registers,
            "reset_due":self.state.reset_due,"resets":self.state.resets,
            "dma":self.state.dma.fingerprint(),"interrupt_asserted":self.state.interrupt_asserted}),
        )
    }
    fn name(&self) -> &'static str {
        "image-combination"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let Some(o) = Self::offset(address, size) else {
            cx.unmodeled();
            return 0;
        };
        self.word(o).into()
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Some(o) = Self::offset(address, size) else {
            cx.unmodeled();
            return;
        };
        let Ok(value) = u32::try_from(value) else {
            cx.fail("image combination requires a 32-bit word");
            return;
        };
        if self.state.dma.deadline().is_some() && !matches!(o, 0 | 0xc4 | 0xc8) {
            cx.fail("combination reconfiguration during DMA");
            return;
        }
        match o {
            0 => {
                if value != 0 {
                    cx.fail(
                        "image combination reset register accepts zero only; starts use FFF48004",
                    );
                    return;
                }
                if self.state.resets == u64::MAX {
                    cx.fail("image combination reset count exhausted");
                    return;
                }
                let Some(due) = cx.now().checked_add(self.options.reset_ticks) else {
                    cx.fail("image combination reset deadline overflow");
                    return;
                };
                self.state.dma.cancel();
                self.state.reset_due = Some(due);
                self.state.registers.insert(4, self.word(4) & !7);
                self.state.registers.insert(0xc0, 0);
                self.state.interrupt_asserted = false;
            }
            0xc0 => {
                cx.fail("image combination status is read-only");
                return;
            }
            0xc4 | 0xc8 => {
                if value & !0x331 != 0 {
                    cx.fail("unknown image combination interrupt bits");
                    return;
                }
                if o == 0xc4 {
                    self.state.registers.insert(0xc0, self.word(0xc0) & !value);
                } else {
                    self.state.registers.insert(o, value);
                }
                if let Err(e) = self.sync_irq(cx) {
                    cx.fail(e);
                }
                return;
            }
            4 => {
                if value & !0x3377 != 0 {
                    cx.fail("unknown image combination mode/selectors");
                    return;
                }
                if value & 7 != 0
                    && let Err(e) = self.start(cx, value)
                {
                    cx.fail(e);
                    return;
                }
            }
            _ => {}
        }
        self.state.registers.insert(o, value);
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.state.reset_due.or(self.state.dma.deadline())
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if self.state.reset_due.is_some_and(|due| due <= cx.now()) {
            self.state.registers.insert(0, 1);
            self.state.reset_due = None;
            self.state.resets += 1;
        }
        match self.state.dma.write(cx) {
            Ok(true) => {
                self.state.registers.insert(4, self.word(4) & !7);
                self.state.registers.insert(0xc0, self.word(0xc0) | 1);
                if let Err(e) = self.sync_irq(cx) {
                    cx.fail(e);
                }
            }
            Err(e) => cx.fail(e),
            _ => {}
        }
    }
    fn event(&mut self, cx: &mut Cx<'_>, _: u64) {
        self.advance(cx);
    }
    fn pollable(&self, address: u64, size: u8) -> bool {
        Self::offset(address, size).is_some()
    }
    fn save(&self) -> Value {
        json!(self.state)
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        self.state = self.validate_state(state)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_imaging::exr2_isp::dma::Clock;
    use fujimod_emulator_devices::test_host::{TestHost, advance, read, write};
    fn model(reset_ticks: u64) -> Combination {
        Combination::new(
            Options {
                reset_ticks,
                clock: Clock::Fixed { ticks: 10 },
                arithmetic: Arithmetic::Unmodeled,
            },
            Irq { core: 0, line: 68 },
        )
        .unwrap()
    }
    #[test]
    fn interrupt_acknowledgment_and_enable_are_independent_registers() {
        let mut host = TestHost::new();
        let mut d = model(16);
        d.state.registers.insert(0xc0, 0x331);
        write(&mut d, &mut host, BASE + 0xc8, 1);
        assert_eq!(read(&mut d, &mut host, BASE + 0xc0), 0x331);
        assert_eq!(read(&mut d, &mut host, BASE + 0xc8), 1);
        write(&mut d, &mut host, BASE + 0xc4, 0x31);
        assert_eq!(read(&mut d, &mut host, BASE + 0xc0), 0x300);
        write(&mut d, &mut host, BASE + 0xc8, 0);
        assert_eq!(read(&mut d, &mut host, BASE + 0xc0), 0x300);
        assert_eq!(host.irqs, [(0, 68)]);
    }

    #[test]
    fn adv_start_is_refused_at_the_start_register_without_fake_completion() {
        // Unsupported operation codes fail atomically without scheduling work.
        let mut host = TestHost::new();
        let mut d = model(16);
        write(&mut d, &mut host, BASE + 4, 0x2000);
        write(&mut d, &mut host, BASE + 0xc8, 1);
        let saved = d.save();
        let mut cx = host.cx();
        d.write(&mut cx, BASE + 4, 4, 0x2002);
        assert!(cx.take_refusal().unwrap().contains("FFF48004 operation 2"));
        assert_eq!(d.save(), saved);
        assert_eq!(d.next_deadline(0), None);
        assert!(host.effects.is_empty());
    }

    #[test]
    fn reset_roundtrip_and_early_ticks_preserve_exact_deadline_without_pixels() {
        let mut host = TestHost::new();
        let mut d = model(100);
        write(&mut d, &mut host, BASE, 0);
        let saved = d.save();
        let checkpoint = d.checkpoint().unwrap();
        advance(&mut d, &mut host, 99);
        assert_eq!(d.save(), saved);
        assert_eq!(read(&mut d, &mut host, BASE), 0);
        let mut restored = model(100);
        restored.restore_checkpoint(&checkpoint).unwrap();
        advance(&mut restored, &mut host, 100);
        advance(&mut d, &mut host, 100);
        assert_eq!(restored.save(), d.save());
        assert_eq!(read(&mut d, &mut host, BASE), 1);
        assert!(host.effects.is_empty());
        let before = d.save();
        let mut cx = host.cx();
        d.write(&mut cx, BASE, 4, 1);
        assert!(cx.take_refusal().unwrap().contains("starts use FFF48004"));
        assert_eq!(d.save(), before);
    }
    fn processing_fixture(enabled: bool) -> (Combination, TestHost) {
        let (host, words) = pyramid::tests::fixture();
        configured_fixture(enabled, host, words)
    }
    fn configured_fixture(
        enabled: bool,
        mut host: TestHost,
        words: BTreeMap<u16, u32>,
    ) -> (Combination, TestHost) {
        let address = words[&0x30] as usize;
        let bytes = words[&0x34] as usize * 2 * (words[&0x38] >> 16) as usize;
        host.ram[address..address + bytes].fill(0xee);
        let mut d = Combination::new(
            Options {
                reset_ticks: 16,
                clock: Clock::Fixed { ticks: 100 },
                arithmetic: Arithmetic::PackedImageUnmeasured,
            },
            Irq { core: 0, line: 68 },
        )
        .unwrap();
        for (&offset, &value) in &words {
            if offset != 4 {
                write(&mut d, &mut host, BASE + u64::from(offset), value);
            }
        }
        write(&mut d, &mut host, BASE + 0xc8, u32::from(enabled));
        write(&mut d, &mut host, BASE + 4, words[&4]);
        (d, host)
    }

    #[test]
    fn pyramid_dma_is_latched_and_checkpointed_before_status_and_physical_irq() {
        use fujimod_emulator_devices::test_host::advance_from_checkpoint;
        let (mut d, mut host) = processing_fixture(true);
        assert_eq!(d.next_deadline(0), Some(100));
        assert!(host.effects.is_empty());
        let saved = d.fingerprint().unwrap();
        for _ in 0..3 {
            assert_eq!(read(&mut d, &mut host, BASE + 0xc0), 0);
            assert_eq!(d.fingerprint().unwrap(), saved);
        }
        // Guest source changes after START do not replace the latched job.
        host.ram[0x10000..0x50000].fill(0);
        advance(&mut d, &mut host, 99);
        assert_eq!(d.fingerprint().unwrap(), saved);
        assert_eq!(&host.ram[0x80000..0x80180], &[0xee; 384]);
        advance_from_checkpoint(&mut d, &mut host, 100);
        assert_eq!(
            &host.ram[0x80000..0x80008],
            &[60, 65, 80, 90, 85, 90, 100, 110]
        );
        for row in 0..8 {
            assert_eq!(
                &host.ram[0x80020 + row * 48..0x80030 + row * 48],
                &[0xee; 16]
            );
        }
        assert_eq!(host.irqs, [(0, 68)]);
        assert_eq!(
            host.effects,
            [
                "dma", "dma", "dma", "dma", "dma", "dma", "dma", "dma", "irq"
            ]
        );
        assert_eq!(read(&mut d, &mut host, BASE + 0xc0), 1);
        assert_eq!(read(&mut d, &mut host, BASE + 4), 0x2000);
        assert_eq!(d.next_deadline(100), None);
    }

    #[test]
    fn masked_completion_latches_status_and_enable_delivers_one_interrupt() {
        let (mut d, mut host) = processing_fixture(false);
        advance(&mut d, &mut host, 100);
        assert!(host.irqs.is_empty());
        assert_eq!(read(&mut d, &mut host, BASE + 0xc0), 1);
        write(&mut d, &mut host, BASE + 0xc8, 1);
        write(&mut d, &mut host, BASE + 0xc8, 1);
        assert_eq!(host.irqs, [(0, 68)]);
        write(&mut d, &mut host, BASE + 0xc4, 1);
        assert_eq!(read(&mut d, &mut host, BASE + 0xc0), 0);
        assert_eq!(read(&mut d, &mut host, BASE + 0xc8), 1);
        advance(&mut d, &mut host, 1000);
        assert_eq!(host.irqs, [(0, 68)]);
    }

    #[test]
    fn reset_cancels_pending_pixels_and_configuration_cannot_mutate_a_job() {
        let (mut d, mut host) = processing_fixture(true);
        let before = d.fingerprint().unwrap();
        {
            let mut cx = host.cx();
            d.write(&mut cx, BASE + 0x34, 4, 0);
            assert!(
                cx.take_refusal()
                    .unwrap()
                    .contains("reconfiguration during DMA")
            );
        }
        assert_eq!(d.fingerprint().unwrap(), before);
        let mut malformed = d.save();
        malformed["dma"]["pixels"] = json!([]);
        assert!(d.restore(&malformed).is_err());
        assert_eq!(d.fingerprint().unwrap(), before);
        host.now = 50;
        write(&mut d, &mut host, BASE, 0);
        assert_eq!(d.next_deadline(50), Some(66));
        advance(&mut d, &mut host, 65);
        assert_eq!(read(&mut d, &mut host, BASE), 0);
        advance(&mut d, &mut host, 66);
        advance(&mut d, &mut host, 100);
        assert_eq!(read(&mut d, &mut host, BASE), 1);
        assert_eq!(read(&mut d, &mut host, BASE + 0xc0), 0);
        assert!(host.effects.is_empty());
        assert_eq!(&host.ram[0x80000..0x80180], &[0xee; 384]);
    }
    #[test]
    fn signed_sum_and_mask_blend_publish_only_after_their_restorable_dma_deadline() {
        use fujimod_emulator_devices::test_host::advance_from_checkpoint;
        for (host, words) in [linear::tests::fixture(), masked::tests::fixture()] {
            let address = words[&0x30] as usize;
            let (mut d, mut host) = configured_fixture(true, host, words);
            assert_eq!(d.next_deadline(0), Some(100));
            advance(&mut d, &mut host, 99);
            assert_eq!(host.ram[address], 0xee);
            assert!(host.effects.is_empty());
            advance_from_checkpoint(&mut d, &mut host, 100);
            assert_ne!(host.ram[address], 0xee);
            assert_eq!(host.irqs, [(0, 68)]);
            assert_eq!(host.effects.last(), Some(&"irq"));
            assert_eq!(read(&mut d, &mut host, BASE + 0xc0), 1);
            assert_eq!(d.next_deadline(100), None);
        }
    }
}
