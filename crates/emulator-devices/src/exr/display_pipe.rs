//! EXR Processor Pro display pipe: three 0x1000 blocks at base (layer),
//! base+0x1000 (output timing) and base+0x2000 (update); pipes at 0xFFF23000
//! and 0xFFF27000. Each block latches 64 words.
//!
//! Layer +0x00 bit 16 is a self-completing reset (driver 0x88a0e8). Output
//! +0x00 bit 8 runs the timing and bit 16 enables the output: the mode change
//! 0x891e40 clears bit 16 (0x889d78, which then waits for the blank level) and
//! registers a frame callback (0x88be60), so frames continue with bit 16 clear,
//! and 0x8886f0 compares the register with its RAM copy apart from bit 8.
//! +0x28 bit 1 is the frame level: it alternates between active (0) and blank
//! (1) while running, raising the IRQ on every edge, and reads 1 while stopped
//! (handler 0x887b60 re-arms on the opposite edge). The layer interrupt is
//! raised on every blank entry while running: pipe 1's layer handler 0x88b7dc
//! (registered on IRQ index 10 by 0x88bf78) applies the layer registers from
//! their RAM copy, so a layer update latches at vertical blank. Update +0x00 bit 28 is a
//! request that clears at the next blank entry while running, or after the
//! reset time while stopped (0x88e62c). The camera's register shadow at
//! 0xa436ec holds layer +0x00 = 0x10100 and a 640x480 window for pipe 1.
//! Durations are declared inputs; scanout pixels are not modeled.
use crate::bus::InterruptLine;
use crate::{Cx, Device};
use serde::{Deserialize, Serialize};

const BLOCK: u64 = 0x1000;
const WORDS: usize = 64;
const RESET: u32 = 1 << 16;
const RUN: u32 = 1 << 8;
const FRAME_LEVEL: u32 = 1 << 1;
const STATUS: usize = 0x28 / 4;
const REQUEST: u32 = 1 << 28;
const MAX_EDGES_PER_ADVANCE: usize = 1024;

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub base: u64,
    pub reset_ticks: u64,
    pub active_ticks: u64,
    pub blank_ticks: u64,
}

pub struct Wiring {
    /// Frame-level edges.
    pub irq: Option<InterruptLine>,
    /// Blank entry, for layer updates.
    pub layer_irq: Option<InterruptLine>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    #[serde(with = "crate::checkpoint::array")]
    layer: [u32; WORDS],
    #[serde(with = "crate::checkpoint::array")]
    output: [u32; WORDS],
    #[serde(with = "crate::checkpoint::array")]
    update: [u32; WORDS],
    layer_due: Option<u64>,
    update_due: Option<u64>,
    update_at_frame: bool,
    blank: bool,
    frame_due: Option<u64>,
}

/// One display pipe's layer, output-timing and update blocks.
pub struct DisplayPipe {
    options: Options,
    irq: Option<InterruptLine>,
    layer_irq: Option<InterruptLine>,
    state: State,
}

impl DisplayPipe {
    pub fn new(options: Options, wiring: Wiring) -> Result<Self, String> {
        if options.base & 0xfff != 0 || options.base.checked_add(3 * BLOCK).is_none() {
            return Err(format!(
                "EXR display-pipe base {:#x} is not a 0x1000-aligned window",
                options.base
            ));
        }
        if options.reset_ticks == 0 || options.active_ticks == 0 || options.blank_ticks == 0 {
            return Err("EXR display-pipe durations must be positive".into());
        }
        Ok(Self {
            options,
            irq: wiring.irq,
            layer_irq: wiring.layer_irq,
            state: State {
                layer: [0; WORDS],
                output: [0; WORDS],
                update: [0; WORDS],
                layer_due: None,
                update_due: None,
                update_at_frame: false,
                blank: true,
                frame_due: None,
            },
        })
    }

    /// (block, word) of a modeled access.
    fn locate(&self, address: u64, size: u8) -> Option<(usize, usize)> {
        let offset = address.checked_sub(self.options.base)?;
        let (block, word) = (offset / BLOCK, offset % BLOCK);
        (size == 4 && offset.is_multiple_of(4) && block < 3 && word < WORDS as u64 * 4)
            .then_some((block as usize, (word / 4) as usize))
    }

    fn after(&self, now: u64, ticks: u64) -> Result<u64, String> {
        now.checked_add(ticks)
            .ok_or_else(|| "EXR display-pipe deadline overflow".into())
    }

