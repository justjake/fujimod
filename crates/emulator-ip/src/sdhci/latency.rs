//! Optional SD card timing model (diagnostic). Without it every command
//! completes `transfer_ticks` after it starts. With it each command takes a
//! seeded, randomized time built from published SD card behavior:
//! - a non-data command answers after `command_us`;
//! - a read waits an access time (`read_access_us`, the card's Nac) before
//!   its first block, then `read_block_us` per 512-byte block;
//! - a write moves `write_block_us` per block and then holds the data line
//!   busy while it programs (`write_busy_us`); with `write_stall_permille`
//!   that busy period is a long one (`write_stall_us`, flash erase / block
//!   merge). Transfer complete (the interrupt) comes when the busy ends, as
//!   on an SDHCI host;
//! - after a write the card may start internal garbage collection
//!   (`gc_permille`, `gc_us`): the next data command waits until it ends.
//! Ranges are inclusive [min, max] microseconds, drawn uniformly. The model
//! state (random stream, busy deadline, statistics) is not checkpointed; a
//! restored run restarts the stream from `seed`.
//!
//! Contributed by Taylor Chapman (heytchap) and Claude (Anthropic); see CREDITS.md.
use serde::{Deserialize, Serialize};

fn d48() -> u64 {
    48
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Latency {
    pub seed: u64,
    #[serde(default = "d48")]
    pub ticks_per_us: u64,
    pub command_us: [u64; 2],
    pub read_access_us: [u64; 2],
    pub read_block_us: [u64; 2],
    pub write_block_us: [u64; 2],
    pub write_busy_us: [u64; 2],
    #[serde(default)]
    pub write_stall_permille: u32,
    #[serde(default)]
    pub write_stall_us: [u64; 2],
    #[serde(default)]
    pub gc_permille: u32,
    #[serde(default)]
    pub gc_us: [u64; 2],
    /// Report (stderr) any command whose total time reaches this many us; 0 = never.
    #[serde(default)]
    pub trace_us: u64,
}
#[derive(Clone, Debug, Default, Serialize)]
pub struct Stats {
    pub commands: u64,
    pub reads: u64,
    pub writes: u64,
    pub read_blocks: u64,
    pub write_blocks: u64,
    pub stalls: u64,
    pub gcs: u64,
    pub gc_waits: u64,
    pub busy_ticks: u64,
    pub max_ticks: u64,
}
pub struct Model {
    pub cfg: Latency,
    state: u64,
    /// The card is busy (internal GC) until this tick.
    pub busy_until: u64,
    pub stats: Stats,
}
impl Model {
    pub fn new(cfg: Latency) -> Result<Self, String> {
        for (name, r) in [
            ("command_us", cfg.command_us),
            ("read_access_us", cfg.read_access_us),
            ("read_block_us", cfg.read_block_us),
            ("write_block_us", cfg.write_block_us),
            ("write_busy_us", cfg.write_busy_us),
            ("write_stall_us", cfg.write_stall_us),
            ("gc_us", cfg.gc_us),
        ] {
            if r[0] > r[1] || r[1] > 10_000_000 {
                return Err(format!("SD latency {name} range invalid"));
            }
        }
        if cfg.ticks_per_us == 0 || cfg.write_stall_permille > 1000 || cfg.gc_permille > 1000 {
            return Err("SD latency rate/probability invalid".into());
        }
        let state = cfg.seed ^ 0x9e37_79b9_7f4a_7c15;
        Ok(Self {
            state: if state == 0 { 1 } else { state },
            cfg,
            busy_until: 0,
            stats: Stats::default(),
        })
    }
    fn next(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn draw(&mut self, r: [u64; 2]) -> u64 {
        let v = if r[1] > r[0] {
            r[0] + self.next() % (r[1] - r[0] + 1)
        } else {
            r[0]
        };
        v * self.cfg.ticks_per_us
    }
    fn chance(&mut self, permille: u32) -> bool {
        permille != 0 && self.next() % 1000 < u64::from(permille)
    }
    /// Ticks from `now` until a request's next event.
    /// `phase`: 0 = the command starts; 1 = a PIO command's response is
    /// in (its data phase begins); 2 = a later data step. `pio`: the host
    /// moves data by PIO, so the command answers at once and the card's
    /// access time / busy show up in the data phase. `blocks`: blocks this
    /// step moves; `last_write`: the step ends a write (program busy).
    #[allow(clippy::too_many_arguments)]
    pub fn step(
        &mut self,
        now: u64,
        command: u8,
        write: Option<bool>,
        blocks: u64,
        phase: u8,
        pio: bool,
        last_write: bool,
    ) -> u64 {
        let mut t = 0;
        if phase == 0 {
            self.stats.commands += 1;
            match write {
                Some(false) => self.stats.reads += 1,
                Some(true) => self.stats.writes += 1,
                None => {}
            }
        }
        // GC holds the card busy: a DMA write waits before it starts, a PIO
        // write before its final transfer complete.
        let gc_wait = |m: &mut Self, at: u64| -> u64 {
            if m.busy_until > at {
                m.stats.gc_waits += 1;
                m.busy_until - at
            } else {
                0
            }
        };
        if write.is_none() || (pio && phase == 0) {
            t += self.draw(self.cfg.command_us);
        } else if write == Some(false) {
            if phase == 0 || phase == 1 {
                t += self.draw(self.cfg.read_access_us);
                if phase == 0 {
                    t += self.draw(self.cfg.command_us);
                }
            }
            for _ in 0..blocks {
                t += self.draw(self.cfg.read_block_us);
            }
            self.stats.read_blocks += blocks;
        } else {
            if phase == 0 {
                t += self.draw(self.cfg.command_us);
                t += gc_wait(self, now);
            }
            for _ in 0..blocks {
                t += self.draw(self.cfg.write_block_us);
            }
            self.stats.write_blocks += blocks;
            if last_write {
                if pio {
                    t += gc_wait(self, now + t);
                }
                if self.chance(self.cfg.write_stall_permille) {
                    self.stats.stalls += 1;
                    t += self.draw(self.cfg.write_stall_us);
                } else {
                    t += self.draw(self.cfg.write_busy_us);
                }
                if self.chance(self.cfg.gc_permille) {
                    self.stats.gcs += 1;
                    let gc = self.draw(self.cfg.gc_us);
                    self.busy_until = now + t + gc;
                }
            }
        }
        let t = t.max(1);
        self.stats.busy_ticks += t;
        self.stats.max_ticks = self.stats.max_ticks.max(t);
        if self.cfg.trace_us != 0 && t >= self.cfg.trace_us * self.cfg.ticks_per_us {
            eprintln!(
                "sdlat t={:.3}s CMD{command} {} blocks {blocks} took {:.1} ms",
                now as f64 / (self.cfg.ticks_per_us as f64 * 1e6),
                match write {
                    None => "cmd",
                    Some(false) => "read",
                    Some(true) => "write",
                },
                t as f64 / (self.cfg.ticks_per_us as f64 * 1e3)
            );
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn latency(seed: u64) -> Latency {
        Latency {
            seed,
            ticks_per_us: 1,
            command_us: [10, 20],
            read_access_us: [100, 200],
            read_block_us: [30, 40],
            write_block_us: [50, 60],
            write_busy_us: [300, 400],
            write_stall_permille: 0,
            write_stall_us: [0, 0],
            gc_permille: 0,
            gc_us: [0, 0],
            trace_us: 0,
        }
    }

    #[test]
    fn one_seed_gives_one_sequence() {
        let mut a = Model::new(latency(7)).unwrap();
        let mut b = Model::new(latency(7)).unwrap();
        let mut c = Model::new(latency(8)).unwrap();
        let steps = |m: &mut Model| (0..32).map(|i| m.step(i * 1000, 17, Some(false), 4, 0, false, false)).collect::<Vec<_>>();
        let first = steps(&mut a);
        assert_eq!(first, steps(&mut b));
        assert_ne!(first, steps(&mut c));
    }

    #[test]
    fn a_dma_read_is_access_command_and_blocks() {
        let mut m = Model::new(latency(1)).unwrap();
        for _ in 0..64 {
            let t = m.step(0, 18, Some(false), 8, 0, false, false);
            assert!((100 + 10 + 8 * 30..=200 + 20 + 8 * 40).contains(&t), "{t}");
        }
        assert_eq!((m.stats.reads, m.stats.read_blocks), (64, 512));
    }

    #[test]
    fn a_write_ends_with_program_busy() {
        let mut m = Model::new(latency(2)).unwrap();
        let t = m.step(0, 25, Some(true), 2, 0, false, true);
        assert!((10 + 2 * 50 + 300..=20 + 2 * 60 + 400).contains(&t), "{t}");
    }

    #[test]
    fn garbage_collection_delays_the_next_write() {
        let mut cfg = latency(3);
        cfg.gc_permille = 1000;
        cfg.gc_us = [5000, 5000];
        let mut m = Model::new(cfg).unwrap();
        let first = m.step(0, 25, Some(true), 1, 0, false, true);
        assert_eq!(m.busy_until, first + 5000);
        let second = m.step(first, 25, Some(true), 1, 0, false, false);
        assert!(second >= 5000, "{second}");
        assert_eq!(m.stats.gc_waits, 1);
    }

    #[test]
    fn invalid_ranges_are_refused() {
        let mut cfg = latency(4);
        cfg.read_block_us = [40, 30];
        assert!(Model::new(cfg).is_err());
        let mut cfg = latency(4);
        cfg.ticks_per_us = 0;
        assert!(Model::new(cfg).is_err());
    }
}
