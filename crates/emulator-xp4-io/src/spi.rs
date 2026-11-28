//! XP4 synchronous serial FIFO. The input pin is explicit; this does not
//! implement a companion protocol.
//!
//! `+0x24` bits 0 and 1 enable the controller's transmit and receive DMA
//! requests. With both set, a DMA engine may run a full-duplex exchange
//! through the data register (`+0x8`): every transmitted word shifts through
//! the same input peer and takes the same shift time as a programmed-I/O
//! word, and status bit 4 (busy) stays set until the last word has shifted.
//! Programmed-I/O data accesses while requests are enabled, and any change
//! to the shifter's configuration during an exchange, are refused.
use fujimod_emulator_devices::{Cx, Device, Record, Value as Field, checkpoint::Checkpoint};
use serde_json::{Value, json};
use std::collections::VecDeque;

pub trait Input: Send {
    fn exchange(&mut self, bits: u32, tx: u16, tick: u64) -> Result<u16, String>;
    /// A peer that observes clock polarity and phase can validate the SPI mode.
    fn exchange_in_mode(
        &mut self,
        bits: u32,
        _mode: u32,
        tx: u16,
        tick: u64,
    ) -> Result<u16, String> {
        self.exchange(bits, tx, tick)
    }
    fn checkpoint(&self) -> Result<Checkpoint, String>;
    fn validate_checkpoint(&self, data: &Checkpoint) -> Result<(), String>;
    fn restore_checkpoint(&mut self, data: &Checkpoint) -> Result<(), String>;
    fn save(&self) -> Value;
    fn restore(&mut self, value: &Value) -> Result<(), String>;
    /// Move a word the outside world drives (a switch the user moves).
    /// `Ok(false)`: this part has no such word.
    fn set_input(&mut self, _register: u8, _value: u16) -> Result<bool, String> {
        Ok(false)
    }
}
pub struct FixedPin(pub bool);
impl Input for FixedPin {
    fn exchange(&mut self, bits: u32, _: u16, _: u64) -> Result<u16, String> {
        Ok(if self.0 { u16::MAX >> (16 - bits) } else { 0 })
    }
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        Checkpoint::encode("xp4-spi-fixed-pin", &self.0)
    }
    fn validate_checkpoint(&self, data: &Checkpoint) -> Result<(), String> {
        if data.decode::<bool>("xp4-spi-fixed-pin")? != self.0 {
            return Err("XP4 SPI fixed pin differs".into());
        }
        Ok(())
    }
    fn restore_checkpoint(&mut self, data: &Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(data)
    }
    fn save(&self) -> Value {
        json!({"kind":"fixed-pin", "miso":self.0})
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        if value != &self.save() {
            return Err("XP4 SPI fixed pin differs".into());
        }
        Ok(())
    }
}
/// A full-duplex DMA exchange the controller admitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DmaExchange {
    /// Bits in one word.
    pub bits: u32,
    /// Ticks one word takes to shift (the programmed-I/O shift time).
    pub word_ticks: u64,
    /// Tick at which the last word has shifted.
    pub end: u64,
}
pub struct Spi {
    base: u64,
    input: Box<dyn Input>,
    control: u32,
    enable: u32,
    divider: u32,
    causes: u32,
    tx: VecDeque<u16>,
    rx: VecDeque<u16>,
    pending: Option<(u64, u64, u16, u32)>,
    generation: u64,
    /// DMA request enables (`+0x24`): bit 0 transmit, bit 1 receive.
    requests: u32,
    /// Running DMA exchange: its start tick and the tick its last word has
    /// shifted.
    exchange: Option<(u64, u64)>,
}
impl Spi {
    /// Forward `Input::set_input` to the part behind this controller.
    pub fn set_input(&mut self, register: u8, value: u16) -> Result<bool, String> {
        self.input.set_input(register, value)
    }
    pub fn new(base: u64, input: Box<dyn Input>) -> Self {
        Self {
            base,
            input,
            control: 0,
            enable: 0,
            divider: 0,
            causes: 0,
            tx: VecDeque::new(),
            rx: VecDeque::new(),
            pending: None,
            generation: 0,
            requests: 0,
            exchange: None,
        }
    }
    /// Ticks one word takes to shift: word bits times the divider times one
    /// more than the prescaler (`+0x0` bits 8..15).
    fn word_ticks(&self) -> u64 {
        u64::from(((self.control & 15) + 1) * self.divider * ((self.control >> 8 & 255) + 1))
    }
    fn mode(&self) -> u32 {
        let polarity = self.control >> 6 & 1;
        let phase = self.control >> 7 & 1;
        (polarity << 1) | phase
    }
    /// The data register: a DMA channel whose fixed side is this address moves
    /// words through the controller.
    pub fn data_register(&self) -> u64 {
        self.base + 8
    }
    /// DMA request enables (`+0x24`): bit 0 transmit, bit 1 receive.
    pub fn dma_requests(&self) -> u32 {
        self.requests
    }
    /// Refuses while an exchange runs or programmed-I/O words are queued or
    /// shifting.
    pub fn dma_check_idle(&self) -> Result<(), String> {
        if self.exchange.is_some() {
            return Err("XP4 SPI DMA exchange already running on this controller".into());
        }
        if self.pending.is_some() || !self.tx.is_empty() {
            return Err("XP4 SPI DMA while the controller is shifting programmed-I/O words".into());
        }
        Ok(())
    }
    /// Starts a full-duplex DMA exchange of `bytes` bytes in transfers of
    /// `width` bytes at `start`. Each transfer is one word; the last word has
    /// shifted after `bytes / width` word times. Changes nothing on refusal.
    pub fn dma_begin(&mut self, start: u64, bytes: u64, width: u64) -> Result<DmaExchange, String> {
        self.dma_check_idle()?;
        if self.requests != 3 {
            return Err("XP4 SPI DMA exchange needs both request enables (+0x24 = 3)".into());
        }
        if self.enable & 2 == 0 {
            return Err(
                "XP4 SPI DMA exchange while the controller is stopped (+0x4 bit 1 clear)".into(),
            );
        }
        if self.control & 15 < 3 || !(2..=254).contains(&self.divider) || self.divider & 1 != 0 {
            return Err("XP4 SPI word length/divider is not configured".into());
        }
        let bits = (self.control & 15) + 1;
        if width != u64::from(bits.div_ceil(8)) {
            return Err(format!(
                "XP4 SPI DMA transfer of {width} bytes is not one {bits}-bit word"
            ));
        }
        if bytes == 0 || !bytes.is_multiple_of(width) {
            return Err("XP4 SPI DMA length is not whole words".into());
        }
        if !self.rx.is_empty() {
            return Err("XP4 SPI DMA exchange with words left in the receive FIFO".into());
        }
        let word_ticks = self.word_ticks();
        let end = (bytes / width)
            .checked_mul(word_ticks)
            .and_then(|ticks| start.checked_add(ticks))
            .ok_or("XP4 SPI DMA exchange deadline overflow")?;
        self.exchange = Some((start, end));
        Ok(DmaExchange {
            bits,
            word_ticks,
            end,
        })
    }
    /// Shifts the next word of the running exchange, which has shifted at
    /// `tick`, through the input peer (or the loopback, `+0x4` bit 0) and
    /// returns the received word. Like a programmed-I/O word, it sets cause
    /// bit 0.
    pub fn dma_shift(&mut self, tx: u16, tick: u64) -> Result<u16, String> {
        let Some((start, end)) = self.exchange else {
            return Err("XP4 SPI DMA word outside an exchange".into());
        };
        if tick <= start || tick > end {
            return Err("XP4 SPI DMA word tick outside its exchange".into());
        }
        let bits = (self.control & 15) + 1;
        let mask = u16::MAX >> (16 - bits);
        let rx = if self.enable & 1 != 0 {
            tx & mask
        } else {
            self.input
                .exchange_in_mode(bits, self.mode(), tx & mask, tick)?
                & mask
        };
        self.causes |= 1;
        Ok(rx)
    }
    /// Ends the exchange started at `start` whose last word shifted at `end`;
    /// busy (status bit 4) drops.
    pub fn dma_finish(&mut self, start: u64, end: u64) -> Result<(), String> {
        if self.exchange != Some((start, end)) {
            return Err(
                "XP4 SPI DMA exchange differs between the DMA engine and the controller".into(),
            );
        }
        self.exchange = None;
        Ok(())
    }
    fn start(&mut self, cx: &mut Cx<'_>) {
        self.start_at(cx, cx.now());
    }
    fn start_at(&mut self, cx: &mut Cx<'_>, now: u64) {
        if self.pending.is_some() || self.enable & 2 == 0 {
            return;
        }
        let Some(word) = self.tx.pop_front() else {
            return;
        };
        let bits = (self.control & 15) + 1;
        self.generation += 1;
        let at = now + self.word_ticks();
        self.pending = Some((at, self.generation, word, bits));
        cx.schedule(at, self.generation);
    }
    fn status(&self) -> u32 {
        u32::from(self.tx.is_empty())
            | (u32::from(self.tx.len() < 8) << 1)
            | (u32::from(!self.rx.is_empty()) << 2)
            | (u32::from(self.rx.len() == 8) << 3)
            | (u32::from(self.pending.is_some() || self.exchange.is_some()) << 4)
    }
    fn access(
        &mut self,
        cx: &mut Cx<'_>,
        address: u64,
        size: u8,
        write: bool,
        value: u32,
    ) -> Result<u64, String> {
        let offset = address.wrapping_sub(self.base);
        if size != 4 || !matches!(offset, 0 | 4 | 8 | 0xc | 0x10 | 0x14 | 0x20 | 0x24) {
            cx.unmodeled();
            return Ok(0);
        }
        if !write {
            return Ok(match offset {
                0 => self.control,
                4 => self.enable,
                8 => {
                    if self.exchange.is_some() {
                        return Err(
                            "XP4 SPI data register read by the CPU during a DMA exchange".into(),
                        );
                    }
                    if self.requests & 2 != 0 {
                        return Err("XP4 SPI programmed-I/O data read while the receive DMA request is enabled (+0x24 bit 1) is not modeled".into());
                    }
                    u32::from(
                        self.rx
                            .pop_front()
                            .ok_or("XP4 SPI receive FIFO underflow")?,
                    )
                }
                0xc => self.status(),
                0x10 => self.divider,
                0x14 => self.causes,
                0x24 => self.requests,
                _ => 0,
            } as u64);
        }
        if self.exchange.is_some() {
            // A DMA exchange owns the shifter until its last word has shifted.
            let refusal = match offset {
                0 => Some("XP4 SPI format change during a DMA exchange"),
                4 if value != self.enable => Some("XP4 SPI enable change during a DMA exchange"),
                8 => Some("XP4 SPI data register written by the CPU during a DMA exchange"),
                0x10 => Some("XP4 SPI clock change during a DMA exchange"),
                0x20 => Some("XP4 SPI FIFO clear during a DMA exchange"),
                0x24 if value != self.requests => {
                    Some("XP4 SPI request enable change during a DMA exchange")
                }
                _ => None,
            };
            if let Some(refusal) = refusal {
                return Err(refusal.into());
            }
        }
        match offset {
            0 => {
                if self.pending.is_some() || value & !0xffff != 0 || value & 0x30 != 0 {
                    return Err("Unsupported XP4 SPI format or active reconfiguration".into());
                }
                self.control = value;
            }
            4 => {
                if value & !0xb != 0 {
                    return Err("XP4 slave SPI is not modeled".into());
                }
                if self.pending.is_some() && value & 2 != 0 && value != self.enable {
                    return Err("XP4 SPI mode change while shifting".into());
                }
                self.enable = value;
                if value & 2 == 0 {
                    self.pending = None;
                    self.tx.clear();
                    self.generation += 1;
                } else {
                    self.start(cx);
                }
            }
            8 => {
                if self.requests != 0 {
                    return Err("XP4 SPI programmed-I/O data write while DMA requests are enabled (+0x24) is not modeled".into());
                }
                if self.control & 15 < 3
                    || !(2..=254).contains(&self.divider)
                    || self.divider & 1 != 0
                {
                    return Err("XP4 SPI word length/divider is not configured".into());
                }
                if self.tx.len() == 8 {
                    return Err("XP4 SPI transmit FIFO overflow".into());
                }
                self.tx.push_back(value as u16);
                self.start(cx);
            }
            0xc => return Err("Write to XP4 SPI status".into()),
            0x10 => {
                if self.pending.is_some() {
                    return Err("XP4 SPI clock change while shifting".into());
                }
                self.divider = value;
            }
            0x14 => self.causes &= !value,
            0x20 => {
                if value & !3 != 0 {
                    return Err("Unknown XP4 SPI FIFO clear bits".into());
                }
                if value & 1 != 0 {
                    self.tx.clear();
                }
                if value & 2 != 0 {
                    self.rx.clear();
                }
            }
            // Bit 0 enables the transmit DMA request, bit 1 the receive one.
            0x24 if value & !3 != 0 => {
                return Err("XP4 SPI request enable +0x24 is unsupported".into());
            }
            0x24 => self.requests = value,
            _ => {}
        }
        Ok(0)
    }
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SpiState<T> {
    base: u64,
    input: T,
    control: u32,
    enable: u32,
    divider: u32,
    causes: u32,
    tx: VecDeque<u16>,
    rx: VecDeque<u16>,
    pending: Option<(u64, u64, u16, u32)>,
    generation: u64,
    /// Absent in checkpoints taken while no DMA request was enabled.
    #[serde(default, skip_serializing_if = "is_zero")]
    requests: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    exchange: Option<(u64, u64)>,
}
fn is_zero(value: &u32) -> bool {
    *value == 0
}

impl Spi {
    fn capture<T>(&self, input: T) -> SpiState<T> {
        SpiState {
            base: self.base,
            input,
            control: self.control,
            enable: self.enable,
            divider: self.divider,
            causes: self.causes,
            tx: self.tx.clone(),
            rx: self.rx.clone(),
            pending: self.pending,
            generation: self.generation,
            requests: self.requests,
            exchange: self.exchange,
        }
    }