    fn run(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        let now = cx.now();
        if self.state.layer_due.is_some_and(|due| due <= now) {
            self.state.layer[0] |= RESET;
            self.state.layer_due = None;
        }
        if self.state.update_due.is_some_and(|due| due <= now) {
            self.state.update[0] &= !REQUEST;
            self.state.update_due = None;
        }
        let mut edges = 0;
        while let Some(due) = self.state.frame_due.filter(|&due| due <= now) {
            if edges == MAX_EDGES_PER_ADVANCE {
                let phase = if self.state.blank {
                    self.options.blank_ticks
                } else {
                    self.options.active_ticks
                };
                self.state.frame_due = Some(self.after(now, phase)?);
                break;
            }
            edges += 1;
            self.state.blank = !self.state.blank;
            raise(cx, self.irq)?;
            if self.state.blank {
                raise(cx, self.layer_irq)?;
            }
            if self.state.blank && self.state.update_at_frame {
                self.state.update[0] &= !REQUEST;
                self.state.update_at_frame = false;
            }
            let phase = if self.state.blank {
                self.options.blank_ticks
            } else {
                self.options.active_ticks
            };
            self.state.frame_due = Some(self.after(due, phase)?);
        }
        Ok(())
    }

    fn put(&mut self, now: u64, block: usize, word: usize, value: u32) -> Result<(), String> {
        match (block, word) {
            (0, 0) => {
                self.state.layer_due = if value & RESET == 0 {
                    Some(self.after(now, self.options.reset_ticks)?)
                } else {
                    None
                };
                self.state.layer[0] = value;
            }
            (1, 0) => {
                let was = self.state.output[0] & RUN != 0;
                let runs = value & RUN != 0;
                if !was && runs {
                    self.state.blank = false;
                    self.state.frame_due = Some(self.after(now, self.options.active_ticks)?);
                } else if was && !runs {
                    self.state.blank = true;
                    self.state.frame_due = None;
                }
                self.state.output[0] = value;
            }
            (1, STATUS) => self.state.output[STATUS] = value & !FRAME_LEVEL,
            (2, 0) => {
                self.state.update[0] = value;
                if value & REQUEST != 0 {
                    if self.state.frame_due.is_some() {
                        self.state.update_at_frame = true;
                    } else {
                        self.state.update_due = Some(self.after(now, self.options.reset_ticks)?);
                    }
                }
            }
            (0, w) => self.state.layer[w] = value,
            (1, w) => self.state.output[w] = value,
            (_, w) => self.state.update[w] = value,
        }
        Ok(())
    }
}

fn raise(cx: &mut Cx<'_>, line: Option<InterruptLine>) -> Result<(), String> {
    match line {
        Some(InterruptLine::Core { core, line }) => cx.raise_irq(core, line),
        Some(InterruptLine::Shared(line)) => {
            cx.set_irq(line, true);
            Ok(())
        }
        None => Ok(()),
    }
}

impl Device for DisplayPipe {
    crate::checkpoint_fields!(state: State);

