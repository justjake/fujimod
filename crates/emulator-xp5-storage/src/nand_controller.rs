//! Serial-NAND flash-storage controller at 0xff90a000.
//!
//! Protocol shapes come from the xe5-112 loader corpus (`FUN_014a1550`
//! identification sequence, `FUN_014a1dc8` bus-state decode, `FUN_014a0bf8`/
//! `FUN_014a0c2c` status polls, `FUN_014a1158`/`FUN_014a1224` page read):
//! opcode at 0x40, address words at 0x44 (byte-swapped writes) / 0x48
//! (direct), count at 0xa0 (`(n-1)&0x7ff | 0x8000` on write, bit15 reads as
//! not-busy), strobe 0xc0 bit0, completion 0xf0, FIFO 0x80 for the ≤8-byte
//! transfers only. Page data never uses the FIFO: the 0xff939000 DMA pulls
//! from the 0xf0000000 aperture; 0x13 (page read to cache) + 0x0b (8-byte
//! cache read) serve the bad-block scan.
//!
//! Divergences from the NOR controller, all evidenced in the corpus:
//! - 0xe0 must decode to the bit4-set family (0x18) while the master enable
//!   (0xfc bit0) is set, or `FUN_014a1e20(3)` spins during chip activation.
//! - 0xa0 reads mask bit15 or the `FUN_014a3eb8` busy poll spins.
//! - The loader's init touches 0x08/0x2c/0x30/0x60/0x64/0x68/0x70/0x78/0x90
//!   (slot regs and the DMA-assist window descriptor), which the NOR model
//!   rejects as unmodeled.
//! - Program load (0x02/0x84), program execute (0x10) and block erase (0xd8)
//!   act on the emulated die only; the program load data arrives through the
//!   aperture DMA. The row of 0x10 and 0xd8 is read from 0x50 (inferred from the
//!   application's driver, not from the part's documentation).
use crate::nand_storage::{NandShared, PAGE};
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};

/// 0xa5 read: the loader's block-0 good test subtracts 500 from
/// `CONCAT11(b[2], b[3])` per 4-byte group and requires the result in
/// [0, 2) (`FUN_014a07b0` consumers). 500 = 0x01f4 little-endian in those
/// byte positions satisfies it for every group; the remaining bytes are
/// erased-space 0xff.
const PARAM_PAGE_ID: [u8; 8] = [0xff, 0xff, 0xf4, 0x01, 0xff, 0xff, 0xf4, 0x01];

#[derive(Serialize, Deserialize)]
struct State {
    regs: Vec<u32>,
    active: bool,
    complete: bool,
    cursor: u32,
    tx: Vec<u8>,
    #[serde(default)]
    rx_fifo: VecDeque<u8>,
    /// Get/set-feature register file (0x1f writes, 0x0f reads).
    #[serde(default)]
    features: BTreeMap<u8, u8>,
    /// Row latched by the last 0x13 page-read-to-cache.
    #[serde(default)]
    cached_row: Option<u32>,
    configuration_complete: bool,
    /// Offset of the address register written last among 0x44, 0x48 and 0x50: the loader gives
    /// a feature address in 0x44/0x48, the setup stage in 0x50.
    #[serde(default)]
    last_address_register: u64,
}

pub struct NandController {
    state: State,
    nand: NandShared,
    jedec: u32,
}

impl NandController {
    pub fn new(nand: NandShared, jedec: u32) -> Self {
        let regs = vec![0; 64];
        Self {
            state: State {
                regs,
                active: false,
                complete: false,
                cursor: 0,
                tx: Vec::new(),
                rx_fifo: Default::default(),
                features: BTreeMap::new(),
                cached_row: None,
                configuration_complete: false,
                last_address_register: 0,
            },
            nand,
            jedec,
        }
    }

