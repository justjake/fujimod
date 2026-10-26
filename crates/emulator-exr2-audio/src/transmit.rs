//! Channel 1 DMA connected to the two serial-audio transmit FIFOs.
//!
//! Opaque RAM bytes are latched when the enabled FIFO admits a descriptor,
//! then consumed before IRQ 5C at a declared, unmeasured transfer deadline.
//! Disabled transmitters have no polling timer. The owning audio register
//! block and DMA channel share a device, so a guest format write wakes it.
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
    pub ticks_per_byte: u64,
    pub minimum_ticks: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    due: u64,
    destination: u64,
    bytes: Vec<u8>,
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
pub struct Transmitter {
    options: Options,
    state: State,
    audio: SerialAudio,
    routes: IrqRoutes,
}
impl Transmitter {
    pub fn new(options: Options, sinks: [AudioSink; 2], routes: IrqRoutes) -> Result<Self, String> {
        if options.ram_bytes == 0
            || options.ram_bytes > 0x20000000
            || options.max_transfer_bytes == 0
            || u64::from(options.max_transfer_bytes) > options.ram_bytes
            || options.ticks_per_byte == 0
            || options.minimum_ticks == 0
        {
            return Err("invalid serial-audio transmit DMA bounds or timing".into());
        }
        Ok(Self {
            options,
            state: State::default(),
            audio: SerialAudio::new(super::Options {}, sinks),
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
        value & !0x500ff007 == 0x00200010
            && value & 7 == 1
            && matches!(value >> 12 & 15, 0..=2)
            && value >> 16 & 15 == 2
    }
    fn descriptor(&self) -> Result<(u64, u64, usize), String> {
        let o = if self.state.alternate { 12 } else { 0 };
        let source = u64::from(self.word(o));
        let destination = u64::from(self.word(o + 4));
        let bytes = self.word(o + 8);
        let physical = source & 0x3fffffff;
        if !Self::configuration(self.word(0x2c))
            || self.word(0x30) != 0
            || self.word(0x34) != 0
            || bytes == 0
            || bytes > self.options.max_transfer_bytes
            || bytes & 1 != 0
            || source >= 0x60000000
            || physical + u64::from(bytes) > self.options.ram_bytes
            || !matches!(destination, 0xfffa0010 | 0xfffa1010)
        {
            return Err("unsupported serial-audio transmit descriptor".into());
        }
        Ok((source, destination, bytes as usize))
    }
    fn enabled(&self) -> bool {
        let destination = self
            .state
            .pending
            .as_ref()
            .map(|p| p.destination)
            .unwrap_or_else(|| u64::from(self.word(if self.state.alternate { 16 } else { 4 })));
        self.audio.can_transmit(destination) == Ok(true)
    }
    fn tick(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        if !self.state.active || !self.enabled() {
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
            self.audio.transmit_dma(p.destination, &p.bytes)?;
            let irq = p.irq;
            self.state.pending = None;
            self.state.completed = completed;
            self.state.alternate = self.word(0x2c) & 0x40000000 != 0;
            self.state.active = self.state.alternate && self.word(0x2c) & 0x10000000 != 0;
            irq.raise(cx)?;
        } else {
            let (source, destination, count) = self.descriptor()?;
            let delay = (count as u64)
                .checked_mul(self.options.ticks_per_byte)
                .ok_or("audio DMA duration overflow")?
                .max(self.options.minimum_ticks);
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
            let mut bytes = vec![0; count];
            cx.ram_read(source, &mut bytes)?;
            self.state.pending = Some(Pending {
                due,
                destination,
                bytes,
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
                p.bytes.is_empty()
                    || p.bytes.len() > self.options.max_transfer_bytes as usize
                    || p.bytes.len() & 1 != 0
                    || p.irq.core >= 2
                    || p.irq.line != 0x5c
                    || !matches!(p.destination, 0xfffa0010 | 0xfffa1010)
            })
        {
            return Err("invalid serial-audio transmit DMA state".into());
        }
        let mut audio = self.audio.clone();
        audio.restore(&value["audio"])?;
        Ok((state, audio))
    }
}
impl Device for Transmitter {
    fn name(&self) -> &'static str {
        "exr2-serial-audio-transmitter"
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
                5
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
                8 => {
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
        (self.state.active && self.enabled())
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
    fn setup() -> (Transmitter, TestHost) {
        let routes = IrqRoutes::default();
        routes.set(0x5c, Some(0)).unwrap();
        let options = Options {
            ram_bytes: 0x100000,
            max_transfer_bytes: 1024,
            ticks_per_byte: 2,
            minimum_ticks: 3,
        };
        let mut d = Transmitter::new(
            options,
            [
                AudioSink::new(fujimod_emulator_parts_io::audio_sink::Options { connected: true }),
                AudioSink::default(),
            ],
            routes,
        )
        .unwrap();
        let mut h = TestHost::new();
        h.ram[0x1000..0x1004].copy_from_slice(&[1, 2, 3, 4]);
        for (o, v) in [
            (0, 0x1000),
            (4, 0xfffa0010),
            (8, 4),
            (0x2c, 0x00221011),
            (0x28, 1),
        ] {
            write(&mut d, &mut h, BASE + o, v);
        }
        (d, h)
    }
    #[test]
    fn fifo_enable_arms_exact_deadline_and_consumes_latched_bytes_before_irq() {
        let (mut d, mut h) = setup();
        assert_eq!(d.next_deadline(0), None);
        let idle = d.save();
        advance(&mut d, &mut h, 100);
        assert_eq!(idle, d.save());
        write(&mut d, &mut h, 0xfffa0008, 0x20001);
        assert_eq!(d.next_deadline(100), Some(100));
        advance(&mut d, &mut h, 100);
        assert_eq!(d.next_deadline(100), Some(108));
        h.ram[0x1000..0x1004].fill(99);
        let pending = d.fingerprint().unwrap();
        advance(&mut d, &mut h, 107);
        assert_eq!(pending, d.fingerprint().unwrap());
        assert!(h.irqs.is_empty());
        assert_eq!(d.state.pending.as_ref().unwrap().bytes, [1, 2, 3, 4]);
        advance(&mut d, &mut h, 108);
        assert_eq!(d.audio.sink_bytes_received(0), Some(4));
        assert_eq!(h.irqs, [(0, 0x5c)]);
        assert_eq!(read(&mut d, &mut h, BASE + 0x24), 0x40);
        assert_eq!(d.next_deadline(108), None);
        advance(&mut d, &mut h, 200);
        assert_eq!(d.audio.sink_bytes_received(0), Some(4));
    }
    #[test]
    fn repeat_consumes_one_handoff_and_abort_blocks_late_rearm() {
        let (mut d, mut h) = setup();
        for (o, value) in [(12, 0x1000), (16, 0xfffa0010), (20, 4), (0x2c, 0x50221011)] {
            write(&mut d, &mut h, BASE + o, value);
        }
        write(&mut d, &mut h, 0xfffa0008, 0x20001);
        advance(&mut d, &mut h, 0);
        advance(&mut d, &mut h, 8);
        advance(&mut d, &mut h, 8);
        assert_eq!(read(&mut d, &mut h, BASE + 0x2c) & 0x10000000, 0);
        advance(&mut d, &mut h, 16);
        assert_eq!(d.next_deadline(16), None);
        assert_eq!(d.audio.sink_bytes_received(0), Some(8));
        assert_eq!(h.irqs.len(), 2);
        write(&mut d, &mut h, BASE + 0x2c, 0x50221011);
        advance(&mut d, &mut h, 16);
        advance(&mut d, &mut h, 24);
        assert_eq!(d.audio.sink_bytes_received(0), Some(12));
        assert_eq!(d.next_deadline(24), None);
        write(&mut d, &mut h, BASE + 0x28, 8);
        write(&mut d, &mut h, BASE + 0x2c, 0x50221011);
        advance(&mut d, &mut h, 32);
        assert_eq!(d.audio.sink_bytes_received(0), Some(12));
    }
    #[test]
    fn stop_and_checkpoint_keep_pending_work_and_disconnected_sinks_do_not_complete() {
        let (mut d, mut h) = setup();
        write(&mut d, &mut h, 0xfffa0008, 0x20001);
        advance(&mut d, &mut h, 0);
        let saved = d.checkpoint().unwrap();
        write(&mut d, &mut h, BASE + 0x28, 8);
        advance(&mut d, &mut h, 100);
        assert!(h.irqs.is_empty());
        d.restore_checkpoint(&saved).unwrap();
        advance(&mut d, &mut h, 8);
        assert_eq!(d.audio.sink_bytes_received(0), Some(4));
        let (mut d, mut h) = setup();
        d.audio.sinks[0].set_connected(false);
        write(&mut d, &mut h, 0xfffa0008, 0x20001);
        advance(&mut d, &mut h, 0);
        h.now = 8;
        let mut cx = h.cx();
        d.advance(&mut cx);
        assert!(cx.refusal().is_some());
        assert_eq!(d.audio.sink_bytes_received(0), Some(0));
        assert!(h.irqs.is_empty());
    }
}