    fn name(&self) -> &'static str {
        "exr-display-pipe"
    }

    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        self.advance(cx);
        let Some((block, word)) = self.locate(address, size) else {
            cx.unmodeled();
            return 0;
        };
        u64::from(match (block, word) {
            (0, w) => self.state.layer[w],
            (1, STATUS) => self.state.output[STATUS] | u32::from(self.state.blank) << 1,
            (1, w) => self.state.output[w],
            (_, w) => self.state.update[w],
        })
    }

    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        self.advance(cx);
        let Some((block, word)) = self.locate(address, size) else {
            cx.unmodeled();
            return;
        };
        let Ok(value) = u32::try_from(value) else {
            cx.fail("EXR display-pipe word exceeds 32 bits");
            return;
        };
        if let Err(error) = self.put(cx.now(), block, word, value) {
            cx.fail(error);
        }
    }

    fn next_deadline(&self, _now: u64) -> Option<u64> {
        [self.state.layer_due, self.state.update_due, self.state.frame_due]
            .into_iter()
            .flatten()
            .min()
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
        self.locate(address, size).is_some()
    }

    fn fingerprint(&self) -> Result<serde_json::Value, String> {
        Ok(self.save())
    }

    fn save(&self) -> serde_json::Value {
        serde_json::to_value(&self.state).unwrap()
    }

    fn restore(&mut self, value: &serde_json::Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        if state.layer_due.is_some() && state.layer[0] & RESET != 0
            || state.frame_due.is_some() != (state.output[0] & RUN != 0)
        {
            return Err("invalid EXR display-pipe checkpoint".into());
        }
        self.state = state;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_host::{TestHost, advance, read, write};

    const BASE: u64 = 0xfff27000;

    fn pipe() -> DisplayPipe {
        DisplayPipe::new(
            Options {
                base: BASE,
                reset_ticks: 10,
                active_ticks: 100,
                blank_ticks: 20,
            },
            Wiring {
                irq: Some(InterruptLine::Core { core: 0, line: 42 }),
                layer_irq: Some(InterruptLine::Core { core: 0, line: 43 }),
            },
        )
        .unwrap()
    }

    #[test]
    fn layer_reset_completes_after_the_declared_time() {
        let mut model = pipe();
        let mut host = TestHost::new();
        host.now = 100;
        write(&mut model, &mut host, BASE, 0x1_0000);
        write(&mut model, &mut host, BASE, 0);
        assert_eq!(read(&mut model, &mut host, BASE), 0);
        assert_eq!(model.next_deadline(100), Some(110));
        advance(&mut model, &mut host, 109);
        assert_eq!(read(&mut model, &mut host, BASE), 0);
        advance(&mut model, &mut host, 110);
        assert_eq!(read(&mut model, &mut host, BASE), 0x1_0000);
        write(&mut model, &mut host, BASE + 0x78, 0x1234_5678);
        assert_eq!(read(&mut model, &mut host, BASE + 0x78), 0x1234_5678);
        assert!(!model.pollable(BASE + 0x100, 4));
        assert!(!model.pollable(BASE + 0x3000, 4));
    }

    #[test]
    fn output_frame_level_alternates_and_raises_the_irq() {
        let mut model = pipe();
        let mut host = TestHost::new();
        assert_eq!(read(&mut model, &mut host, BASE + 0x1028) & 2, 2);
        write(&mut model, &mut host, BASE + 0x1000, 0x1_0000);
        assert_eq!(read(&mut model, &mut host, BASE + 0x1028) & 2, 2);
        assert_eq!(model.next_deadline(host.now), None);
        write(&mut model, &mut host, BASE + 0x1000, 0x100);
        assert_eq!(read(&mut model, &mut host, BASE + 0x1028) & 2, 0);
        advance(&mut model, &mut host, 99);
        assert!(host.irqs.is_empty());
        advance(&mut model, &mut host, 100);
        assert_eq!(read(&mut model, &mut host, BASE + 0x1028) & 2, 2);
        assert_eq!(host.irqs, [(0, 42), (0, 43)]);
        advance(&mut model, &mut host, 120);
        assert_eq!(read(&mut model, &mut host, BASE + 0x1028) & 2, 0);
        assert_eq!(host.irqs, [(0, 42), (0, 43), (0, 42)]);
        write(&mut model, &mut host, BASE + 0x1000, 0);
        assert_eq!(read(&mut model, &mut host, BASE + 0x1028) & 2, 2);
        assert_eq!(model.next_deadline(host.now), None);
    }

    #[test]
    fn update_request_clears_at_blank_or_after_reset() {
        let mut model = pipe();
        let mut host = TestHost::new();
        write(&mut model, &mut host, BASE + 0x2000, REQUEST);
        assert_eq!(read(&mut model, &mut host, BASE + 0x2000), REQUEST as u64);
        advance(&mut model, &mut host, 10);
        assert_eq!(read(&mut model, &mut host, BASE + 0x2000), 0);
        write(&mut model, &mut host, BASE + 0x1000, 0x100);
        write(&mut model, &mut host, BASE + 0x2000, REQUEST);
        advance(&mut model, &mut host, 109);
        assert_eq!(read(&mut model, &mut host, BASE + 0x2000), REQUEST as u64);
        advance(&mut model, &mut host, 110);
        assert_eq!(read(&mut model, &mut host, BASE + 0x2000), 0);
    }

    #[test]
    fn options_are_validated() {
        let wiring = || Wiring {
            irq: None,
            layer_irq: None,
        };
        let options = Options {
            base: BASE,
            reset_ticks: 10,
            active_ticks: 100,
            blank_ticks: 20,
        };
        assert!(DisplayPipe::new(Options { base: BASE + 4, ..options }, wiring()).is_err());
        assert!(DisplayPipe::new(Options { reset_ticks: 0, ..options }, wiring()).is_err());
        assert!(DisplayPipe::new(Options { blank_ticks: 0, ..options }, wiring()).is_err());
    }
}