    fn known(o: u64) -> bool {
        matches!(
            o,
            0 | 4
                | 8
                // Two-bit mode word the setup stage writes (0xff815250) and reads back
                // (0xff815244); kept as a latch, no other effect is evidenced.
                | 0xc
                | 0x10
                | 0x14
                | 0x18
                | 0x20
                | 0x24
                | 0x28
                | 0x2c
                | 0x30
                | 0x34
                | 0x40
                | 0x44
                | 0x48
                | 0x50
                | 0x60
                | 0x64
                | 0x68
                | 0x70
                | 0x74
                | 0x78
                | 0x80
                | 0x90
                | 0x94
                | 0x98
                | 0xa0
                | 0xa4
                | 0xc0
                | 0xc4
                | 0xc8
                | 0xd0
                | 0xe0
                | 0xf0
                | 0xf4
                | 0xf8
                | 0xfc
        )
    }

    fn cmd(&self) -> u8 {
        if self.state.regs[0] & 3 == 1 {
            self.state.regs[0x40 / 4] as u8
        } else {
            (self.state.regs[0x40 / 4] >> 8) as u8
        }
    }

    /// The 1-byte feature address of 0x0f/0x1f. `FUN_014a287c` writes it
    /// byte-swapped into 0x44 or directly into 0x48 depending on the device
    /// table entry, so accept both encodings; 0xc0 (status) is the fallback
    /// the polls need if neither latched.
    /// Set Feature (0x1f) and the 0x81 write the setup stage sends with the same shape: an
    /// address register, one data byte through the FIFO (0xff810fd8). That 0x81 is a feature
    /// write is inferred from this shape, not from the part's documentation.
    fn is_feature_write(cmd: u8) -> bool {
        matches!(cmd, 0x1f | 0x81)
    }

    fn feature_addr(&self) -> u8 {
        if self.state.last_address_register == 0x50 {
            // The setup stage writes the address at 0x50; in the 16-bit (octal) command form
            // the byte sits in bits 15..8 (0xc000 for feature 0xc0), as the opcode does.
            let a50 = self.state.regs[0x50 / 4];
            let byte = if self.state.regs[0] & 3 == 1 { a50 as u8 } else { (a50 >> 8) as u8 };
            if byte != 0 {
                return byte;
            }
        }
        let a44 = self.state.regs[0x44 / 4];
        let a48 = self.state.regs[0x48 / 4];
        let swapped = (a44 >> 8) as u8;
        if swapped != 0 {
            swapped
        } else if a44 as u8 != 0 {
            a44 as u8
        } else if a48 != 0 {
            a48 as u8
        } else {
            0xc0
        }
    }

    /// The 32-bit row (page number) of 0x13/0x0b, direct from 0x48 or 0x44.
    fn row(&self) -> u32 {
        // Single-byte form: the setup stage gives the row at 0x50 (0x7d00 for block 500).
        if self.state.last_address_register == 0x50 && self.state.regs[0] & 3 == 1 {
            return self.state.regs[0x50 / 4];
        }
        let a48 = self.state.regs[0x48 / 4];
        let a44 = self.state.regs[0x44 / 4];
        if a48 != 0 {
            a48
        } else {
            a44
        }
    }

    fn feature_value(&self, addr: u8) -> u8 {
        if addr == 0xc0 {
            // Status: OIP clear (operations complete instantly), WEL from
            // the session, program/erase failure bits clear.
            (self.nand.lock().unwrap().session.write_enable as u8) << 1
        } else {
            self.state.features.get(&addr).copied().unwrap_or(0)
        }
    }

