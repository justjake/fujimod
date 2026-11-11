//! EXR Processor Pro GPU command front end at 0xFFF50000 (OpenVG-era GPU
//! driver 0x8f6f00..0x8f8300). Only the submission handshake is modeled; the
//! commands in a list are fetched but not executed, so nothing is drawn.
//!
//! Driver facts (task 0x8f7b6c, init 0x8f81ac): register init writes
//! +0x10 = +0x1c = 0x3ffff, +0x18 = 0xf0ff, +0x24 = 0x62; channel c has its
//! command-list ring base at +0x80 + 8c and a size word at +0x84 + 8c.
//! +0x20 holds one 4-bit FIFO level per channel; the task waits (event 0x3c,
//! 1000 ticks) while a level exceeds 7. A descriptor is written to +0x40:
//! channel in bits 31:28, flags in 27:24, list length in words minus one in
//! 23:12 and the list offset divided by 4 KiB in 11:0, relative to the channel's
//! ring base. The submitter (op 7, 0x8f7e60) then polls, every 200 ticks, until
//! the list's first word reads 0: the list is written with 0xffffffff there
//! (0x8fb84c) so the GPU clears it when it has consumed the list. A list whose
//! descriptor flags have bit 2 set (flags 0xc instead of 0x8) also asks for a
//! completion interrupt: when it is consumed the channel's bit in the status
//! word +0x10 is set, and the interrupt output stays high while a status bit
//! enabled in +0x18 is set; writing ones to +0x10 clears those bits. The
//! driver's handler (0x8f7d78) reads +0x10, writes 0x3ffff back and signals
//! event 0x3d for channel bits 0..7 (0x3c for bit 12, not modeled); init
//! (0x8f81ac) writes +0x18 = 0xf0ff.
//! The list consumption time is a declared input, one tick count per word.
mod render;

use crate::bus::InterruptLine;
use crate::{Cx, Device};
use serde::{Deserialize, Serialize};