    fn validate<T>(&self, state: &SpiState<T>) -> Result<(), String> {
        if state.base != self.base
            || state.control & !0xffcf != 0
            || state.enable & !0xb != 0
            || state.tx.len() > 8
            || state.rx.len() > 8
            || state.causes & !1 != 0
            || state.pending.is_some_and(|(_, token, _, bits)| {
                token != state.generation
                    || state.enable & 2 == 0
                    || state.divider == 0
                    || bits != (state.control & 15) + 1
            })
            || state.requests & !3 != 0
            // An exchange needs both requests, a running and configured
            // shifter, and owns it: no programmed-I/O word may be around.
            || state.exchange.is_some_and(|(start, end)| {
                end <= start
                    || state.requests != 3
                    || state.enable & 2 == 0
                    || state.pending.is_some()
                    || !state.tx.is_empty()
                    || !state.rx.is_empty()
                    || state.control & 15 < 3
                    || !(2..=254).contains(&state.divider)
                    || state.divider & 1 != 0
            })
        {
            return Err("Invalid XP4 SPI checkpoint or base".into());
        }
        Ok(())
    }

    fn apply<T>(&mut self, state: SpiState<T>) -> Result<(), String> {
        self.control = state.control;
        self.enable = state.enable;
        self.divider = state.divider;
        self.causes = state.causes;
        self.tx = state.tx;
        self.rx = state.rx;
        self.pending = state.pending;
        self.generation = state.generation;
        self.requests = state.requests;
        self.exchange = state.exchange;
        Ok(())
    }
}

impl Device for Spi {
    fn future_deadline_is_stable(&self) -> bool {
        true
    }
    fn name(&self) -> &'static str {
        match self.base {
            0xfff32000 => "xp4-spi0",
            0xfff33000 => "xp4-spi1",
            0xfff34000 => "xp4-spi2",
            0xfff35000 => "xp4-spi3",
            0xfff36000 => "xp4-spi4",
            0xfff37000 => "xp4-spi5",
            0xfff38000 => "xp4-spi6",
            _ => "xp4-spi",
        }
    }
    fn projected_value_until(
        &self,
        address: u64,
        size: u8,
    ) -> Option<fujimod_emulator_devices::bus::ReadValidity> {
        if size != 4 || address.wrapping_sub(self.base) != 0xc {
            return None;
        }
        Some(fujimod_emulator_devices::bus::ReadValidity {
            before: self
                .exchange
                .map_or(u64::MAX, |(_, end)| end)
                .min(self.pending.map_or(u64::MAX, |pending| pending.0)),
            countdown_expiry: None,
        })
    }
    fn read_without_advance(&self, address: u64, size: u8, now: u64) -> Option<u64> {
        (size == 4
            && address.wrapping_sub(self.base) == 0xc
            && self.exchange.is_none_or(|(_, end)| end > now)
            && self.pending.is_none_or(|pending| pending.0 > now))
        .then(|| u64::from(self.status()))
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        match self.access(cx, address, size, false, 0) {
            Ok(v) => v,
            Err(e) => {
                cx.fail(e);
                0
            }
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        if let Err(e) = self.access(cx, address, size, true, value as u32) {
            cx.fail(e);
        }
    }
    /// Control, enable, status, divider and cause words read stored state;
    /// the receive FIFO word pops and is not stable. During a DMA exchange
    /// status busy drops at the DMA engine's completion, not at a deadline
    /// of this controller, so status is not stable then.
    fn read_stable(&self, address: u64, size: u8) -> bool {
        size == 4
            && match address.wrapping_sub(self.base) {
                0 | 4 | 0x10 | 0x14 => true,
                0xc => self.exchange.is_none(),
                _ => false,
            }
    }
    fn next_deadline(&self, _now: u64) -> Option<u64> {
        self.pending.map(|p| p.0)
    }
    fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
        let Some((at, generation, tx, bits)) = self.pending else {
            return;
        };
        if generation != token || cx.now() < at {
            return;
        }
        self.pending = None;
        if self.rx.len() == 8 {
            cx.fail("XP4 SPI receive FIFO overflow");
            return;
        }
        let mask = u16::MAX >> (16 - bits);
        let rx = if self.enable & 1 != 0 {
            tx & mask
        } else {
            match self
                .input
                .exchange_in_mode(bits, self.mode(), tx & mask, at)
            {
                Ok(value) => value & mask,
                Err(error) => {
                    cx.fail(error);
                    return;
                }
            }
        };
        self.rx.push_back(rx);
        self.causes |= 1;
        cx.record(Record::new(
            "XP4 SPI word complete",
            [
                ("base", Field::int(self.base)),
                ("tx", Field::int(tx & mask)),
                ("rx", Field::int(rx)),
                ("bits", Field::int(bits)),
                ("tick", Field::int(at)),
            ],
        ));
        self.start_at(cx, at);
    }
    fn save(&self) -> Value {
        let mut state = json!({"base":self.base,"input":self.input.save(),"control":self.control,"enable":self.enable,
            "divider":self.divider,"causes":self.causes,"tx":self.tx,"rx":self.rx,"pending":self.pending,
            "generation":self.generation});
        if self.requests != 0 {
            state["requests"] = json!(self.requests);
        }
        if let Some(exchange) = self.exchange {
            state["exchange"] = json!(exchange);
        }
        state
    }
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        Ok(Checkpoint::encode(self.name(), &self.capture(()))?
            .with_children(vec![self.input.checkpoint()?]))
    }
    fn validate_checkpoint(&self, data: &Checkpoint) -> Result<(), String> {
        let state: SpiState<()> = data.decode(self.name())?;
        self.validate(&state)?;
        let [input] = data.children() else {
            return Err("XP4 SPI checkpoint requires one input".into());
        };
        self.input.validate_checkpoint(input)
    }
    fn restore_checkpoint(&mut self, data: &Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(data)?;
        let state: SpiState<()> = data.decode(self.name())?;
        self.input.restore_checkpoint(&data.children()[0])?;
        self.apply(state)
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: SpiState<Value> =
            serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        self.validate(&state)?;
        self.input.restore(&state.input)?;
        self.apply(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::Host;
    #[derive(Default)]
    struct Clock {
        now: u64,
        records: Vec<Record>,
    }
    impl Host for Clock {
        fn now(&self) -> u64 {
            self.now
        }
        fn schedule(&mut self, _: usize, _: u64, _: u64) {}
        fn set_irq(&mut self, _: u32, _: bool) {
            panic!("polling SPI has no IRQ")
        }
        fn raise_irq(&mut self, _: usize, _: u32) -> Result<(), String> {
            unreachable!()
        }
        fn ram_read(&self, _: u64, _: &mut [u8]) -> Result<(), String> {
            unreachable!()
        }
        fn ram_write(&mut self, _: u64, _: &[u8]) -> Result<(), String> {
            unreachable!()
        }
        fn record(&mut self, r: Record) {
            self.records.push(r);
        }
    }
    fn trace(pc: u64, pin: bool) -> (Value, Vec<Record>) {
        let mut m = Spi::new(0x1000, Box::new(FixedPin(pin)));
        let mut h = Clock::default();
        for (o, v) in [(0, 0xc7), (0x10, 2), (4, 0xa), (8, 0x12), (8, 0x34)] {
            let mut cx = Cx::new(&mut h, 3, pc, 0);
            m.write(&mut cx, 0x1000 + o, 4, v);
            assert_eq!(cx.refusal(), None);
        }
        assert_eq!(m.next_deadline(0), Some(16));
        let before = m.save();
        h.now = 15;
        m.event(&mut Cx::new(&mut h, 1, pc, 0), 1);
        assert_eq!(m.save(), before);
        h.now = 16;
        m.event(&mut Cx::new(&mut h, 1, pc, 0), 1);
        assert_eq!(m.next_deadline(16), Some(32));
        assert_eq!(m.rx.front(), Some(&(if pin { 255 } else { 0 })));
        let before = m.save();
        assert_eq!(m.save(), before);
        assert_eq!(
            m.read(&mut Cx::new(&mut h, 0, pc, 0), 0x1008, 4),
            if pin { 255 } else { 0 }
        );
        assert!(m.rx.is_empty());
        // Disabling aborts the queued shift; its old scheduled token is inert.
        m.write(&mut Cx::new(&mut h, 0, pc, 0), 0x1004, 4, 8);
        h.now = 32;
        m.event(&mut Cx::new(&mut h, 0, pc, 0), 2);
        assert!(m.rx.is_empty());
        assert_eq!(m.next_deadline(32), None);
        assert_eq!(h.records.len(), 1);
        (m.save(), h.records)
    }
    #[test]
    fn pin_sampling_fifo_deadlines_and_cancel_ignore_program_state() {
        for pin in [false, true] {
            assert_eq!(trace(0x2000, pin), trace(0x123456, pin));
        }
    }
    #[test]
    fn status_projection_preserves_fifo_state_and_declines_at_the_edge() {
        let mut device = Spi::new(0x1000, Box::new(FixedPin(false)));
        let mut host = Clock::default();
        write_all(
            &mut device,
            &mut host,
            &[(0, 0xc7), (0x10, 2), (4, 0xa), (8, 0x12)],
        );
        let deadline = device.next_deadline(0).unwrap();
        assert_eq!(
            device
                .projected_value_until(0x100c, 4)
                .map(|read| read.before),
            Some(deadline)
        );
        assert_eq!(
            device
                .projected_value_until(0x1008, 4)
                .map(|read| read.before),
            None
        );
        let before = device.save();
        for now in [0, deadline - 1] {
            host.now = now;
            assert_eq!(
                device.read_without_advance(0x100c, 4, now),
                Some(device.read(&mut Cx::new(&mut host, 0, 0, 0), 0x100c, 4))
            );
        }
        assert_eq!(device.save(), before);
        assert_eq!(device.read_without_advance(0x1008, 4, 0), None);
        assert_eq!(device.read_without_advance(0x100c, 2, 0), None);
        assert_eq!(device.read_without_advance(0x100c, 4, deadline), None);

        let (mut dma, _) = linked(false);
        let transfer = dma.dma_begin(100, 2, 1).unwrap();
        assert_eq!(
            dma.projected_value_until(0x100c, 4).map(|read| read.before),
            Some(transfer.end)
        );
        let before = dma.save();
        assert_eq!(dma.read_without_advance(0x100c, 4, 100), Some(0x13));
        assert_eq!(dma.read_without_advance(0x100c, 4, transfer.end), None);
        assert_eq!(dma.save(), before);
    }
    #[test]
    fn pending_fifo_words_keep_their_edges_after_restore() {
        let mut source = Spi::new(0x1000, Box::new(FixedPin(true)));
        let mut clock = Clock::default();
        for (offset, value) in [(0, 0xc7), (0x10, 2), (4, 0xa), (8, 0x12), (8, 0x34)] {
            source.write(&mut Cx::new(&mut clock, 0, 0, 0), 0x1000 + offset, 4, value);
        }
        let mut resumed = Spi::new(0x1000, Box::new(FixedPin(true)));
        resumed.restore(&source.save()).unwrap();
        for (now, token) in [(15, 1), (16, 1), (31, 2), (32, 2)] {
            clock.now = now;
            source.event(&mut Cx::new(&mut clock, 0, 0x1000, 0), token);
            resumed.event(&mut Cx::new(&mut clock, 3, 0xdead0000, 0), token);
            assert_eq!(source.save(), resumed.save());
        }
        assert_eq!(source.rx, [255, 255]);
    }
    #[test]
    fn queued_words_keep_their_clock_edges_when_events_are_delivered_late() {
        let mut outcomes = vec![];
        for calls in [[16, 32], [64, 64]] {
            let mut m = Spi::new(0x1000, Box::new(FixedPin(false)));
            let mut h = Clock::default();
            for (o, v) in [(0, 0xc7), (0x10, 2), (4, 0xb), (8, 0x12), (8, 0x34)] {
                m.write(&mut Cx::new(&mut h, 0, 0, 0), 0x1000 + o, 4, v);
            }
            for (i, now) in calls.into_iter().enumerate() {
                h.now = now;
                m.event(&mut Cx::new(&mut h, 0, 0, 0), i as u64 + 1);
            }
            assert_eq!(m.rx, [0x12, 0x34]);
            assert_eq!(m.next_deadline(h.now), None);
            outcomes.push((m.save(), h.records));
        }
        assert_eq!(outcomes[0], outcomes[1]);
    }
    fn write_all(m: &mut Spi, h: &mut Clock, writes: &[(u64, u64)]) {
        for &(offset, value) in writes {
            let mut cx = Cx::new(h, 0, 0, 0);
            m.write(&mut cx, 0x1000 + offset, 4, value);
            assert_eq!(cx.refusal(), None, "{offset:#x} = {value:#x}");
        }
    }
    /// The refusal of one write (`Some(value)`) or read (`None`).
    fn refusal(m: &mut Spi, h: &mut Clock, offset: u64, value: Option<u64>) -> Option<String> {
        let mut cx = Cx::new(h, 0, 0, 0);
        match value {
            Some(value) => m.write(&mut cx, 0x1000 + offset, 4, value),
            None => {
                m.read(&mut cx, 0x1000 + offset, 4);
            }
        }
        cx.refusal().map(str::to_owned)
    }
    /// The link as a driver opens it: 8-bit words, divider 0x24, run, both
    /// DMA requests, causes cleared.
    fn linked(pin: bool) -> (Spi, Clock) {
        let mut m = Spi::new(0x1000, Box::new(FixedPin(pin)));
        let mut h = Clock::default();
        let writes = [
            (0, 0xc7),
            (4, 8),
            (0x10, 0x24),
            (4, 0xa),
            (0x24, 3),
            (0x14, 0xf),
        ];
        write_all(&mut m, &mut h, &writes);
        (m, h)
    }
    #[test]
    fn request_enables_accept_bits_0_and_1_and_read_back() {
        let mut m = Spi::new(0x1000, Box::new(FixedPin(false)));
        let mut h = Clock::default();
        for value in 0..4 {
            assert_eq!(refusal(&mut m, &mut h, 0x24, Some(value)), None);
            assert_eq!(m.read(&mut Cx::new(&mut h, 0, 0, 0), 0x1024, 4), value);
            assert_eq!(m.dma_requests(), value as u32);
        }
        for value in [4, 8, 0x100] {
            assert_eq!(
                refusal(&mut m, &mut h, 0x24, Some(value)).as_deref(),
                Some("XP4 SPI request enable +0x24 is unsupported")
            );
        }
        // Without requests the controller saves exactly as before.
        write_all(&mut m, &mut h, &[(0x24, 0)]);
        let saved = m.save();
        assert!(saved.get("requests").is_none() && saved.get("exchange").is_none());
        assert_eq!(m.data_register(), 0x1008);
    }
    #[test]
    fn programmed_io_data_accesses_refuse_while_requests_are_enabled() {
        let (mut m, mut h) = linked(false);
        let write = refusal(&mut m, &mut h, 8, Some(0x5a)).unwrap();
        assert!(write.contains("DMA requests are enabled"), "{write}");
        let read = refusal(&mut m, &mut h, 8, None).unwrap();
        assert!(read.contains("receive DMA request is enabled"), "{read}");
        write_all(&mut m, &mut h, &[(0x24, 0), (8, 0x5a)]);
        assert_eq!(m.next_deadline(0), Some(8 * 0x24));
    }
    #[test]
    fn a_dma_exchange_shifts_words_through_the_input_and_holds_busy() {
        for (pin, expected) in [(false, 0), (true, 0xff)] {
            let (mut m, mut h) = linked(pin);
            h.now = 100;
            let x = m.dma_begin(100, 4, 1).unwrap();
            assert_eq!(
                x,
                DmaExchange {
                    bits: 8,
                    word_ticks: 8 * 0x24,
                    end: 100 + 4 * 8 * 0x24
                }
            );
            assert_eq!(
                m.read(&mut Cx::new(&mut h, 0, 0, 0), 0x100c, 4) & 0x10,
                0x10
            );
            assert!(!m.read_stable(0x100c, 4));
            assert_eq!(
                m.next_deadline(100),
                None,
                "the DMA engine owns the deadline"
            );
            for (offset, value, text) in [
                (8, Some(1), "written by the CPU"),
                (8, None, "read by the CPU"),
                (0, Some(0xc7), "format change"),
                (4, Some(8), "enable change"),
                (0x10, Some(0x24), "clock change"),
                (0x20, Some(3), "FIFO clear"),
                (0x24, Some(0), "request enable change"),
            ] {
                let error = refusal(&mut m, &mut h, offset, value).unwrap();
                assert!(error.contains(text), "{offset:#x}: {error}");
            }
            // Unchanged enables and a cause clear are not reconfiguration.
            write_all(&mut m, &mut h, &[(4, 0xa), (0x24, 3), (0x14, 1)]);
            assert!(m.dma_check_idle().is_err());
            assert!(m.dma_begin(100, 4, 1).is_err());
            for word in 1..=4 {
                assert_eq!(m.dma_shift(0x5a, 100 + word * x.word_ticks), Ok(expected));
            }
            assert!(m.dma_shift(0x5a, x.end + 1).is_err());
            assert!(m.dma_finish(100, x.end + 1).is_err());
            m.dma_finish(100, x.end).unwrap();
            assert_eq!(m.read(&mut Cx::new(&mut h, 0, 0, 0), 0x100c, 4) & 0x10, 0);
            assert!(m.read_stable(0x100c, 4));
            assert_eq!(m.causes, 1);
            assert!(m.dma_shift(0x5a, x.end).is_err());
            assert!(h.records.is_empty());
        }
    }
    #[test]
    fn dma_begin_refuses_without_changing_the_controller() {
        let (mut m, mut h) = linked(false);
        for (bytes, width, text) in [(4, 2, "not one 8-bit word"), (0, 1, "not whole words")] {
            assert!(m.dma_begin(0, bytes, width).unwrap_err().contains(text));
        }
        write_all(&mut m, &mut h, &[(0x24, 1)]);
        assert!(
            m.dma_begin(0, 4, 1)
                .unwrap_err()
                .contains("both request enables")
        );
        write_all(&mut m, &mut h, &[(0x24, 3), (4, 8)]);
        assert!(m.dma_begin(0, 4, 1).unwrap_err().contains("stopped"));
        write_all(&mut m, &mut h, &[(0x24, 0), (4, 0xa), (8, 0x12), (0x24, 3)]);
        assert!(m.dma_begin(0, 4, 1).unwrap_err().contains("programmed-I/O"));
        h.now = 8 * 0x24;
        let token = m.generation;
        m.event(&mut Cx::new(&mut h, 0, 0, 0), token);
        assert!(
            m.dma_begin(h.now, 4, 1)
                .unwrap_err()
                .contains("receive FIFO")
        );
        write_all(&mut m, &mut h, &[(0x20, 2)]);
        assert!(
            m.dma_begin(u64::MAX - 1, 4, 1)
                .unwrap_err()
                .contains("overflow")
        );
        assert!(m.exchange.is_none());
        m.dma_begin(h.now, 4, 1).unwrap();
    }
    #[test]
    fn a_running_exchange_survives_checkpoints_and_impossible_ones_are_refused() {
        let (mut m, _) = linked(true);
        let x = m.dma_begin(100, 4, 1).unwrap();
        let saved = m.save();
        assert_eq!(saved["exchange"], json!([100, x.end]));
        assert_eq!(saved["requests"], 3);
        let checkpoint = m.checkpoint().unwrap();
        let mut from_json = Spi::new(0x1000, Box::new(FixedPin(true)));
        from_json.restore(&saved).unwrap();
        let mut from_checkpoint = Spi::new(0x1000, Box::new(FixedPin(true)));
        from_checkpoint.restore_checkpoint(&checkpoint).unwrap();
        for restored in [&mut from_json, &mut from_checkpoint] {
            assert_eq!(restored.save(), saved);
            assert_eq!(restored.dma_shift(1, 100 + x.word_ticks), Ok(0xff));
        }
        for (field, value) in [
            ("requests", json!(1)),
            ("requests", json!(7)),
            ("tx", json!([1])),
            ("enable", json!(8)),
            ("exchange", json!([100, 100])),
        ] {
            let mut bad = saved.clone();
            bad[field] = value;
            let mut target = Spi::new(0x1000, Box::new(FixedPin(true)));
            assert!(target.restore(&bad).is_err(), "{field}");
            assert!(target.exchange.is_none());
        }
        // A checkpoint taken without requests carries neither new field and
        // restores into a controller as before.
        let (mut plain, mut h) = linked(true);
        write_all(&mut plain, &mut h, &[(0x24, 0)]);
        let mut restored = Spi::new(0x1000, Box::new(FixedPin(true)));
        restored
            .restore_checkpoint(&plain.checkpoint().unwrap())
            .unwrap();
        assert_eq!(restored.save(), plain.save());
    }
}
