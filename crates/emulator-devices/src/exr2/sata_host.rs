//! Declared EXR II SATA/ATA host register block at 0x9FEC0000.
//!
//! X-T1 5.52 initializes this controller before it learns whether a SATA
//! medium is attached (`sata_initialize_controller`, decomp/app/drivers/sata/).
//! The model keeps the controller's configuration and task-file registers and
//! declares the physical link state instead of recognizing the firmware:
//! configuration writes are retained, the declared link is active, the
//! controller reports its initialization complete after the register sequence
//! starts it, and the ATA task-file status reports no attached medium
//! (`0x00`, so a driver's bounded ready polls expire). No ATA command, PIO
//! transfer, DMA descriptor or interrupt is fabricated.
//!
//! Every other offset in the window refuses through `Cx::unmodeled`, and a
//! configuration this model does not declare refuses construction.

use crate::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub const BASE: u64 = 0x9fec_0000;
pub const SIZE: u64 = 0x10000;

/// Declared medium attachment. Only the absent medium is modeled: X-T1 has no
/// SATA device, and no supplied EXR II body declares one.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Medium {
    #[default]
    None,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    /// Physical medium the controller sees. Absent by default.
    #[serde(default)]
    pub medium: Medium,
}

#[derive(Default, Serialize, Deserialize)]
struct State {
    /// Retained configuration words, keyed by word-aligned byte offset.
    words: BTreeMap<u32, u32>,
    /// Set by the initialization register sequence and cleared by its
    /// write-one-to-clear status bit.
    ready: bool,
}

pub struct SataHost {
    state: State,
}

/// Link status: the declared active link (bits 8..15 = 0x100) and the ready
/// low nibble (3). These are physical link state, not a firmware handshake.
const LINK_ACTIVE: u32 = 0x0100 | 0x3;
/// Controller initialization complete in the interrupt/status register.
const CONTROLLER_READY: u32 = 1 << 26;
/// Device control bit the driver polls while the controller is busy.
const DEVICE_CONTROL_BUSY: u32 = 1 << 7;

impl SataHost {
    pub fn new(options: Options) -> Result<Self, String> {
        match options.medium {
            Medium::None => {}
        }
        Ok(Self {
            state: State::default(),
        })
    }

    fn word_offset(address: u64) -> Result<u32, ()> {
        let offset = address.checked_sub(BASE).ok_or(())?;
        if offset >= SIZE {
            return Err(());
        }
        Ok((offset & !3) as u32)
    }

    /// Registers the stock ATA driver addresses through this host.
    fn supported(offset: u32) -> bool {
        matches!(
            offset,
            0x000
                | 0x004
                | 0x008
                | 0x00c
                | 0x010
                | 0x014
                | 0x078
                | 0x080
                | 0x200
                | 0x204
                | 0x208
                | 0x20c
                | 0x210
                | 0x214
                | 0x218
                | 0x21c
                | 0x238
                | 0x240
                | 0x244
                | 0x248
                | 0x268
                | 0x26c
                | 0x270
                | 0x274
                | 0x2a0
                | 0x2b0
                | 0x2b4
                | 0x2b8
                | 0x700
        )
    }

    fn lane(address: u64, size: u8) -> (u32, u32, u32) {
        let shift = (address as u32 & 3) * 8;
        let width = u32::from(size.min(4));
        let lane_mask = if width >= 4 {
            u32::MAX
        } else {
            (1u32 << (width * 8)) - 1
        };
        (shift, lane_mask, lane_mask << shift)
    }

    fn retained(&self, offset: u32) -> u32 {
        self.state.words.get(&offset).copied().unwrap_or(0)
    }

    /// The word a read presents, before the byte-lane extract.
    fn presented(&self, offset: u32) -> u32 {
        match offset {
            0x004 => self.retained(offset) & !0x100,
            0x00c | 0x200 | 0x21c | 0x2b0 | 0x700 => 0,
            0x238 => self.retained(offset) & !DEVICE_CONTROL_BUSY,
            0x240 => (self.retained(offset) & !(0xff00 | 0xf)) | LINK_ACTIVE,
            0x244 => {
                let mut word = self.retained(offset);
                word &= !CONTROLLER_READY;
                word | if self.state.ready { CONTROLLER_READY } else { 0 }
            }
            _ => self.retained(offset),
        }
    }
}

impl Device for SataHost {
    crate::checkpoint_fields!(state: State);

    fn fingerprint(&self) -> std::result::Result<Value, String> {
        Ok(self.save())
    }

