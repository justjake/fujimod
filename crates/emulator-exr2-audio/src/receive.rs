//! Clocked external signed 16-bit stereo PCM input and channel 1 receive DMA.
//!
//! The source rate and constant sample are explicit board inputs. Receive
//! enable and DMA descriptors gate transfers; the declared virtual sample
//! clock determines their deadlines. DMA stores precede completion IRQ 5C.
use super::SerialAudio;
use fujimod_emulator_devices::{Cx, Device};
use fujimod_emulator_devices::exr2::irq_routes::{Irq, IrqRoutes};
use fujimod_emulator_parts_io::audio_sink::AudioSink;
use fujimod_emulator_exr2_serial::dma::audio::BASE;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub ram_bytes: u64,
    pub max_transfer_bytes: u32,
    pub ticks_per_second: u64,
    pub sample_rate: u32,
    pub sample: i16,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    due: u64,
    source: u64,
    destination: u64,
    count: u32,
    irq: Irq,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    registers: BTreeMap<u16, u32>,
    active: bool,
    alternate: bool,
    completed: u64,
    pending: Option<Pending>,
}
pub struct Receiver {
    options: Options,
    state: State,
    audio: SerialAudio,
    routes: IrqRoutes,
}
impl Receiver {
    pub fn new(options: Options, routes: IrqRoutes) -> Result<Self, String> {
        if options.ram_bytes == 0
            || options.ram_bytes > 0x20000000
            || options.max_transfer_bytes == 0
            || u64::from(options.max_transfer_bytes) > options.ram_bytes
            || options.ticks_per_second == 0
            || options.sample_rate != 48000
        {
            return Err("invalid serial-audio receive DMA bounds or timing".into());
        }
        Ok(Self {
            options,
            state: State::default(),
            audio: SerialAudio::new(
                super::Options {},
                [AudioSink::default(), AudioSink::default()],
            ),
            routes,
        })
    }
    fn offset(address: u64, size: u8) -> Option<u16> {
        let o = address.checked_sub(BASE)?;
        (size == 4
            && matches!(
                o,
                0 | 4 | 8 | 12 | 16 | 20 | 0x24 | 0x28 | 0x2c | 0x30 | 0x34
            ))
        .then_some(o as u16)
    }
    fn word(&self, o: u16) -> u32 {
        self.state.registers.get(&o).copied().unwrap_or(0)
    }
    pub(crate) fn configuration(value: u32) -> bool {
        value & !0x50000000 == 0x00122011
    }
    fn descriptor(&self) -> Result<(u64, u64, usize), String> {
        let o = if self.state.alternate { 12 } else { 0 };
        let source = u64::from(self.word(o));
        let destination = u64::from(self.word(o + 4));
        let bytes = self.word(o + 8);
        let physical = destination & 0x3fffffff;
        if !Self::configuration(self.word(0x2c))
            || self.word(0x30) != 0
            || self.word(0x34) != 0
            || bytes == 0
            || bytes > self.options.max_transfer_bytes
            || bytes & 3 != 0
            || destination >= 0x80000000
            || destination & 3 != 0
            || physical + u64::from(bytes) > self.options.ram_bytes
            || !matches!(source, 0xfffa0020 | 0xfffa1020)
        {
            return Err("unsupported serial-audio receive descriptor".into());
        }
        Ok((source, destination, bytes as usize))
    }
    fn enabled(&self) -> Result<bool, String> {
        let source = self
            .state
            .pending
            .as_ref()
            .map(|p| p.source)
            .unwrap_or_else(|| u64::from(self.word(if self.state.alternate { 12 } else { 0 })));
        self.audio.can_receive(source, self.options.sample_rate)
    }
    fn tick(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        if !self.state.active || !self.enabled()? {
            return Ok(());
        }
        if let Some(p) = self.state.pending.as_ref() {
            if p.due > cx.now() {
                return Ok(());
            }
            let completed = self
                .state
                .completed
                .checked_add(1)
                .ok_or("audio DMA completion counter overflow")?;
            self.routes.validate(p.irq)?;
            let sample = self.options.sample.to_le_bytes();
            let bytes: Vec<u8> = sample.into_iter().cycle().take(p.count as usize).collect();
            cx.ram_write(p.destination, &bytes)?;
            let irq = p.irq;
            self.state.pending = None;
            self.state.completed = completed;
            self.state.alternate = self.word(0x2c) & 0x40000000 != 0;
            self.state.active = self.state.alternate && self.word(0x2c) & 0x10000000 != 0;
            irq.raise(cx)?;
        } else {
            let (source, destination, count) = self.descriptor()?;
            let delay = (count as u64)
                .checked_mul(self.options.ticks_per_second)
                .ok_or("audio DMA duration overflow")?
                .div_ceil(u64::from(self.options.sample_rate) * 4);
            let due = cx
                .now()
                .checked_add(delay)
                .ok_or("audio DMA deadline overflow")?;
            let irq = self.routes.latch(0x5c)?;
            // Bit 28 hands off one staged alternate descriptor. Consuming
            // it prevents a repeat channel from replaying an unstaged buffer.
            if self.state.alternate {
                self.state.registers.insert(0x2c, self.word(0x2c) & !0x10000000);
            }
            self.state.pending = Some(Pending {
                due,
                source,
                destination,
                count: count as u32,
                irq,
            });
        }
        Ok(())
    }
    pub(crate) fn dma_active(&self) -> bool {
        self.state.active
    }
    pub(crate) fn validate_state(&self, value: &Value) -> Result<(), String> {
        self.decode(value).map(|_| ())
    }
    fn decode(&self, value: &Value) -> Result<(State, SerialAudio), String> {
        let state: State =
            serde_json::from_value(value["dma"].clone()).map_err(|e| e.to_string())?;
        if state.registers.iter().any(|(&o, &v)| {
            Self::offset(BASE + u64::from(o), 4).is_none()
                || o == 0x24
                || matches!(o, 0x30 | 0x34) && v != 0
                || o == 0x2c && !Self::configuration(v)
        }) || state.pending.is_some() && !state.active
            || state.pending.as_ref().is_some_and(|p| {
                p.count == 0
                    || p.count > self.options.max_transfer_bytes
                    || p.count & 3 != 0
                    || p.irq.core >= 2
                    || p.irq.line != 0x5c
                    || !matches!(p.source, 0xfffa0020 | 0xfffa1020)
                    || p.destination >= 0x80000000
                    || p.destination & 3 != 0
                    || (p.destination & 0x3fffffff) + u64::from(p.count) > self.options.ram_bytes
            })
        {
            return Err("invalid serial-audio receive DMA state".into());
        }
        let mut audio = self.audio.clone();
        audio.restore(&value["audio"])?;
        Ok((state, audio))
    }
}
impl Device for Receiver {
    fn name(&self) -> &'static str {
        "exr2-serial-audio-receiver"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        if !(BASE..BASE + 0x40).contains(&address) {
            return self.audio.read(cx, address, size);
        }
        let Some(o) = Self::offset(address, size) else {
            cx.unmodeled();
            return 0;
        };
        if o == 0x24 {
            return if self.state.active {
                // Completion clears BUSY even when the repeat channel stays
                // armed. Software may drain the channel from its completion
                // callback before disabling the serial receiver.
                if self.state.completed == 0 { 5 } else { 4 }
            } else if self.state.completed != 0 {
                0x40
            } else {
                0
            };
        }
        self.word(o).into()
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        if !(BASE..BASE + 0x40).contains(&address) {
            self.audio.write(cx, address, size, value);
            return;
        }
        let Some(o) = Self::offset(address, size) else {
            cx.unmodeled();
            return;
        };
        let Ok(value) = u32::try_from(value) else {
            cx.fail("audio DMA value exceeds 32 bits");
            return;
        };
        if o == 0x24
            || matches!(o, 0x30 | 0x34) && value != 0
            || o == 0x2c && !Self::configuration(value)
        {
            cx.fail("unsupported serial-audio DMA register value");
            return;
        }
        if o == 0x28 {
            match value {
                v if v & !0x10b == 0 && v & (8 | 0x100 | 2) != 0 => {
                    self.state.active = false;
                    self.state.alternate = false;
                    self.state.pending = None;
                    self.state.completed = 0;
                }
                1 | 5 => {
                    if self.state.active {
                        cx.fail("audio DMA already active");
                        return;
                    }
                    if let Err(e) = self.descriptor() {
                        cx.fail(e);
                        return;
                    }
                    self.state.active = true;
                    self.state.alternate = false;
                    self.state.completed = 0;
                }
                _ => {
                    cx.fail("unsupported serial-audio DMA control");
                    return;
                }
            }
        }
        self.state.registers.insert(o, value);
        // A late handoff resumes a drained repeat channel. An abort clears
        // alternate/completed and requires a new START instead.
        if o == 0x2c && value & 0x50000000 == 0x50000000
            && self.state.alternate && self.state.completed != 0 && self.state.pending.is_none()
        {
            self.state.active = true;
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if let Err(e) = self.tick(cx) {
            cx.fail(e);
        }
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        (self.state.active && self.enabled() != Ok(false))
            .then(|| self.state.pending.as_ref().map_or(now, |p| p.due.max(now)))
    }
    fn pollable(&self, address: u64, size: u8) -> bool {
        Self::offset(address, size).is_some()
    }
    fn save(&self) -> Value {
        json!({"dma":self.state,"audio":self.audio.save()})
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let (state, audio) = self.decode(value)?;
        self.state = state;
        self.audio = audio;
        Ok(())
    }
    fn fingerprint(&self) -> Result<Value, String> {
        Ok(json!({"options":self.options,"state":self.save()}))
    }
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        fujimod_emulator_devices::checkpoint::Checkpoint::encode(self.name(), &(self.options, self.save()))
    }
    fn validate_checkpoint(&self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        let (options, value): (Options, Value) = saved.decode(self.name())?;
        if options != self.options {
            return Err("audio DMA snapshot configuration differs".into());
        }
        self.decode(&value).map(|_| ())
    }
    fn restore_checkpoint(&mut self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        let (_, value): (Options, Value) = saved.decode(self.name())?;
        self.restore(&value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::{TestHost, advance, read, write};
    fn setup() -> (Receiver, TestHost) {
        let routes = IrqRoutes::default();
        routes.set(0x5c, Some(0)).unwrap();
        let d = Receiver::new(
            Options {
                ram_bytes: 0x100000,
                max_transfer_bytes: 0x10000,
                ticks_per_second: 12_000_000,
                sample_rate: 48000,
                sample: -1234,
            },
            routes,
        )
        .unwrap();
        let mut h = TestHost::new();
        h.ram[0x1000..0x1020].fill(0xa5);
        (d, h)
    }
    fn start(d: &mut Receiver, h: &mut TestHost, repeat: bool) {
        for (o, v) in [
            (0, 0xfffa0020),
            (4, 0x40001000),
            (8, 8),
            (12, 0xfffa0020),
            (16, 0x40001010),
            (20, 8),
            (0x2c, if repeat { 0x40122011 } else { 0x122011 }),
            (0x28, 1),
        ] {
            write(d, h, BASE + o, v);
        }
        if repeat {
            write(d, h, BASE + 0x2c, 0x50122011);
        }
    }
    #[test]
    fn explicit_pcm_clock_dma_precedes_irq_and_disabled_receiver_has_no_deadline() {
        let (mut d, mut h) = setup();
        start(&mut d, &mut h, false);
        assert_eq!(d.next_deadline(0), None);
        advance(&mut d, &mut h, 100);
        assert!(h.irqs.is_empty());
        write(&mut d, &mut h, 0xfffa0008, 0x10002);
        assert_eq!(d.next_deadline(100), Some(100));
        advance(&mut d, &mut h, 100);
        assert_eq!(d.next_deadline(100), Some(600));
        advance(&mut d, &mut h, 599);
        assert_eq!(&h.ram[0x1000..0x1008], &[0xa5; 8]);
        advance(&mut d, &mut h, 600);
        assert_eq!(&h.ram[0x1000..0x1008], &(-1234i16).to_le_bytes().repeat(4));
        assert_eq!(h.effects, ["dma", "irq"]);
        assert_eq!(read(&mut d, &mut h, BASE + 0x24), 0x40);
        assert_eq!(d.next_deadline(600), None);
    }
    #[test]
    fn repeated_buffers_latch_descriptors_replay_and_abort_without_extra_completion() {
        let (mut d, mut h) = setup();
        start(&mut d, &mut h, true);
        write(&mut d, &mut h, 0xfffa0008, 0x10002);
        advance(&mut d, &mut h, 0);
        let saved = d.checkpoint().unwrap();
        let before = h.clone();
        advance(&mut d, &mut h, 500);
        let first = d.fingerprint().unwrap();
        d.restore_checkpoint(&saved).unwrap();
        h = before;
        advance(&mut d, &mut h, 500);
        assert_eq!(first, d.fingerprint().unwrap());
        advance(&mut d, &mut h, 500);
        write(&mut d, &mut h, BASE + 16, 0x1020);
        advance(&mut d, &mut h, 1000);
        assert_eq!(&h.ram[0x1010..0x1018], &(-1234i16).to_le_bytes().repeat(4));
        assert_eq!(h.irqs.len(), 2);
        assert_eq!(read(&mut d, &mut h, BASE + 0x24), 0x40);
        write(&mut d, &mut h, BASE + 0x28, 0x101);
        advance(&mut d, &mut h, 2000);
        assert_eq!(h.irqs.len(), 2);
        assert_eq!(d.next_deadline(2000), None);
    }
    #[test]
    fn staged_alternate_is_consumed_once_then_a_new_movie_can_start() {
        let (mut d, mut h) = setup();
        start(&mut d, &mut h, true);
        write(&mut d, &mut h, 0xfffa0008, 0x10002);
        advance(&mut d, &mut h, 0);
        advance(&mut d, &mut h, 500);
        advance(&mut d, &mut h, 500);
        assert_eq!(read(&mut d, &mut h, BASE + 0x2c) & 0x10000000, 0);
        advance(&mut d, &mut h, 1000);
        assert_eq!(d.next_deadline(1000), None);
        assert_eq!(read(&mut d, &mut h, BASE + 0x24), 0x40);
        advance(&mut d, &mut h, 2000);
        assert_eq!(h.irqs.len(), 2);
        write(&mut d, &mut h, BASE + 0x2c, 0x50122011);
        assert_eq!(d.next_deadline(2000), Some(2000));
        advance(&mut d, &mut h, 2000);
        advance(&mut d, &mut h, 2500);
        assert_eq!(h.irqs.len(), 3);
        assert_eq!(d.next_deadline(2500), None);
        start(&mut d, &mut h, true);
        assert_eq!(read(&mut d, &mut h, BASE + 0x24), 5);
        advance(&mut d, &mut h, 2500);
        advance(&mut d, &mut h, 3000);
        assert_eq!(h.irqs.len(), 4);
        assert_eq!(&h.ram[0x1000..0x1008], &(-1234i16).to_le_bytes().repeat(4));
    }
    #[test]
    fn repeat_rearms_while_its_irq_bank_is_masked() {
        let (mut d, mut h) = setup();
        start(&mut d, &mut h, true);
        write(&mut d, &mut h, 0xfffa0008, 0x10002);
        advance(&mut d, &mut h, 0);
        advance(&mut d, &mut h, 500);
        d.routes.set(0x5c, None).unwrap();
        advance(&mut d, &mut h, 500);
        assert_eq!(d.next_deadline(500), Some(1000));
        advance(&mut d, &mut h, 1000);
        assert_eq!(&h.ram[0x1010..0x1018], &(-1234i16).to_le_bytes().repeat(4));
        assert_eq!(h.effects, ["dma", "irq", "dma", "irq"]);
        assert_eq!(h.irqs, [(0, 0x5c), (0, 0x5c)]);
        write(&mut d, &mut h, BASE + 0x28, 0x101);
        advance(&mut d, &mut h, 2000);
        assert_eq!(h.irqs.len(), 2);
    }
    #[test]
    fn unsupported_packing_refuses_and_bad_descriptors_never_store_or_interrupt() {
        let (mut d, mut h) = setup();
        start(&mut d, &mut h, false);
        write(&mut d, &mut h, 0xfffa0008, 0x10001);
        d.advance(&mut h.cx());
        let mut cx = h.cx();
        d.advance(&mut cx);
        assert!(cx.refusal().is_some());
        assert!(h.irqs.is_empty());
        let (mut d, mut h) = setup();
        for (o, v) in [(0, 0xfffa0020), (4, 0xffff0), (8, 64), (0x2c, 0x122011)] {
            write(&mut d, &mut h, BASE + o, v);
        }
        let mut cx = h.cx();
        d.write(&mut cx, BASE + 0x28, 4, 1);
        assert!(cx.refusal().is_some());
        assert!(h.effects.is_empty());
    }
}
