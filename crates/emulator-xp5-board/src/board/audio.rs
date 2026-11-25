//! XP5 audio capture: the audio serial interface (kind `audio-serial`) and the
//! PL330 that moves its samples (kind `audio-dma`).
//!
//! The register contract is the X-T5 4.31 driver's: serial driver
//! 0x1388700..0x1389098 (two instances, `0xff91c000 + id * 0x75000`, so
//! 0xff91c000 and 0xff991000), PL330 driver 0x139ad80..0x139b6c8 at 0xff938000
//! (driver channels 0x10..0x17 are its threads 0..7; 0x138babc class 1), and
//! the `audio` task 0x148c000..0x1492000.
//!
//! Movie audio records through instance 0: the task opens it at 0x1491420 and
//! starts its receive DMA at 0x1491a8c with descriptor 0xa (0x18e9660 + 0xa *
//! 0x14 = {channel 0x10, request 0x40, class 1, burst 16}), that is PL330
//! thread 0 on peripheral request 0. The program (generator 0x1396af8 for
//! 16-bit, 0x1398168 for 24-bit) is a ring of two buffers of 240 frames each:
//! `DMALDPS` from the receive FIFO at +0x00, `DMAST` into the buffer, then
//! `DMAWFE 1` (the CPU's permit, `DMASEV 1` from the manager at 0x139b280) and
//! `DMASEV 0` (INTID 253, the buffer-done callback 0x1490fbc).
//!
//! Serial registers (offsets from the instance base):
//!
//! - `+0x00` receive FIFO, `+0x04` transmit FIFO. The PL330 reads and writes
//!   them through [`Fifo`]. Each receive word is one sample of one slot.
//! - `+0x08` control, `+0x0c` format (bits 4..0: word length - 1; 15, 23 or
//!   31), `+0x10` slot enable mask (3 = stereo, 0xf = four slots), `+0x14`,
//!   `+0x20` interrupt mask and `+0x28` DMA enable are plain words.
//! - `+0x18` enable (bit 0 starts the interface) is a plain word.
//! - `+0x1c` soft reset: the driver writes 1 and polls bit 0 until it reads 0.
//!   The reset completes at once (a model assumption).
//! - `+0x24` status: 0x1388e00 polls it after enabling until bit 16 or bit 17
//!   is set, with no timeout. Bit 16 reads 1 while `+0x18` bit 0 is set.
//!
//! The receive data is a host test signal: silence, or one sine tone at
//! `tone_hz` in every slot. It is paced at `sample_rate_hz`: a PL330 transfer
//! through the FIFO takes the time its frames take at that rate, so buffer-done
//! interrupts arrive at the audio rate (see [`pl330`]). The sample rate is a
//! declared option: the serial clock dividers are not decoded. Transmit words
//! are discarded.
use fujimod_emulator_devices::checkpoint::Checkpoint;
use fujimod_emulator_devices::graph::{DeviceGraph, DeviceWindow};
use fujimod_emulator_devices::{Cx, Device};
use fujimod_emulator_ip_arm::pl330;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, Mutex};

const SERIAL_SIZE: u64 = 0x1000;
const SERIAL_BASES: [u64; 2] = [0xff91c000, 0xff991000];
const DMA_BASE: u64 = 0xff938000;
const DMA_SIZE: u64 = 0x1000;
/// XP5 board time is in nanoseconds.
const TICKS_PER_SECOND: u64 = 1_000_000_000;
const RESET: u64 = 0x1c;
const ENABLE: u64 = 0x18;
const STATUS: u64 = 0x24;
const FORMAT: u64 = 0x0c;
const SLOTS: u64 = 0x10;
/// Plain configuration words: control, format, slot mask, 0x14, enable,
/// interrupt mask and DMA enable.
const WORDS: [u64; 7] = [0x08, 0x0c, 0x10, 0x14, 0x18, 0x20, 0x28];