    fn begin(&mut self) -> Result<(), String> {
        let cmd = self.cmd();
        self.state.cursor = 0;
        self.state.tx.clear();
        self.state.active = true;
        self.state.complete = false;
        // No up-front session lock: the 0x0f arm re-enters via
        // feature_value (status WEL read); each arm locks its own use.
        match cmd {
            0x06 => {
                self.nand.lock().unwrap().session.write_enable = true;
                self.state.complete = true;
            }
            0x04 => {
                self.nand.lock().unwrap().session.write_enable = false;
                self.state.complete = true;
            }
            0x9f => {
                let count = (self.state.regs[0xa0 / 4] & 0x7ff) as usize + 1;
                self.state.rx_fifo = (0..count)
                    .map(|i| {
                        if i < 3 {
                            (self.jedec >> (16 - i * 8)) as u8
                        } else {
                            0xff
                        }
                    })
                    .collect();
                self.state.complete = true;
            }
            0x0f => {
                let value = self.feature_value(self.feature_addr());
                let count = (self.state.regs[0xa0 / 4] & 0x7ff) as usize + 1;
                self.state.rx_fifo = std::iter::repeat_n(value, count).collect();
                self.state.complete = true;
            }
            0xa5 => {
                let count = (self.state.regs[0xa0 / 4] & 0x7ff) as usize + 1;
                self.state.rx_fifo = (0..count)
                    .map(|i| PARAM_PAGE_ID[i % PARAM_PAGE_ID.len()])
                    .collect();
                self.state.complete = true;
            }
            // Set Feature: the value arrives through the FIFO and `stop` stores it, so the
            // transaction stays active and incomplete until then.
            0x1f | 0x81 => {}
            0x13 => {
                self.state.cached_row = Some(self.row());
                self.state.complete = true;
            }
            0x0b => {
                let row = self
                    .state
                    .cached_row
                    .ok_or("serial-NAND cache read before page read")?;
                let col = (self.state.regs[0x74 / 4] & 0xfff) as u64;
                let count = (self.state.regs[0xa0 / 4] & 0x7ff) as usize + 1;
                self.state.rx_fifo = self
                    .nand
                    .lock()
                    .unwrap()
                    .read_device(row as u64 * PAGE as u64 + col, count)?
                    .into();
                self.state.complete = true;
            }
            // Program load (0x02/0x84), program execute (0x10) and block
            // erase: modeled refusals — the boot-read milestone never issues
            // them and a silent no-op would misreport update writes.
            // Program load: the data arrives through the aperture DMA into the page buffer.
            0x02 | 0x84 => {
                if cmd == 0x02 {
                    self.nand.lock().unwrap().reset_program_buffer();
                }
                self.state.complete = true;
            }
            // Program execute: the row is in 0x50 as for the block erase.
            0x10 => {
                let row = if self.state.last_address_register == 0x50 {
                    self.state.regs[0x50 / 4]
                } else {
                    self.row()
                };
                self.nand.lock().unwrap().execute_program(row)?;
                self.state.complete = true;
            }
            // Block erase (0xd8 on the W35N01JW): the application's driver writes the block's first
            // row to 0x50 after the Write Enable. The erase completes instantly with WEL clear.
            0xd8 | 0xd0 => {
                let row = if self.state.last_address_register == 0x50 {
                    self.state.regs[0x50 / 4]
                } else {
                    self.row()
                };
                self.nand.lock().unwrap().erase_block(row)?;
                self.state.complete = true;
            }
            _ => return Err(format!("unsupported serial-NAND storage command {cmd:#x}")),
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<(), String> {
        if Self::is_feature_write(self.cmd()) && !self.state.tx.is_empty() {
            self.state
                .features
                .insert(self.feature_addr(), self.state.tx[0]);
        }
        // The setup stage sends Write Enable, waits for WEL, writes with 0x81 and then waits for
        // status bits 1..0 to clear (0xff811298), so the write ends with WEL clear.
        if self.cmd() == 0x81 {
            self.nand.lock().unwrap().session.write_enable = false;
        }
        self.state.active = false;
        self.state.complete = true;
        Ok(())
    }

    fn rx(&mut self) -> Result<u8, String> {
        if let Some(b) = self.state.rx_fifo.pop_front() {
            self.state.cursor += 1;
            return Ok(b);
        }
        if !self.state.active {
            return Err("serial-NAND FIFO read outside transaction".into());
        }
        let b = match self.cmd() {
            0x0f => self.feature_value(self.feature_addr()),
            0x9f => {
                if self.state.cursor < 3 {
                    (self.jedec >> (16 - self.state.cursor * 8)) as u8
                } else {
                    0xff
                }
            }
            0xa5 => PARAM_PAGE_ID[self.state.cursor as usize % PARAM_PAGE_ID.len()],
            0x0b => {
                let row = self
                    .state
                    .cached_row
                    .ok_or("serial-NAND cache read before page read")?;
                let col = (self.state.regs[0x74 / 4] & 0xfff) as u64;
                self.nand
                    .lock()
                    .unwrap()
                    .read_device(row as u64 * PAGE as u64 + col + self.state.cursor as u64, 1)?[0]
            }
            cmd => return Err(format!("serial-NAND FIFO read for command {cmd:#x}")),
        };
        self.state.cursor += 1;
        self.state.complete = true;
        Ok(b)
    }
}

impl NandController {
    fn validate_state(&self, state: &State) -> Result<(), String> {
        if state.regs.len() != self.state.regs.len() {
            return Err("serial flash checkpoint has the wrong register count".into());
        }
        Ok(())
    }
}

impl Device for NandController {
    fn pollable(&self, offset: u64, size: u8) -> bool {
        offset == 0xff90a0f0 && size == 4
    }
    fn name(&self) -> &'static str {
        "serial-NAND storage"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, n: u8) -> u64 {
        let o = address.wrapping_sub(0xff90a000);
        if n != 4 || !Self::known(o) {
            cx.unmodeled();
            return 0;
        }
        match o {
            // The setup stage sends a byte with a read-modify-write of this register
            // (ldr/bfxil/str at 0xff815944..0xff81594c), so a read during a transmit
            // transaction is accepted and carries nothing.
            0x80 if self.state.active && Self::is_feature_write(self.cmd()) => 0,
            0x80 => match self.rx() {
                Ok(b) => b as u64,
                Err(e) => {
                    cx.fail(e);
                    0
                }
            },
            0xe0 => {
                // Bits 4..3 follow the configuration register at +0x10: both drivers write
                // it and wait for the matching state. The loader sets bits 0 and 16 together
                // from its argument (FUN_014a200c) and waits for state 3 (8 in these bits)
                // when set and state 2 (0x18, decoded by FUN_014a1dc8) when clear; the setup
                // stage waits for 8 when `value & 0x10001` is nonzero and 0x18 otherwise
                // (0xff813be0..0xff813c00).
                let enabled = self.state.regs[0xfc / 4] & 1 != 0;
                let mode = if self.state.regs[0x10 / 4] & 0x10001 != 0 {
                    8
                } else {
                    0x18
                };
                // Bit 7 acknowledges the master enable: the setup stage writes the enable
                // bit (0xff8163d4) and then waits for bit 7 to equal it (0xff8139e8 with
                // mask and expected value 0x80), with no timeout.
                mode | if enabled { 0x80 } else { 0 }
                    | if self.state.configuration_complete {
                        1 << 23
                    } else {
                        0
                    }
            }
            0xa0 => (self.state.regs[0xa0 / 4] & 0x7fff) as u64,
            0xf0 => {
                let receive = !self.state.rx_fifo.is_empty()
                    || (self.state.active
                        && matches!(self.cmd(), 0x9f | 0x0f | 0xa5 | 0x0b));
                let complete = self.state.complete;
                // Bit 2 requests transmit data. The setup stage's FIFO service routine
                // (0xff8146a0) sends when bit 2 is set and bytes remain, so it must stay clear
                // during a read transaction such as Get Feature; the loader's driver only waits
                // for bit 0 (FUN_014a25b8).
                let transmit = self.state.active && Self::is_feature_write(self.cmd());
                // Bit 8 accompanies completion: the setup stage's service routine finishes a
                // transaction only when this bit is set and the last chunk was moved
                // (0xff81470c..0xff81479c).
                (if transmit { 4u64 } else { 0 })
                    | (if receive { 2 } else { 0 })
                    | complete as u64
                    | (complete as u64) << 8
            }
            0xd0 => 0,
            _ => self.state.regs[o as usize / 4] as u64,
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, n: u8, v: u64) {
        let o = address.wrapping_sub(0xff90a000);
        if n != 4 || !Self::known(o) {
            cx.unmodeled();
            return;
        }
        let v = v as u32;
        if matches!(o, 0x44 | 0x48 | 0x50) {
            self.state.last_address_register = o;
        }
        let result = match o {
            0x80 => {
                if !self.state.active || !Self::is_feature_write(self.cmd()) {
                    Err(format!(
                        "unsupported serial-NAND FIFO write (command {:#x}, transaction {})",
                        self.cmd(),
                        if self.state.active { "active" } else { "not started" },
                    ))
                } else {
                    self.state.tx.push(v as u8);
                    if self.state.tx.len() == (self.state.regs[0xa0 / 4] & 0x7ff) as usize + 1 {
                        self.stop()
                    } else {
                        Ok(())
                    }
                }
            }
            0xc0 => match v {
                1 => self.begin(),
                2 => self.stop(),
                _ => Err("unsupported serial-NAND transaction strobe".into()),
            },
            0xf0 => {
                if v & 1 != 0 {
                    self.state.complete = false;
                }
                Ok(())
            }
            0xd0 => {
                // Self-clearing resets: bits 0, 8 and 9 (reads return 0). The setup stage
                // sets bit 8 (0xff815ebc) and bit 9 (0xff815ee4) and polls them back
                // (0xff815eb0, 0xff815ed8).
                if v & !0x301 != 0 {
                    Err("unsupported serial-NAND FIFO reset".into())
                } else {
                    Ok(())
                }
            }
            0xe0 => {
                cx.unmodeled();
                return;
            }
            // 0x44/0x48 are plain address words on the NAND path (the NOR
            // model's 0x48 program_mode trigger is not a thing here).
            _ => {
                self.state.regs[o as usize / 4] = v;
                if o == 0xc8 {
                    self.state.configuration_complete = true;
                }
                Ok(())
            }
        };
        if let Err(e) = result {
            cx.fail(e);
        }
    }
    fujimod_emulator_devices::checkpoint_state!(state: State, Self::validate_state);
    fn save(&self) -> Value {
        serde_json::to_value(&self.state).unwrap()
    }
    fn restore(&mut self, v: &Value) -> Result<(), String> {
        self.state = serde_json::from_value(v.clone())
            .map_err(|e| format!("invalid serial-NAND controller checkpoint: {e}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nand_storage::NandStorage;
    use fujimod_emulator_devices::test_host::recording::RecordingHost;
    use std::sync::{Arc, Mutex};

    fn controller(base: Vec<u8>) -> NandController {
        NandController::new(
            Arc::new(Mutex::new(NandStorage::new(base).unwrap())),
            0xefdc21,
        )
    }

    fn strobe(c: &mut NandController, host: &mut RecordingHost) {
        c.write(&mut Cx::new(host, 0, 0, 0), 0xff90a0c0, 4, 1);
    }

    #[test]
    fn jedec_identification_returns_efdc21() {
        let mut c = controller(vec![0xff; PAGE + 2]);
        let mut host = RecordingHost::default();
        c.state.regs[0] = 1;
        c.state.regs[0x40 / 4] = 0x9f;
        c.state.regs[0xa0 / 4] = 2 | 0x8000;
        strobe(&mut c, &mut host);
        let mut id = Vec::new();
        for _ in 0..3 {
            id.push(c.read(&mut Cx::new(&mut host, 0, 0, 0), 0xff90a080, 4) as u8);
        }
        assert_eq!(id, [0xef, 0xdc, 0x21]);
    }

    #[test]
    fn status_polls_track_wel_and_idle() {
        let mut c = controller(vec![0xff; PAGE + 2]);
        let mut host = RecordingHost::default();
        c.state.regs[0] = 1;
        c.state.regs[0x40 / 4] = 0x0f;
        c.state.regs[0xa0 / 4] = 0x8000;
        c.state.regs[0x48 / 4] = 0xc0;
        let status = |c: &mut NandController, host: &mut RecordingHost| {
            strobe(c, host);
            c.read(&mut Cx::new(host, 0, 0, 0), 0xff90a080, 4) as u8
        };
        assert_eq!(status(&mut c, &mut host), 0, "idle: OIP and WEL clear");
        c.state.regs[0x40 / 4] = 0x06;
        strobe(&mut c, &mut host);
        assert!(c
            .nand
            .lock()
            .unwrap()
            .session
            .write_enable);
        c.state.regs[0x40 / 4] = 0x0f;
        assert_eq!(status(&mut c, &mut host), 2, "WEL set after WREN");
    }

    #[test]
    fn set_feature_is_stored_and_readable() {
        let mut c = controller(vec![0xff; PAGE + 2]);
        let mut host = RecordingHost::default();
        c.state.regs[0] = 1;
        // FUN_014a1070(0xffffffb0, 0x10): set-feature addr 0xb0 value 0x10.
        c.state.regs[0x40 / 4] = 0x1f;
        c.state.regs[0xa0 / 4] = 0x8000;
        c.state.regs[0x48 / 4] = 0xb0;
        strobe(&mut c, &mut host);
        c.write(&mut Cx::new(&mut host, 0, 0, 0), 0xff90a080, 4, 0x10);
        assert!(c.state.features[&0xb0] == 0x10);
        c.state.regs[0x40 / 4] = 0x0f;
        strobe(&mut c, &mut host);
        assert_eq!(c.read(&mut Cx::new(&mut host, 0, 0, 0), 0xff90a080, 4) as u8, 0x10);
    }

    #[test]
    fn parameter_page_groups_decode_to_block_good() {
        let mut c = controller(vec![0xff; PAGE + 2]);
        let mut host = RecordingHost::default();
        c.state.regs[0] = 1;
        c.state.regs[0x40 / 4] = 0xa5;
        c.state.regs[0xa0 / 4] = 7 | 0x8000;
        strobe(&mut c, &mut host);
        let mut bytes = Vec::new();
        for _ in 0..8 {
            bytes.push(c.read(&mut Cx::new(&mut host, 0, 0, 0), 0xff90a080, 4) as u8);
        }
        for group in bytes.chunks(4) {
            let value = ((group[3] as u16) << 8) | group[2] as u16;
            assert!(
                (500..502).contains(&value),
                "group {group:?} must satisfy the loader's block-0 good test"
            );
        }
    }

    #[test]
    fn bad_block_scan_reads_cached_page_bytes() {
        // Page 0 = erased (0xff, block good); page 64 (block 1) = data.
        let mut base = vec![0xff; 65 * PAGE];
        base[64 * PAGE] = 0x5a;
        let mut c = controller(base);
        let mut host = RecordingHost::default();
        c.state.regs[0] = 1;
        // FUN_014a1158 passes the full row; 0x48 direct is one encoding.
        c.state.regs[0x40 / 4] = 0x13;
        c.state.regs[0xa0 / 4] = 0;
        c.state.regs[0x48 / 4] = 64;
        strobe(&mut c, &mut host);
        assert_eq!(c.state.cached_row, Some(64));
        c.state.regs[0x40 / 4] = 0x0b;
        c.state.regs[0xa0 / 4] = 7 | 0x8000;
        strobe(&mut c, &mut host);
        let mut bytes = Vec::new();
        for _ in 0..8 {
            bytes.push(c.read(&mut Cx::new(&mut host, 0, 0, 0), 0xff90a080, 4) as u8);
        }
        assert_eq!(bytes[0], 0x5a, "cache read returns the latched page");
        // Block 0: erased page reads 0xff → good.
        c.state.regs[0x48 / 4] = 0;
        c.state.regs[0x40 / 4] = 0x13;
        strobe(&mut c, &mut host);
        c.state.regs[0x40 / 4] = 0x0b;
        strobe(&mut c, &mut host);
        assert_eq!(
            c.read(&mut Cx::new(&mut host, 0, 0, 0), 0xff90a080, 4) as u8,
            0xff
        );
    }

    #[test]
    fn bus_state_decode_matches_loader_expectations() {
        let mut c = controller(vec![0xff; PAGE + 2]);
        let mut host = RecordingHost::default();
        let mut read = |c: &mut NandController| {
            c.read(&mut Cx::new(&mut host, 0, 0, 0), 0xff90a0e0, 4) as u32
        };
        // Configuration bits 0/16 clear: the 0x18 family (loader state 2, setup wait for 0x18).
        c.state.regs[0x10 / 4] = 0;
        assert_eq!(read(&mut c) & 0x18, 0x18);
        // Either bit set: the idle family 8 (loader state 3, setup wait for 8).
        c.state.regs[0x10 / 4] = 1;
        assert_eq!(read(&mut c) & 0x18, 8);
        c.state.regs[0x10 / 4] = 0x10000;
        assert_eq!(read(&mut c) & 0x18, 8);
        // Bit 7 follows the master enable.
        c.state.regs[0xfc / 4] = 0;
        assert_eq!(read(&mut c) & 0x80, 0);
        c.state.regs[0xfc / 4] = 1;
        assert_eq!(read(&mut c) & 0x80, 0x80);
    }

    #[test]
    fn status_requests_transmit_only_for_set_feature_and_flags_completion() {
        let mut c = controller(vec![0xff; PAGE + 2]);
        let mut host = RecordingHost::default();
        c.state.regs[0] = 1;
        let mut status = |c: &mut NandController| {
            c.read(&mut Cx::new(&mut host, 0, 0, 0), 0xff90a0f0, 4) as u32
        };
        // Get Feature: receive data and completion (bits 1, 0 and 8), no transmit request.
        c.state.regs[0x40 / 4] = 0x0f;
        c.state.regs[0xa0 / 4] = 0x8000;
        c.write(&mut Cx::new(&mut RecordingHost::default(), 0, 0, 0), 0xff90a0c0, 4, 1);
        assert_eq!(status(&mut c), 0x103);
        // The driver reads the feature byte, then ends the transaction.
        c.read(&mut Cx::new(&mut RecordingHost::default(), 0, 0, 0), 0xff90a080, 4);
        // Set Feature: transmit requested until the byte arrives, not complete.
        c.state.regs[0x40 / 4] = 0x1f;
        c.write(&mut Cx::new(&mut RecordingHost::default(), 0, 0, 0), 0xff90a0c0, 4, 1);
        assert_eq!(status(&mut c), 0x4);
        // The send routine's read-modify-write read of the FIFO register carries nothing.
        assert_eq!(c.read(&mut Cx::new(&mut RecordingHost::default(), 0, 0, 0), 0xff90a080, 4), 0);
        c.write(&mut Cx::new(&mut RecordingHost::default(), 0, 0, 0), 0xff90a080, 4, 0x10);
        assert_eq!(status(&mut c), 0x101, "complete after the byte, no transmit request");
    }

    #[test]
    fn feature_address_follows_the_register_written_last() {
        let mut c = controller(vec![0xff; PAGE + 2]);
        let mut host = RecordingHost::default();
        c.state.regs[0] = 1;
        let strobe_cmd = |c: &mut NandController, cmd: u32| {
            c.state.regs[0x40 / 4] = cmd;
            c.write(&mut Cx::new(&mut RecordingHost::default(), 0, 0, 0), 0xff90a0c0, 4, 1);
        };
        strobe_cmd(&mut c, 0x06);
        // A stale value in 0x44 must not hide the setup stage's address in 0x50.
        c.write(&mut Cx::new(&mut host, 0, 0, 0), 0xff90a044, 4, 0xb);
        c.write(&mut Cx::new(&mut host, 0, 0, 0), 0xff90a050, 4, 0xc0);
        strobe_cmd(&mut c, 0x0f);
        let status = c.read(&mut Cx::new(&mut host, 0, 0, 0), 0xff90a080, 4) as u8;
        assert_eq!(status & 2, 2, "WEL must read back from the status feature");
    }

    #[test]
    fn octal_form_reads_the_feature_address_from_the_high_byte() {
        let mut c = controller(vec![0xff; PAGE + 2]);
        let mut host = RecordingHost::default();
        // Octal format word: low bits not 1, opcode in bits 15..8.
        c.state.regs[0] = 2;
        c.state.regs[0x40 / 4] = 0x0600;
        c.write(&mut Cx::new(&mut host, 0, 0, 0), 0xff90a0c0, 4, 1);
        c.state.regs[0x40 / 4] = 0x0f00;
        c.write(&mut Cx::new(&mut host, 0, 0, 0), 0xff90a050, 4, 0xc000);
        c.write(&mut Cx::new(&mut host, 0, 0, 0), 0xff90a0c0, 4, 1);
        let status = c.read(&mut Cx::new(&mut host, 0, 0, 0), 0xff90a080, 4) as u8;
        assert_eq!(status & 2, 2, "status feature 0xc0 must read back with WEL set");
    }

    #[test]
    fn write_enable_clears_when_the_0x81_write_completes() {
        let mut c = controller(vec![0xff; PAGE + 2]);
        let mut host = RecordingHost::default();
        c.state.regs[0] = 1;
        let run = |c: &mut NandController, cmd: u32| {
            c.state.regs[0x40 / 4] = cmd;
            c.write(&mut Cx::new(&mut RecordingHost::default(), 0, 0, 0), 0xff90a0c0, 4, 1);
        };
        run(&mut c, 0x06);
        run(&mut c, 0x81);
        c.write(&mut Cx::new(&mut host, 0, 0, 0), 0xff90a080, 4, 0xe7);
        c.write(&mut Cx::new(&mut host, 0, 0, 0), 0xff90a050, 4, 0xc0);
        run(&mut c, 0x0f);
        let status = c.read(&mut Cx::new(&mut host, 0, 0, 0), 0xff90a080, 4) as u8;
        assert_eq!(status & 3, 0, "idle and write-disabled after the 0x81 write");
    }

    #[test]
    fn count_register_reads_as_not_busy() {
        let mut c = controller(vec![0xff; PAGE + 2]);
        let mut host = RecordingHost::default();
        c.state.regs[0xa0 / 4] = 3 | 0x8000;
        assert_eq!(
            c.read(&mut Cx::new(&mut host, 0, 0, 0), 0xff90a0a0, 4),
            3,
            "bit15 must read 0 or FUN_014a3eb8 spins"
        );
    }

    #[test]
    fn init_writes_to_slot_and_descriptor_registers_are_accepted() {
        let mut c = controller(vec![0xff; PAGE + 2]);
        let mut host = RecordingHost::default();
        // FUN_014a2e34 window descriptor + slot regs from FUN_014a1550.
        for (offset, value) in [
            (0x24u64, 0xf0000000u64),
            (0x28, 0xffffffff),
            (0x2c, 0xffffffff),
            (0x30, 0xffffffff),
            (0x34, 0xf8000000),
            (0x60, 0x1234),
            (0x64, 1),
            (0x68, 2),
            (0x70, 3),
            (0x78, 4),
            (0x08, 5),
            (0x90, 6),
        ] {
            let mut cx = Cx::new(&mut host, 0, 0, 0);
            c.write(&mut cx, 0xff90a000 + offset, 4, value);
            assert!(
                cx.refusal().is_none(),
                "init register write 0x{offset:x} must not be unmodeled"
            );
        }
    }

    #[test]
    fn block_erase_clears_the_block_under_write_enable() {
        let mut c = controller(vec![0x00; PAGE * 64 * 2]);
        let mut host = RecordingHost::default();
        let strobe = |c: &mut NandController, cmd: u32| {
            c.state.regs[0x40 / 4] = cmd;
            let mut host = RecordingHost::default();
            let mut cx = Cx::new(&mut host, 0, 0, 0);
            c.write(&mut cx, 0xff90a0c0, 4, 1);
            assert!(cx.refusal().is_none());
        };
        c.state.regs[0] = 1;
        strobe(&mut c, 0x06);
        c.write(&mut Cx::new(&mut host, 0, 0, 0), 0xff90a050, 4, 64);
        strobe(&mut c, 0xd8);
        let nand = c.nand.lock().unwrap();
        assert_eq!(nand.read(64 * PAGE as u64, 4).unwrap(), [0xff; 4]);
        assert_eq!(nand.read(0, 4).unwrap(), [0x00; 4]);
        assert!(!nand.session.write_enable);
    }

    #[test]
    fn program_execute_writes_the_loaded_page_under_write_enable() {
        let mut c = controller(vec![0xff; PAGE * 64 * 2]);
        let mut host = RecordingHost::default();
        let strobe = |c: &mut NandController, cmd: u32| {
            c.state.regs[0x40 / 4] = cmd;
            let mut host = RecordingHost::default();
            let mut cx = Cx::new(&mut host, 0, 0, 0);
            c.write(&mut cx, 0xff90a0c0, 4, 1);
            assert!(cx.refusal().is_none());
        };
        c.state.regs[0] = 1;
        strobe(&mut c, 0x06);
        strobe(&mut c, 0x02);
        c.nand.lock().unwrap().load_program_data(8, &[1, 2, 3, 4]).unwrap();
        c.write(&mut Cx::new(&mut host, 0, 0, 0), 0xff90a050, 4, 70);
        strobe(&mut c, 0x10);
        let nand = c.nand.lock().unwrap();
        assert_eq!(nand.read(70 * PAGE as u64 + 6, 8).unwrap(), [0xff, 0xff, 1, 2, 3, 4, 0xff, 0xff]);
        assert!(!nand.session.write_enable);
    }

}
