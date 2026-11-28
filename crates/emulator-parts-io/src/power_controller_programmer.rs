//! Three-byte SPI programming peer for an external power controller.
//!
//! Models the erase, four-byte/byte program, and verify protocol spoken on the
//! power-controller serial bus while the dedicated mode-select pin is asserted.
//! This is a declared virtual device, not an identification of a camera's MCU.
use crate::serial::{Endpoint, Frame};
use fujimod_emulator_devices::checkpoint::Checkpoint;
use serde::{Deserialize, Serialize};

const SECTOR_BYTES: usize = 0x4000;
const MAX_FLASH_BYTES: usize = 0x10_0000;

/// Declared virtual flash size; the physical part is not identified.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub flash_bytes: usize,
}

/// SPI three-byte programming peer with sector-erased flash initialized to `0xff`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Programmer {
    config: Config,
    #[serde(with = "fujimod_emulator_devices::checkpoint::bytes")]
    flash: Vec<u8>,
    request: [u8; 3],
    reply: [u8; 2],
    position: u8,
    unlock_stage: u8,
    status_lo: u8,
    password_words: [[u8; 2]; 5],
    password_mask: u8,
    flash_enabled: bool,
    op_mode: u8,
    bank: u8,
    offset: u16,
    cursor: usize,
    pending_hi: [u8; 2],
    verify_hi: [u8; 2],
    busy_status: u8,
    verify_ok: bool,
}