pub fn owns(kind: &str) -> bool {
    matches!(kind, "audio-serial" | "audio-dma")
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Signal {
    pub sample_rate_hz: u32,
    /// 0 is silence.
    #[serde(default)]
    pub tone_hz: u32,
    /// Peak level as a fraction of full scale.
    #[serde(default)]
    pub tone_amplitude: f64,
}

impl Signal {
    fn check(&self) -> Result<(), String> {
        if !(8000..=192_000).contains(&self.sample_rate_hz)
            || self.tone_hz >= self.sample_rate_hz / 2
            || !(0.0..=1.0).contains(&self.tone_amplitude)
        {
            return Err("invalid audio-serial signal".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct State {
    words: [u32; WORDS.len()],
    /// Receive words delivered since the first enable.
    delivered: u64,
}

struct Inner {
    signal: Signal,
    state: State,
}

impl Inner {
    fn word(&self, offset: u64) -> u32 {
        WORDS
            .iter()
            .position(|o| *o == offset)
            .map_or(0, |i| self.state.words[i])
    }
    fn bits(&self) -> u32 {
        (self.word(FORMAT) & 0x1f) + 1
    }
    /// FIFO bytes per sample word: 16-bit and narrower words are halfwords.
    fn word_bytes(&self) -> u64 {
        if self.bits() <= 16 { 2 } else { 4 }
    }
    fn slots(&self) -> u64 {
        match (self.word(SLOTS) & 0xff).count_ones() {
            0 => 2,
            n => u64::from(n),
        }
    }
    /// The next receive word, little-endian, `word_bytes` long.
    fn next_word(&mut self) -> Vec<u8> {
        let index = self.state.delivered;
        self.state.delivered += 1;
        let frame = index / self.slots();
        let bits = self.bits().clamp(8, 32);
        let full = ((1u64 << (bits - 1)) - 1) as f64;
        let s = &self.signal;
        let value = if s.tone_hz == 0 || s.tone_amplitude == 0.0 {
            0
        } else {
            // The phase wraps per tone period so it stays exact over long runs.
            let period = u64::from(s.sample_rate_hz);
            let phase = (frame * u64::from(s.tone_hz)) % period;
            let angle = std::f64::consts::TAU * phase as f64 / period as f64;
            (angle.sin() * s.tone_amplitude * full).round() as i64
        };
        match self.word_bytes() {
            2 => (value as i16).to_le_bytes().to_vec(),
            _ => (value as i32).to_le_bytes().to_vec(),
        }
    }
    fn ticks(&self, bytes: u64) -> u64 {
        let per_second = u128::from(self.signal.sample_rate_hz)
            * u128::from(self.slots())
            * u128::from(self.word_bytes());
        (u128::from(bytes) * u128::from(TICKS_PER_SECOND) / per_second) as u64
    }
}

pub struct AudioSerial {
    base: u64,
    inner: Arc<Mutex<Inner>>,
}

impl AudioSerial {
    pub fn new(base: u64, signal: Signal) -> Result<Self, String> {
        signal.check()?;
        Ok(Self {
            base,
            inner: Arc::new(Mutex::new(Inner {
                signal,
                state: State::default(),
            })),
        })
    }
    /// The FIFO window the PL330 transfers through.
    pub fn fifo(&self) -> Arc<dyn pl330::Source> {
        Arc::new(Fifo {
            base: self.base,
            inner: self.inner.clone(),
        })
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Device for AudioSerial {
    fn name(&self) -> &'static str {
        "audio-serial"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let offset = address - self.base;
        let mut inner = self.lock();
        match (offset, size) {
            (0, 2 | 4) => {
                let word = inner.next_word();
                let mut value = [0u8; 8];
                value[..word.len()].copy_from_slice(&word);
                u64::from_le_bytes(value) & if size == 2 { 0xffff } else { 0xffff_ffff }
            }
            (RESET, 4) => 0,
            (STATUS, 4) => u64::from(inner.word(ENABLE) & 1) << 16,
            (o, 4) if WORDS.contains(&o) => u64::from(inner.word(o)),
            _ => {
                cx.unmodeled();
                0
            }
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let offset = address - self.base;
        let mut inner = self.lock();
        match (offset, size, u32::try_from(value)) {
            (4, 2 | 4, _) => {}
            (RESET, 4, Ok(1)) => {}
            (o, 4, Ok(v)) if WORDS.contains(&o) => {
                let i = WORDS.iter().position(|w| *w == o).unwrap_or(0);
                inner.state.words[i] = v;
            }
            _ => cx.unmodeled(),
        }
    }
    fn pollable(&self, address: u64, _size: u8) -> bool {
        matches!(address - self.base, RESET | STATUS)
    }
    fn save(&self) -> Value {
        serde_json::to_value(&self.lock().state).unwrap_or(Value::Null)
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        self.lock().state = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        Ok(())
    }
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        let inner = self.lock();
        Checkpoint::encode(self.name(), &(inner.signal, &inner.state))
    }
    fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        if !saved.children().is_empty() || !saved.blobs().is_empty() {
            return Err("audio-serial snapshot has unexpected parts".into());
        }
        let (signal, _): (Signal, State) = saved.decode(self.name())?;
        if signal != self.lock().signal {
            return Err("audio-serial snapshot signal differs".into());
        }
        Ok(())
    }
    fn restore_checkpoint(&mut self, saved: &Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        let (_, state): (Signal, State) = saved.decode(self.name())?;
        self.lock().state = state;
        Ok(())
    }
    fn fingerprint(&self) -> Result<Value, String> {
        Ok(self.checkpoint()?.state())
    }
}

/// The receive (+0x00) and transmit (+0x04) FIFO words as a PL330 window.
struct Fifo {
    base: u64,
    inner: Arc<Mutex<Inner>>,
}

impl pl330::Source for Fifo {
    fn contains(&self, address: u64, len: usize) -> bool {
        address >= self.base && address + len as u64 <= self.base + 8
    }
    fn read(&self, address: u64, len: usize) -> Result<Vec<u8>, String> {
        if address != self.base {
            return Err(format!("audio-serial: DMA load from {address:#x}"));
        }
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut out = Vec::with_capacity(len);
        while out.len() < len {
            out.extend(inner.next_word());
        }
        out.truncate(len);
        Ok(out)
    }
    fn writable(&self) -> bool {
        true
    }
    fn write(&self, address: u64, _bytes: &[u8]) -> Result<(), String> {
        if address != self.base + 4 {
            return Err(format!("audio-serial: DMA store to {address:#x}"));
        }
        Ok(())
    }
    fn transfer_ticks(&self, bytes: u64) -> Option<u64> {
        Some(self.inner.lock().unwrap_or_else(|e| e.into_inner()).ticks(bytes))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DmaWiring {
    /// GIC INTID of event 0; events 0..7 use consecutive INTIDs.
    irq_base: u32,
    setup_ticks: u64,
    bytes_per_tick: u64,
}

fn options<T: serde::de::DeserializeOwned>(w: &DeviceWindow) -> Result<T, String> {
    toml::Value::Table(w.options.clone())
        .try_into()
        .map_err(|e| format!("{} options: {e}", w.kind))
}

pub fn register_all(bus: &mut DeviceGraph, windows: &[DeviceWindow]) -> Result<(), String> {
    let mut fifos = vec![];
    for w in windows.iter().filter(|w| w.kind == "audio-serial") {
        if !SERIAL_BASES.contains(&w.base) || w.size != SERIAL_SIZE {
            return Err("invalid audio-serial window".into());
        }
        let serial = AudioSerial::new(w.base, options(w)?)?;
        fifos.push(serial.fifo());
        bus.register(w.base, w.size, Box::new(serial))?;
    }
    let mut dmas = windows.iter().filter(|w| w.kind == "audio-dma");
    if let Some(w) = dmas.next() {
        if w.base != DMA_BASE || w.size != DMA_SIZE || dmas.next().is_some() {
            return Err("invalid or duplicate audio-dma window".into());
        }
        let wiring: DmaWiring = options(w)?;
        let routes = (0..8).map(|e| wiring.irq_base + e).collect();
        let dma = pl330::Pl330::new(
            w.base,
            routes,
            fifos,
            pl330::Throughput {
                setup_ticks: wiring.setup_ticks,
                bytes_per_tick: wiring.bytes_per_tick,
            },
        )?;
        bus.register(w.base, w.size, Box::new(dma))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::recording::RecordingHost;

    const SERIAL: u64 = 0xff91c000;
    const PROGRAM: u64 = 0x1000;
    const BUFFER_A: u32 = 0x4000;
    const BUFFER_B: u32 = 0x5000;

    fn host() -> RecordingHost {
        RecordingHost {
            ram: vec![0; 0x8000],
            ..RecordingHost::default()
        }
    }
    fn write(d: &mut dyn Device, h: &mut RecordingHost, address: u64, value: u32) {
        let mut cx = Cx::new(h, 0, 0, 0);
        d.write(&mut cx, address, 4, u64::from(value));
        assert_eq!(cx.refusal(), None, "{address:#x}={value:#x}");
    }
    fn read(d: &mut dyn Device, h: &mut RecordingHost, address: u64) -> u32 {
        let mut cx = Cx::new(h, 0, 0, 0);
        let value = d.read(&mut cx, address, 4) as u32;
        assert_eq!(cx.refusal(), None, "{address:#x}");
        value
    }
    fn tone() -> Signal {
        Signal {
            sample_rate_hz: 48000,
            tone_hz: 1000,
            tone_amplitude: 0.5,
        }
    }

    /// The driver's open and enable for stereo receive (0x1388700 with word
    /// selector `bits`, 0x13889e4, 0x1388e00).
    fn open(serial: &mut AudioSerial, h: &mut RecordingHost, word: u32) {
        write(serial, h, SERIAL + 0x1c, 1);
        assert_eq!(read(serial, h, SERIAL + 0x1c), 0);
        write(serial, h, SERIAL + 0x08, 0x1000_2563);
        write(serial, h, SERIAL + 0x0c, 1 << 10 | word);
        write(serial, h, SERIAL + 0x10, 3);
        write(serial, h, SERIAL + 0x14, 0);
        write(serial, h, SERIAL + 0x20, 0x7f3d_0f0f);
        write(serial, h, SERIAL + 0x28, 0x100);
        assert_eq!(read(serial, h, SERIAL + 0x24) & 0x30000, 0);
        write(serial, h, SERIAL + 0x18, 0x0100_0001);
        assert_eq!(read(serial, h, SERIAL + 0x24) & 0x30000, 0x10000);
    }

    /// The program 0x1396af8 writes for 16-bit stereo: two 960-byte buffers,
    /// DMALP 30 { DMAWFPS 0; 8 x (DMALDPS 0; DMALDPS 0; DMAST) }, each followed by
    /// DMAWFE 1; DMASEV 0, then DMALP 1/DMALPEND back to the start.
    fn record_program(ccr: u32, body: &[u8], iterations: u8) -> Vec<u8> {
        let mut p = vec![0xbc, 0];
        p.extend((SERIAL as u32).to_le_bytes());
        for (i, buffer) in [BUFFER_A, BUFFER_B].into_iter().enumerate() {
            p.extend([0xbc, 2]);
            p.extend(buffer.to_le_bytes());
            p.extend([0xbc, 1]);
            p.extend(ccr.to_le_bytes());
            if i == 0 {
                p.extend([0x30, 0, 0x30, 0]);
            }
            let start = p.len() + 2;
            p.extend([0x20, iterations - 1, 0x30, 0]);
            p.extend(body);
            p.push(0x13);
            let back = (p.len() - start) as u8;
            p.extend([0x38, back, 0x36, 1 << 3, 0x34, 0]);
        }
        let back = p.len() as u8 + 2;
        p.extend([0x20, 1, 0x38, back, 0]);
        p
    }

    fn i16s(ram: &[u8]) -> Vec<i16> {
        ram.chunks(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect()
    }

    #[test]
    fn stereo_16_bit_capture_delivers_a_paced_tone_buffer_per_interrupt() {
        let mut h = host();
        let mut serial = AudioSerial::new(SERIAL, tone()).unwrap();
        open(&mut serial, &mut h, 0xf);
        let timing = pl330::Throughput {
            setup_ticks: 1000,
            bytes_per_tick: 1,
        };
        let mut dma =
            pl330::Pl330::new(DMA_BASE, (253..261).collect(), vec![serial.fifo()], timing).unwrap();
        let body: Vec<u8> = (0..8).flat_map(|_| [0x25, 0, 0x25, 0, 0x08]).collect();
        let program = record_program(0x0084_c202, &body, 30);
        h.ram[PROGRAM as usize..PROGRAM as usize + program.len()].copy_from_slice(&program);
        // 0x139b13c: INTEN bit 0, DMAGO thread 0; 0x139b280: INTCLR, DMASEV 1.
        write(&mut dma, &mut h, DMA_BASE + 0x20, 1);
        write(&mut dma, &mut h, DMA_BASE + 0xd08, 0xa2_0000);
        write(&mut dma, &mut h, DMA_BASE + 0xd0c, PROGRAM as u32);
        write(&mut dma, &mut h, DMA_BASE + 0xd04, 0);
        write(&mut dma, &mut h, DMA_BASE + 0x2c, 1);
        write(&mut dma, &mut h, DMA_BASE + 0xd08, 0x34_0000 | 1 << 27);
        write(&mut dma, &mut h, DMA_BASE + 0xd04, 0);
        // 240 stereo frames at 48 kHz: 5 ms.
        assert_eq!(h.events, vec![(0, 5_001_000, 0)]);

        h.now = 5_001_000;
        let mut cx = Cx::new(&mut h, 0, 0, 0);
        dma.event(&mut cx, 0);
        assert_eq!(cx.refusal(), None);
        let a = i16s(&h.ram[BUFFER_A as usize..BUFFER_A as usize + 960]);
        // Left equals right; 1 kHz at 48 kHz has a 48-frame period; peak 0.5.
        assert!(a.chunks(2).all(|f| f[0] == f[1]));
        assert_eq!(a[0], 0);
        assert_eq!(a[2 * 12], (0.5f64 * 32767.0).round() as i16);
        assert_eq!(a[2 * 48], 0);
        assert_eq!(&a[..96], &a[96..192]);
        assert_eq!(h.irqs, vec![(253, true)]);
        assert_eq!(h.events.last(), Some(&(0, 10_001_000, 0)));
        assert!(h.ram[BUFFER_B as usize..BUFFER_B as usize + 960].iter().all(|b| *b == 0));
    }

    #[test]
    fn stereo_24_bit_words_carry_the_sample_in_the_low_three_bytes() {
        let mut h = host();
        let mut serial = AudioSerial::new(SERIAL, tone()).unwrap();
        open(&mut serial, &mut h, 0x17);
        let fifo = serial.fifo();
        // 0x1398168: 4-byte loads, 4-byte stores and DMAADNH DAR 1 pack 3 bytes.
        assert_eq!(fifo.transfer_ticks(1920), Some(5_000_000));
        let words: Vec<i32> = fifo
            .read(SERIAL, 4 * 26)
            .unwrap()
            .chunks(4)
            .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        assert_eq!(words[24], (0.5f64 * 8_388_607.0).round() as i32);
        assert_eq!(words[24], words[25]);
        assert!(fifo.write(SERIAL + 4, &[0; 4]).is_ok());
        assert!(fifo.read(SERIAL + 4, 4).is_err());
    }

    /// The X-T5 4.31 audio windows open both serial instances and the PL330 on this board.
    #[test]
    fn the_xt5_audio_windows_open() {
        let windows = crate::boot::xt5_windows(&["audio-serial", "audio-dma"]);
        let mut graph = DeviceGraph::new();
        register_all(&mut graph, &windows).unwrap();
        assert!(graph.at(SERIAL).is_ok());
        assert!(graph.at(0xff991000).is_ok());
        assert!(graph.at(DMA_BASE).is_ok());
        let mut moved = windows.clone();
        moved.retain(|w| w.kind == "audio-serial");
        moved[0].base = 0xff920000;
        assert!(register_all(&mut DeviceGraph::new(), &moved).is_err());
    }

    #[test]
    fn silence_and_checkpoint_restore_keep_the_sample_position() {
        let mut h = host();
        let silent = Signal {
            sample_rate_hz: 48000,
            tone_hz: 0,
            tone_amplitude: 0.0,
        };
        let mut serial = AudioSerial::new(SERIAL, silent).unwrap();
        open(&mut serial, &mut h, 0xf);
        assert_eq!(serial.fifo().read(SERIAL, 8).unwrap(), [0; 8]);
        let saved = serial.checkpoint().unwrap();
        let mut copy = AudioSerial::new(SERIAL, silent).unwrap();
        copy.restore_checkpoint(&saved).unwrap();
        assert_eq!(copy.lock().state, serial.lock().state);
        assert_eq!(copy.lock().state.delivered, 4);
        let other = AudioSerial::new(SERIAL, tone()).unwrap();
        assert!(other.validate_checkpoint(&saved).is_err());
        assert!(AudioSerial::new(SERIAL, Signal { tone_hz: 24000, ..tone() }).is_err());
    }
}