const WINDOW: u64 = 0x1000;
const WORDS: usize = WINDOW as usize / 4;
const LEVELS: usize = 0x20 / 4;
const DESCRIPTOR: u64 = 0x40;
const RING_BASE: u64 = 0x80;
const CHANNELS: usize = 8;
const STATUS: usize = 0x10 / 4;
const ENABLE: usize = 0x18 / 4;
/// Descriptor flag (bit 26): raise the completion interrupt when this list is consumed.
const INTERRUPT_FLAG: u32 = 1 << 26;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub base: u64,
    /// Ticks to consume one command word of a list.
    pub ticks_per_word: u64,
    /// Ticks before a list starts to be consumed.
    pub setup_ticks: u64,
    /// Keep the words of up to this many consumed lists in the device state, for host-side decoding.
    #[serde(default)]
    pub record_lists: usize,
    /// Execute consumed lists in software, drawing into guest memory (see `render`).
    #[serde(default)]
    pub render: bool,
    /// Declared register values the renderer starts from (register, value), for state the driver establishes
    /// outside submitted lists.
    #[serde(default)]
    pub render_registers: Vec<(u32, u32)>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct List {
    channel: usize,
    address: u64,
    due: u64,
    #[serde(default)]
    words: u64,
    #[serde(default)]
    interrupt: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordedList {
    channel: usize,
    address: u64,
    at: u64,
    #[serde(default)]
    descriptor_words: u64,
    words: Vec<u32>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    #[serde(with = "crate::checkpoint::array")]
    registers: [u32; WORDS],
    /// Lists in submission order; each channel consumes its own in order.
    lists: Vec<List>,
    /// Tick the channel's previous list finishes.
    free_at: [u64; CHANNELS],
    /// Consumed lists, oldest first, when `record_lists` is set.
    #[serde(default)]
    recorded: Vec<RecordedList>,
    #[serde(default)]
    renderer: render::Renderer,
    /// Level last driven on the completion interrupt output.
    #[serde(default)]
    irq_level: bool,
}

/// The GPU's command front end.
pub struct CommandGpu {
    options: Options,
    irq: Option<InterruptLine>,
    state: State,
}

impl CommandGpu {
    /// `irq`: the completion interrupt output; without it a list asking for one refuses.
    pub fn new(options: Options, irq: Option<InterruptLine>) -> Result<Self, String> {
        if options.base & 0xfff != 0
            || options.base.checked_add(WINDOW).is_none()
            || options.ticks_per_word == 0
        {
            return Err("EXR GPU command front end needs an aligned window and positive timing".into());
        }
        Ok(Self {
            options,
            irq,
            state: State {
                registers: [0; WORDS],
                lists: Vec::new(),
                free_at: [0; CHANNELS],
                recorded: Vec::new(),
                renderer: render::Renderer::default(),
                irq_level: false,
            },
        })
    }

    /// Drive the completion output from the enabled status bits.
    fn publish(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        let high = self.state.registers[STATUS] & self.state.registers[ENABLE] != 0;
        if high != self.state.irq_level {
            if let Some(line) = self.irq {
                line.drive(cx, high)?;
            }
            self.state.irq_level = high;
        }
        Ok(())
    }

    fn index(&self, address: u64, size: u8) -> Option<usize> {
        let offset = address.checked_sub(self.options.base)?;
        (size == 4 && offset.is_multiple_of(4) && offset < WINDOW).then_some((offset / 4) as usize)
    }

    /// FIFO level of one channel: lists submitted and not yet consumed.
    fn level(&self, channel: usize) -> u32 {
        self.state.lists.iter().filter(|l| l.channel == channel).count() as u32
    }

    fn submit(&mut self, cx: &mut Cx<'_>, descriptor: u32) -> Result<(), String> {
        let channel = (descriptor >> 28) as usize;
        if channel >= CHANNELS {
            return Err(format!("EXR GPU descriptor channel {channel} is not wired"));
        }
        if self.level(channel) > 15 {
            return Err("EXR GPU descriptor FIFO overflow".into());
        }
        let words = u64::from(descriptor >> 12 & 0xfff) + 1;
        let ring = u64::from(self.state.registers[(RING_BASE / 4) as usize + channel * 2]);
        let address = ring + (u64::from(descriptor & 0xfff) << 12);
        let start = cx.now().max(self.state.free_at[channel]) + self.options.setup_ticks;
        let due = start
            .checked_add(words * self.options.ticks_per_word)
            .ok_or("EXR GPU list deadline overflow")?;
        let interrupt = descriptor & INTERRUPT_FLAG != 0;
        if interrupt && self.irq.is_none() {
            return Err("EXR GPU list asks for a completion interrupt, but none is wired".into());
        }
        self.state.free_at[channel] = due;
        self.state.lists.push(List {
            channel,
            address,
            due,
            words,
            interrupt,
        });
        Ok(())
    }

    fn run(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        let now = cx.now();
        let mut index = 0;
        while index < self.state.lists.len() {
            if self.state.lists[index].due > now {
                index += 1;
                continue;
            }
            let list = self.state.lists.remove(index);
            if self.options.render {
                let mut bytes = vec![0u8; list.words as usize * 16];
                cx.ram_read(list.address, &mut bytes)?;
                let words: Vec<u32> = bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                    .collect();
                if !self.state.renderer.seeded() {
                    self.state.renderer.seed(&self.options.render_registers)?;
                }
                self.state.renderer.execute(cx, &words)?;
            }
            if self.options.record_lists > 0 {
                // The descriptor's length field counts 16-byte units: a list of n units holds 4n words
                // (lists of 1, 2, 3 and 5 units end their batch terminator at words 3, 7, 11 and 19).
                let mut bytes = vec![0u8; list.words as usize * 16];
                cx.ram_read(list.address, &mut bytes)?;
                let words = bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                    .collect();
                self.state.recorded.push(RecordedList {
                    channel: list.channel,
                    address: list.address,
                    at: now,
                    descriptor_words: list.words,
                    words,
                });
                let excess = self.state.recorded.len().saturating_sub(self.options.record_lists);
                self.state.recorded.drain(..excess);
            }
            // The first word is the fence the submitter polls.
            cx.ram_write(list.address, &0u32.to_le_bytes())?;
            if list.interrupt {
                self.state.registers[STATUS] |= 1 << list.channel;
            }
        }
        self.publish(cx)
    }
}

impl Device for CommandGpu {
    crate::checkpoint_fields!(state: State);

    fn name(&self) -> &'static str {
        "exr-pro-command-gpu"
    }

    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        self.advance(cx);
        let Some(index) = self.index(address, size) else {
            cx.unmodeled();
            return 0;
        };
        if index == LEVELS {
            return (0..CHANNELS)
                .map(|c| u64::from(self.level(c).min(15)) << (c * 4))
                .sum();
        }
        u64::from(self.state.registers[index])
    }

    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        self.advance(cx);
        let Some(index) = self.index(address, size) else {
            cx.unmodeled();
            return;
        };
        let Ok(value) = u32::try_from(value) else {
            cx.fail("EXR GPU register exceeds 32 bits");
            return;
        };
        if index as u64 * 4 == DESCRIPTOR {
            if let Err(error) = self.submit(cx, value) {
                cx.fail(error);
            }
            return;
        }
        if index == LEVELS {
            cx.fail("EXR GPU FIFO level is read-only");
            return;
        }
        if index == STATUS {
            self.state.registers[STATUS] &= !value;
        } else {
            self.state.registers[index] = value;
        }
        if (index == STATUS || index == ENABLE)
            && let Err(error) = self.publish(cx)
        {
            cx.fail(error);
        }
    }

    fn next_deadline(&self, _now: u64) -> Option<u64> {
        self.state.lists.iter().map(|l| l.due).min()
    }

    fn advance(&mut self, cx: &mut Cx<'_>) {
        if let Err(error) = self.run(cx) {
            cx.fail(error);
        }
    }

    fn event(&mut self, cx: &mut Cx<'_>, _: u64) {
        self.advance(cx);
    }

    fn pollable(&self, address: u64, size: u8) -> bool {
        self.index(address, size).is_some()
    }

    fn fingerprint(&self) -> Result<serde_json::Value, String> {
        Ok(self.save())
    }

    fn save(&self) -> serde_json::Value {
        serde_json::to_value(&self.state).unwrap()
    }

    fn restore(&mut self, value: &serde_json::Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        if state.lists.iter().any(|l| l.channel >= CHANNELS) {
            return Err("invalid EXR GPU checkpoint".into());
        }
        self.state = state;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_host::{TestHost, advance, read, write};

    const BASE: u64 = 0xfff50000;

    fn gpu() -> CommandGpu {
        CommandGpu::new(
            Options {
                base: BASE,
                ticks_per_word: 2,
                setup_ticks: 10,
                record_lists: 0,
                render: false,
                render_registers: vec![],
            },
            None,
        )
        .unwrap()
    }

    #[test]
    fn a_descriptor_raises_the_channel_level_until_the_list_is_consumed() {
        let mut model = gpu();
        let mut host = TestHost::new();
        write(&mut model, &mut host, BASE + 0x80, 0x1000);
        host.ram[0x1000..0x1004].copy_from_slice(&[0xff; 4]);
        // Channel 0, 4 words, list at ring + 0.
        write(&mut model, &mut host, BASE + 0x40, 3 << 12);
        assert_eq!(read(&mut model, &mut host, BASE + 0x20), 1);
        assert_eq!(model.next_deadline(host.now), Some(10 + 4 * 2));
        advance(&mut model, &mut host, 17);
        assert_eq!(&host.ram[0x1000..0x1004], &[0xff; 4]);
        advance(&mut model, &mut host, 18);
        assert_eq!(&host.ram[0x1000..0x1004], &[0; 4]);
        assert_eq!(read(&mut model, &mut host, BASE + 0x20), 0);
        assert_eq!(model.next_deadline(host.now), None);
    }

    #[test]
    fn a_list_flagged_for_completion_sets_its_status_bit_and_holds_the_line() {
        let line = InterruptLine::Core { core: 0, line: 63 };
        let mut model = CommandGpu::new(
            Options {
                base: BASE,
                ticks_per_word: 2,
                setup_ticks: 10,
                record_lists: 0,
                render: false,
                render_registers: vec![],
            },
            Some(line),
        )
        .unwrap();
        let mut host = TestHost::new();
        write(&mut model, &mut host, BASE + 0x80 + 8 * 2, 0x1000);
        write(&mut model, &mut host, BASE + 0x18, 0xf0ff);
        host.ram[0x1000..0x1004].copy_from_slice(&[0xff; 4]);
        // Channel 2 with flags 0xc: 4 words at ring + 0.
        write(
            &mut model,
            &mut host,
            BASE + 0x40,
            2 << 28 | 0xc << 24 | 3 << 12,
        );
        advance(&mut model, &mut host, 17);
        assert_eq!(read(&mut model, &mut host, BASE + 0x10), 0);
        assert_ne!(host.levels.get(&(0, 63)), Some(&true));
        advance(&mut model, &mut host, 18);
        assert_eq!(read(&mut model, &mut host, BASE + 0x10), 1 << 2);
        assert_eq!(host.levels.get(&(0, 63)), Some(&true));
        // The handler writes ones back: the bit clears and the line drops.
        write(&mut model, &mut host, BASE + 0x10, 0x3ffff);
        assert_eq!(read(&mut model, &mut host, BASE + 0x10), 0);
        assert_eq!(host.levels.get(&(0, 63)), Some(&false));
        // An ordinary list (flags 0x8) sets nothing.
        host.ram[0x1000..0x1004].copy_from_slice(&[0xff; 4]);
        write(
            &mut model,
            &mut host,
            BASE + 0x40,
            2 << 28 | 0x8 << 24 | 3 << 12,
        );
        advance(&mut model, &mut host, 40);
        assert_eq!(read(&mut model, &mut host, BASE + 0x10), 0);
        // A masked status bit is kept but does not raise the line.
        write(&mut model, &mut host, BASE + 0x18, 0);
        host.ram[0x1000..0x1004].copy_from_slice(&[0xff; 4]);
        write(
            &mut model,
            &mut host,
            BASE + 0x40,
            2 << 28 | 0xc << 24 | 3 << 12,
        );
        advance(&mut model, &mut host, 80);
        assert_eq!(read(&mut model, &mut host, BASE + 0x10), 1 << 2);
        assert_eq!(host.levels.get(&(0, 63)), Some(&false));
        // Without a wired line, a list asking for the interrupt refuses.
        let mut unwired = gpu();
        let mut cx = host.cx();
        unwired.write(&mut cx, BASE + 0x40, 4, (0xc << 24 | 3 << 12) as u64);
        assert!(cx.refusal().is_some());
    }

    #[test]
    fn channels_queue_independently_and_lists_on_one_channel_run_in_order() {
        let mut model = gpu();
        let mut host = TestHost::new();
        write(&mut model, &mut host, BASE + 0x80, 0x1000);
        write(&mut model, &mut host, BASE + 0x90, 0x2000);
        write(&mut model, &mut host, BASE + 0x40, 0);
        write(&mut model, &mut host, BASE + 0x40, 1 << 12 | 1);
        write(&mut model, &mut host, BASE + 0x40, 1 << 28);
        assert_eq!(read(&mut model, &mut host, BASE + 0x20), 0x10 | 2);
        // Channel 0's first list and channel 1's list (one word each) finish at 12; channel 0's second
        // list (two words) starts after the first, at 12 + 10, and ends at 26.
        advance(&mut model, &mut host, 12);
        assert_eq!(read(&mut model, &mut host, BASE + 0x20), 1);
        advance(&mut model, &mut host, 25);
        assert_eq!(read(&mut model, &mut host, BASE + 0x20), 1);
        advance(&mut model, &mut host, 26);
        assert_eq!(read(&mut model, &mut host, BASE + 0x20), 0);
    }

    #[test]
    fn invalid_options_and_channels_are_refused() {
        assert!(
            CommandGpu::new(
                Options {
                    base: BASE + 4,
                    ticks_per_word: 1,
                    setup_ticks: 0,
                    record_lists: 0,
                    render: false,
                    render_registers: vec![]
                },
                None
            )
            .is_err()
        );
        assert!(
            CommandGpu::new(
                Options {
                    base: BASE,
                    ticks_per_word: 0,
                    setup_ticks: 0,
                    record_lists: 0,
                    render: false,
                    render_registers: vec![]
                },
                None
            )
            .is_err()
        );
        let mut model = gpu();
        let mut host = TestHost::new();
        let mut cx = host.cx();
        model.write(&mut cx, BASE + 0x40, 4, 15 << 28);
        assert!(cx.refusal().is_some());
    }
}