impl Programmer {
    pub fn new(config: Config) -> Result<Self, String> {
        if config.flash_bytes < SECTOR_BYTES
            || config.flash_bytes > MAX_FLASH_BYTES
            || !config.flash_bytes.is_multiple_of(SECTOR_BYTES)
        {
            return Err("invalid virtual power-controller flash size".into());
        }
        Ok(Self {
            flash: vec![0xff; config.flash_bytes],
            config,
            request: [0; 3],
            reply: [0; 2],
            position: 0,
            unlock_stage: 0,
            status_lo: 0,
            password_words: [[0; 2]; 5],
            password_mask: 0,
            flash_enabled: false,
            op_mode: 0,
            bank: 0,
            offset: 0,
            cursor: 0,
            pending_hi: [0; 2],
            verify_hi: [0; 2],
            busy_status: 1,
            verify_ok: false,
        })
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn flash(&self) -> &[u8] {
        &self.flash
    }

    /// Reset transient session state when the external mode pin enters programming mode.
    pub fn reset_session(&mut self) {
        self.request = [0; 3];
        self.reply = [0; 2];
        self.position = 0;
        self.unlock_stage = 0;
        self.status_lo = 0;
        self.password_words = [[0; 2]; 5];
        self.password_mask = 0;
        self.flash_enabled = false;
        self.op_mode = 0;
        self.bank = 0;
        self.offset = 0;
        self.cursor = 0;
        self.pending_hi = [0; 2];
        self.verify_hi = [0; 2];
        self.busy_status = 1;
        self.verify_ok = false;
    }

    /// Diagnostic state excludes raw flash bytes (summaries and metadata only).
    pub fn diagnostic_state(&self) -> serde_json::Value {
        serde_json::json!({
            "config": self.config,
            "flash": fujimod_emulator_devices::state::bytes(&self.flash),
            "position": self.position,
            "unlock_stage": self.unlock_stage,
            "status_lo": self.status_lo,
            "password_ok": self.password_ok(),
            "flash_enabled": self.flash_enabled,
            "op_mode": self.op_mode,
            "bank": self.bank,
            "offset": self.offset,
            "cursor": self.cursor,
            "busy_status": self.busy_status,
            "verify_ok": self.verify_ok,
        })
    }

    /// Validate state invariants before accepting a decoded checkpoint.
    pub fn valid(&self) -> bool {
        self.config.flash_bytes == self.flash.len()
            && self.flash.len() >= SECTOR_BYTES
            && self.flash.len() <= MAX_FLASH_BYTES
            && self.flash.len().is_multiple_of(SECTOR_BYTES)
            && self.position < 3
            && self.unlock_stage <= 2
            && self.password_mask <= 0x1f
            && self.cursor <= self.flash.len()
    }

    fn password_ok(&self) -> bool {
        self.unlock_stage == 2
            && self.password_mask == 0x1f
            && self.password_words.iter().all(|w| *w == [0xff, 0xff])
    }

    fn read_reply(&self, command: u8) -> [u8; 2] {
        match command {
            0x01 if self.unlock_stage == 2 => [0x06, self.status_lo],
            0x91 => [0, u8::from(!self.password_ok())],
            0xc5 => [self.busy_status, 0],
            0xe7 => [0, if self.verify_ok { 3 } else { 0 }],
            _ => [0, 0],
        }
    }

    fn address(&self) -> usize {
        (usize::from(self.bank) << 16) | usize::from(self.offset)
    }

    fn program_byte(&mut self, value: u8) {
        if self.cursor < self.flash.len() && (self.flash[self.cursor] & value) == value {
            self.flash[self.cursor] = value;
            self.cursor += 1;
            self.busy_status = 0;
        } else {
            self.busy_status = 1;
        }
    }

    fn program_word(&mut self, word: [u8; 4]) {
        let Some(end) = self.cursor.checked_add(4) else {
            self.busy_status = 1;
            return;
        };
        if end <= self.flash.len()
            && self.flash[self.cursor..end]
                .iter()
                .zip(word)
                .all(|(old, new)| (*old & new) == new)
        {
            self.flash[self.cursor..end].copy_from_slice(&word);
            self.cursor = end;
            self.busy_status = 0;
        } else {
            self.busy_status = 1;
        }
    }

    fn execute(&mut self) {
        let [cmd, hi, lo] = self.request;
        match cmd {
            0xab if hi == 0x63 && lo == 0x59 => {
                self.unlock_stage = 1;
                self.status_lo = 0;
                self.password_mask = 0;
            }
            0xa4 if hi == 0x55 && lo == 0x0d && self.unlock_stage == 1 => {
                self.unlock_stage = 2;
                self.status_lo = 0xe0;
            }
            0x1a if self.unlock_stage == 2 && hi == 0 && matches!(lo, 0x00 | 0x08) => {
                self.status_lo = 0xc0;
            }
            0x96 | 0x98 | 0x9a | 0x9e | 0x9c if self.unlock_stage == 2 => {
                let slot = match cmd {
                    0x96 => 0,
                    0x98 => 1,
                    0x9a => 2,
                    0x9e => 3,
                    0x9c => 4,
                    _ => unreachable!(),
                };
                self.password_words[slot] = [hi, lo];
                self.password_mask |= 1 << slot;
            }
            0xce if hi == 0 => {
                self.flash_enabled = (lo & 1) != 0;
                if self.flash_enabled {
                    self.busy_status = 0;
                }
            }
            0xc0 if hi == 0 => {
                self.op_mode = lo;
            }
            0xc6 if hi == 0 => {
                self.bank = lo;
            }
            0xc8 => {
                self.offset = u16::from_be_bytes([hi, lo]);
            }
            0xc2 if hi == 0 => {
                if !self.password_ok() {
                    self.busy_status = 1;
                    return;
                }
                let addr = self.address();
                match (lo, self.flash_enabled, self.op_mode) {
                    (5, true, 1) => {
                        if addr.is_multiple_of(SECTOR_BYTES)
                            && addr
                                .checked_add(SECTOR_BYTES)
                                .is_some_and(|end| end <= self.flash.len())
                        {
                            self.flash[addr..addr + SECTOR_BYTES].fill(0xff);
                            self.busy_status = 0;
                        } else {
                            self.busy_status = 1;
                        }
                    }
                    (4, true, 5) => {
                        if addr < self.flash.len() {
                            self.cursor = addr;
                            self.busy_status = 0;
                        } else {
                            self.cursor = self.flash.len();
                            self.busy_status = 1;
                        }
                    }
                    (2, _, 3) => {
                        self.cursor = addr.min(self.flash.len());
                        self.verify_ok = false;
                    }
                    _ => {
                        self.cursor = self.flash.len();
                        self.busy_status = 1;
                    }
                }
            }
            0xd2 if self.password_ok() && self.flash_enabled && self.op_mode == 5 => {
                self.pending_hi = [lo, hi];
            }
            0xca if self.password_ok() && self.flash_enabled && self.op_mode == 5 => {
                if self.bank == 0x1f {
                    self.program_byte(lo);
                } else {
                    self.program_word([lo, hi, self.pending_hi[0], self.pending_hi[1]]);
                }
            }
            0xe6 => {}
            0xe4 if self.password_ok() && self.op_mode == 3 => {
                if self.bank == 0x1f {
                    if self.cursor < self.flash.len() {
                        self.verify_ok = self.flash[self.cursor] == lo;
                        self.cursor += 1;
                    } else {
                        self.verify_ok = false;
                    }
                } else {
                    self.verify_hi = [lo, hi];
                }
            }
            0xe2 if self.password_ok() && self.op_mode == 3 && self.bank != 0x1f => {
                let expected = [lo, hi, self.verify_hi[0], self.verify_hi[1]];
                if self
                    .cursor
                    .checked_add(4)
                    .is_some_and(|end| end <= self.flash.len())
                {
                    self.verify_ok = self.flash[self.cursor..self.cursor + 4] == expected;
                    self.cursor += 4;
                } else {
                    self.verify_ok = false;
                }
            }
            _ => {}
        }
    }

    pub fn exchange(&mut self, byte: u8) -> u8 {
        let pos = self.position as usize;
        self.request[pos] = byte;
        let out = match pos {
            0 => {
                self.reply = self.read_reply(byte);
                0
            }
            1 => self.reply[0],
            2 => {
                self.execute();
                self.reply[1]
            }
            _ => unreachable!(),
        };
        self.position = ((pos + 1) % 3) as u8;
        out
    }
}

impl Endpoint for Programmer {
    fn name(&self) -> &'static str {
        "power-controller-programmer"
    }