    fn name(&self) -> &'static str {
        "exr2-sata-host"
    }

    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let Ok(offset) = Self::word_offset(address) else {
            cx.unmodeled();
            return 0;
        };
        if !Self::supported(offset) {
            cx.unmodeled();
            return 0;
        }
        let word = self.presented(offset);
        let (shift, lane_mask, _) = Self::lane(address, size);
        u64::from((word >> shift) & lane_mask)
    }

    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Ok(offset) = Self::word_offset(address) else {
            cx.unmodeled();
            return;
        };
        if !Self::supported(offset) {
            cx.unmodeled();
            return;
        }
        let (shift, _, mask) = Self::lane(address, size);
        let written = ((value as u32) << shift) & mask;
        match offset {
            0x00c => {
                // Write-one-to-clear DMA completion bits; nothing completes.
            }
            0x200 => {
                // PIO data with no medium: retain the byte lanes for observation.
                self.state.words.insert(offset, self.retained(offset) | written);
            }
            0x21c => {
                // Command is write-only; the status read reports no medium.
                let commands = self.state.words.entry(offset).or_insert(0);
                *commands = (*commands & !mask) | written;
            }
            0x244 => {
                if written & CONTROLLER_READY != 0 {
                    self.state.ready = false;
                }
                let word = self.state.words.entry(offset).or_insert(0);
                *word = (*word & !mask) | (written & !CONTROLLER_READY);
            }
            0x2a0 => {
                // 0x2a2 bit 0 starts the controller's initialization sequence.
                let word = self.state.words.entry(offset).or_insert(0);
                *word = (*word & !mask) | written;
                // The driver starts initialization with a byte write to 0x2a2.
                if address - BASE == 0x2a2 && value & 1 != 0 {
                    self.state.ready = true;
                }
            }
            0x2b0 => {
                // Interrupt status acknowledgements; no events are raised.
            }
            _ => {
                let word = self.state.words.entry(offset).or_insert(0);
                *word = (*word & !mask) | written;
            }
        }
    }

    fn save(&self) -> Value {
        json!({"state": self.state})
    }

    fn restore(&mut self, value: &Value) -> Result<(), String> {
        self.state = serde_json::from_value(value["state"].clone())
            .map_err(|error| format!("invalid SATA host state: {error}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_host::{TestHost, read, write};

    fn host() -> (SataHost, TestHost) {
        (
            SataHost::new(Options {
                medium: Medium::None,
            })
            .unwrap(),
            TestHost::new(),
        )
    }

    #[test]
    fn configuration_writes_are_retained() {
        let (mut sata, mut h) = host();
        write(&mut sata, &mut h, BASE + 0x10, 0x33);
        write(&mut sata, &mut h, BASE + 0x14, 0x9154b);
        write(&mut sata, &mut h, BASE + 0x268, 0x4f);
        write(&mut sata, &mut h, BASE + 0x270, 0x94);
        assert_eq!(read(&mut sata, &mut h, BASE + 0x10), 0x33);
        assert_eq!(read(&mut sata, &mut h, BASE + 0x14), 0x9154b);
        assert_eq!(read(&mut sata, &mut h, BASE + 0x268), 0x4f);
        assert_eq!(read(&mut sata, &mut h, BASE + 0x270), 0x94);
    }

    #[test]
    fn reset_busy_self_clears_and_controller_ready_follows_its_start_register() {
        let (mut sata, mut h) = host();
        assert_eq!(read(&mut sata, &mut h, BASE + 0x4) & 0x100, 0);
        assert_eq!(
            read(&mut sata, &mut h, BASE + 0x244) & u64::from(CONTROLLER_READY),
            0
        );
        let mut cx = h.cx();
        sata.write(&mut cx, BASE + 0x2a2, 1, 1);
        assert_eq!(cx.refusal(), None);
        assert_eq!(
            read(&mut sata, &mut h, BASE + 0x244) & u64::from(CONTROLLER_READY),
            u64::from(CONTROLLER_READY)
        );
        write(&mut sata, &mut h, BASE + 0x244, 0xffff_ffff);
        assert_eq!(
            read(&mut sata, &mut h, BASE + 0x244) & u64::from(CONTROLLER_READY),
            0
        );
    }

    #[test]
    fn declared_link_is_active_and_the_task_file_reports_no_medium() {
        let (mut sata, mut h) = host();
        assert_eq!(
            read(&mut sata, &mut h, BASE + 0x240) & 0xff0f,
            u64::from(LINK_ACTIVE)
        );
        assert_eq!(read(&mut sata, &mut h, BASE + 0x21c), 0);
        write(&mut sata, &mut h, BASE + 0x238, 0x80);
        assert_eq!(
            read(&mut sata, &mut h, BASE + 0x238) & u64::from(DEVICE_CONTROL_BUSY),
            0
        );
        write(&mut sata, &mut h, BASE + 0x2a1, 1);
        assert_eq!(read(&mut sata, &mut h, BASE + 0x2a1), 1);
    }

    #[test]
    fn byte_lanes_merge_within_one_register_word() {
        let (mut sata, mut h) = host();
        let mut cx = h.cx();
        sata.write(&mut cx, BASE + 0x2a1, 1, 1);
        assert_eq!(cx.refusal(), None);
        let mut cx = h.cx();
        sata.write(&mut cx, BASE + 0x2a3, 1, 1);
        assert_eq!(cx.refusal(), None);
        assert_eq!(read(&mut sata, &mut h, BASE + 0x2a0), 0x0100_0100);
    }

    #[test]
    fn unsupported_offsets_refuse() {
        let (mut sata, mut h) = host();
        let mut cx = h.cx();
        sata.write(&mut cx, BASE + 0x300, 4, 0);
        assert!(cx.refusal().is_some());
        let mut cx = h.cx();
        sata.read(&mut cx, BASE + SIZE, 4);
        assert!(cx.refusal().is_some());
        let mut cx = h.cx();
        sata.read(&mut cx, BASE - 4, 4);
        assert!(cx.refusal().is_some());
    }
}
