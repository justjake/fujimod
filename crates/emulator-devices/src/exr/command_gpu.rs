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
//! (0x8fb84c) so the GPU clears it when it has consumed the list. No interrupt
//! is involved.
//! The list consumption time is a declared input, one tick count per word.
mod render;

use crate::{Cx, Device};
use serde::{Deserialize, Serialize};

const WINDOW: u64 = 0x1000;
const WORDS: usize = WINDOW as usize / 4;
const LEVELS: usize = 0x20 / 4;
const DESCRIPTOR: u64 = 0x40;
const RING_BASE: u64 = 0x80;
const CHANNELS: usize = 8;

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
}

/// The GPU's command front end.
pub struct CommandGpu {
    options: Options,
    state: State,
}

impl CommandGpu {
    pub fn new(options: Options) -> Result<Self, String> {
        if options.base & 0xfff != 0
            || options.base.checked_add(WINDOW).is_none()
            || options.ticks_per_word == 0
        {
            return Err("EXR GPU command front end needs an aligned window and positive timing".into());
        }
        Ok(Self {
            options,
            state: State {
                registers: [0; WORDS],
                lists: Vec::new(),
                free_at: [0; CHANNELS],
                recorded: Vec::new(),
                renderer: render::Renderer::default(),
            },
        })
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
        self.state.free_at[channel] = due;
        self.state.lists.push(List { channel, address, due, words });
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
        }
        Ok(())
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
        self.state.registers[index] = value;
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
        CommandGpu::new(Options {
            base: BASE,
            ticks_per_word: 2,
            setup_ticks: 10,
            record_lists: 0,
            render: false,
            render_registers: vec![],
        })
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
        assert!(CommandGpu::new(Options { base: BASE + 4, ticks_per_word: 1, setup_ticks: 0, record_lists: 0, render: false, render_registers: vec![] }).is_err());
        assert!(CommandGpu::new(Options { base: BASE, ticks_per_word: 0, setup_ticks: 0, record_lists: 0, render: false, render_registers: vec![] }).is_err());
        let mut model = gpu();
        let mut host = TestHost::new();
        let mut cx = host.cx();
        model.write(&mut cx, BASE + 0x40, 4, 15 << 28);
        assert!(cx.refusal().is_some());
    }
}