    fn validate(&self, frame: Frame) -> Result<(), String> {
        if frame.bits != 8 || frame.mode != 3 {
            Err("power-controller programmer requires an 8-bit mode3 frame".into())
        } else {
            Ok(())
        }
    }

    fn reset_transaction(&mut self) {
        self.position = 0;
        self.request = [0; 3];
        self.reply = [0; 2];
    }

    fn transfer(&mut self, frame: Frame, _now_ns: u64) -> u16 {
        self.exchange(frame.word as u8).into()
    }

    fn checkpoint(&self) -> Result<Checkpoint, String> {
        Checkpoint::encode(self.name(), self)
    }

    fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        let decoded: Self = saved.decode(self.name())?;
        if !decoded.valid() || decoded.config != self.config {
            return Err("invalid power-controller programmer checkpoint".into());
        }
        Ok(())
    }

    fn restore_checkpoint(&mut self, saved: &Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        *self = saved.decode(self.name())?;
        Ok(())
    }

    fn save(&self) -> serde_json::Value {
        self.diagnostic_state()
    }

    fn restore(&mut self, _state: &serde_json::Value) -> Result<(), String> {
        Err("power-controller programmer binary flash requires a typed checkpoint".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn send(p: &mut Programmer, b0: u8, b1: u8, b2: u8) -> (u8, u8) {
        let _ = p.exchange(b0);
        let r0 = p.exchange(b1);
        let r1 = p.exchange(b2);
        (r0, r1)
    }

    fn unlock_with_password(p: &mut Programmer, word: [u8; 2]) {
        send(p, 0xab, 0x63, 0x59);
        send(p, 0xa4, 0x55, 0x0d);
        assert_eq!(send(p, 0x01, 0, 0), (0x06, 0xe0));
        send(p, 0x1a, 0x00, 0x08);
        assert_eq!(send(p, 0x01, 0, 0), (0x06, 0xc0));
        send(p, 0x1a, 0x00, 0x00);
        for cmd in [0x96, 0x98, 0x9a, 0x9e, 0x9c] {
            send(p, cmd, word[0], word[1]);
        }
    }

    #[test]
    fn validates_flash_size_and_frame_format() {
        assert!(Programmer::new(Config { flash_bytes: 0 }).is_err());
        assert!(
            Programmer::new(Config {
                flash_bytes: 0x5000
            })
            .is_err()
        );
        assert!(
            Programmer::new(Config {
                flash_bytes: 0x20_0000
            })
            .is_err()
        );
        let p = Programmer::new(Config {
            flash_bytes: 0x1_0000,
        })
        .unwrap();
        assert!(
            p.validate(Frame {
                bits: 8,
                mode: 3,
                word: 0
            })
            .is_ok()
        );
        assert!(
            p.validate(Frame {
                bits: 8,
                mode: 0,
                word: 0
            })
            .is_err()
        );
        assert!(
            p.validate(Frame {
                bits: 16,
                mode: 3,
                word: 0
            })
            .is_err()
        );
    }

    #[test]
    fn unlock_rejects_wrong_sequence_and_non_blank_password() {
        let mut p = Programmer::new(Config {
            flash_bytes: 0x1_0000,
        })
        .unwrap();
        send(&mut p, 0xa4, 0x55, 0x0d);
        assert_eq!(send(&mut p, 0x01, 0, 0), (0, 0));

        unlock_with_password(&mut p, [0xff, 0xfe]);
        assert_eq!(send(&mut p, 0x91, 0, 0), (0, 1));

        p.reset_session();
        unlock_with_password(&mut p, [0xff, 0xff]);
        assert_eq!(send(&mut p, 0x91, 0, 0), (0, 0));
    }

    #[test]
    fn erase_program_word_and_byte_modes_verify_and_checkpoint() {
        let mut p = Programmer::new(Config {
            flash_bytes: 0x1_0000,
        })
        .unwrap();
        unlock_with_password(&mut p, [0xff, 0xff]);
        assert_eq!(send(&mut p, 0x91, 0, 0), (0, 0));

        // Erase four 16 KiB sectors across bank 0.
        for offset in [0x0000u16, 0x4000, 0x8000, 0xc000] {
            send(&mut p, 0xce, 0, 1);
            send(&mut p, 0xc0, 0, 1);
            send(&mut p, 0xc6, 0, 0);
            send(&mut p, 0xc8, (offset >> 8) as u8, 0);
            send(&mut p, 0xc2, 0, 5);
            assert_eq!(send(&mut p, 0xc5, 0, 0), (0, 0));
            send(&mut p, 0xce, 0, 0);
            send(&mut p, 0xc5, 0, 0);
        }

        // Program 8 bytes at bank 0 using 4-byte word commands.
        let image = [0x12u8, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0];
        send(&mut p, 0xce, 0, 1);
        send(&mut p, 0xc0, 0, 5);
        send(&mut p, 0xc6, 0, 0);
        send(&mut p, 0xc8, 0, 0);
        send(&mut p, 0xc2, 0, 4);
        for chunk in image.as_chunks::<4>().0 {
            send(&mut p, 0xd2, chunk[3], chunk[2]);
            send(&mut p, 0xca, chunk[1], chunk[0]);
            assert_eq!(send(&mut p, 0xc5, 0, 0), (0, 0));
        }
        send(&mut p, 0xce, 0, 0);
        assert_eq!(&p.flash()[..8], &image);

        // Attempting to program a 0->1 bit without erasing fails with busy status 1.
        send(&mut p, 0xce, 0, 1);
        send(&mut p, 0xc0, 0, 5);
        send(&mut p, 0xc6, 0, 0);
        send(&mut p, 0xc8, 0, 0);
        send(&mut p, 0xc2, 0, 4);
        send(&mut p, 0xd2, 0xff, 0xff);
        send(&mut p, 0xca, 0xff, 0xff);
        assert_eq!(send(&mut p, 0xc5, 0, 0), (1, 0));
        send(&mut p, 0xce, 0, 0);
        assert_eq!(&p.flash()[..4], &image[..4]);

        // Verify passes on matching bytes and fails on mismatch.
        send(&mut p, 0xc0, 0, 3);
        send(&mut p, 0xc6, 0, 0);
        send(&mut p, 0xc8, 0, 0);
        send(&mut p, 0xc2, 0, 2);
        send(&mut p, 0xd2, 0, 0);
        send(&mut p, 0xca, 0, 0);
        send(&mut p, 0xe6, 0, 3);
        for chunk in image.as_chunks::<4>().0 {
            send(&mut p, 0xe4, chunk[3], chunk[2]);
            send(&mut p, 0xe2, chunk[1], chunk[0]);
            assert_eq!(send(&mut p, 0xe7, 0, 0), (0, 3));
        }
        send(&mut p, 0xc0, 0, 3);
        send(&mut p, 0xc6, 0, 0);
        send(&mut p, 0xc8, 0, 0);
        send(&mut p, 0xc2, 0, 2);
        send(&mut p, 0xe6, 0, 3);
        send(&mut p, 0xe4, 0x00, 0x00);
        send(&mut p, 0xe2, 0x00, 0x00);
        assert_eq!(send(&mut p, 0xe7, 0, 0), (0, 0));

        // Save emits summary only (no large byte array), restore refuses JSON, checkpoint preserves flash.
        let saved_json = p.save();
        assert_eq!(
            saved_json["flash"],
            serde_json::json!({ "bytes": 0x1_0000 })
        );
        assert!(p.restore(&saved_json).is_err());

        let cp = p.checkpoint().unwrap();
        let mut restored = Programmer::new(Config {
            flash_bytes: 0x1_0000,
        })
        .unwrap();
        restored.restore_checkpoint(&cp).unwrap();
        assert_eq!(&restored.flash()[..8], &image);

        // Out-of-range erase reports busy status 1 without crashing.
        send(&mut restored, 0xce, 0, 1);
        send(&mut restored, 0xc0, 0, 1);
        send(&mut restored, 0xc6, 0, 1);
        send(&mut restored, 0xc8, 0, 0);
        send(&mut restored, 0xc2, 0, 5);
        assert_eq!(send(&mut restored, 0xc5, 0, 0), (1, 0));
    }

    #[test]
    fn byte_mode_at_bank_1f_programs_and_verifies_single_bytes() {
        assert!(
            Programmer::new(Config {
                flash_bytes: 0x20_0000
            })
            .is_err()
        );
        let mut p = Programmer::new(Config {
            flash_bytes: 0x10_0000,
        })
        .unwrap();
        unlock_with_password(&mut p, [0xff, 0xff]);
        // Bank 0x1f is past MAX_FLASH_BYTES (1 MiB) so out-of-range write is rejected safely.
        send(&mut p, 0xce, 0, 1);
        send(&mut p, 0xc0, 0, 5);
        send(&mut p, 0xc6, 0, 0x1f);
        send(&mut p, 0xc8, 0, 0);
        send(&mut p, 0xc2, 0, 4);
        send(&mut p, 0xca, 0xff, 0x5a);
        assert_eq!(send(&mut p, 0xc5, 0, 0), (1, 0));
    }
}
