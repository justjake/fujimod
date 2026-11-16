//! Virtual RL78 Protocol A programming peer.
//!
//! Frame and command formats follow Renesas R01AN0815EJ0110, sections 2, 3, and 4.5.2.
//! This is a declared virtual device, not an identification of a camera's MCU.
//! Pin-level mode entry and timing are not modeled. As a virtual recovery
//! extension, a valid reset or baud command at a frame boundary aborts an
//! active transfer; invalid baud settings return NACK instead of timing out.
use serde::{Deserialize, Serialize};

const SOH: u8 = 1;
const STX: u8 = 2;
const ETX: u8 = 3;
const ETB: u8 = 0x17;
const ACK: u8 = 6;
const COMMAND_ERROR: u8 = 4;
const CHECKSUM_ERROR: u8 = 7;
const NACK: u8 = 0x15;
const PARAMETER_ERROR: u8 = 5;
const VERIFY_ERROR: u8 = 0x0f;
const BLANK_ERROR: u8 = 0x1b;
const WRITE_ERROR: u8 = 0x1c;
const BLOCK: usize = 1024;
const CHUNK: usize = 256;
const SIGNATURE_BYTES: usize = 22;
/// Largest response emitted by a single received byte (signature, two frames).
pub const MAX_RESPONSE_BYTES: usize = 5 + 4 + SIGNATURE_BYTES;

/// Declared virtual code-flash size; the physical device is not measured.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub flash_bytes: usize,
}

/// A peer starts in programming mode with erased virtual code flash.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Peer {
    config: Config,
    #[serde(with = "fujimod_emulator_devices::checkpoint::bytes")]
    frame: Vec<u8>,
    baud_set: bool,
    ready: bool,
    #[serde(with = "fujimod_emulator_devices::checkpoint::bytes")]
    flash: Vec<u8>,
    transfer: Option<Transfer>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Transfer {
    verify: bool,
    next: usize,
    end: usize,
    mismatch: bool,
}

impl Peer {
    /// Diagnostic state excludes the contents of binary buffers.
    pub fn diagnostic_state(&self) -> serde_json::Value {
        serde_json::json!({
            "config": self.config,
            "frame": fujimod_emulator_devices::state::bytes(&self.frame),
            "baud_set": self.baud_set,
            "ready": self.ready,
            "flash": fujimod_emulator_devices::state::bytes(&self.flash),
            "transfer": self.transfer,
        })
    }

    pub fn config(&self) -> &Config {
        &self.config
    }
    pub fn new(config: Config) -> Result<Self, String> {
        if config.flash_bytes < 1024
            || config.flash_bytes > 0x100000
            || config.flash_bytes % 1024 != 0
        {
            return Err("Invalid virtual RL78 code-flash size".into());
        }
        Ok(Self {
            flash: vec![0xff; config.flash_bytes],
            config,
            frame: Vec::new(),
            baud_set: false,
            ready: false,
            transfer: None,
        })
    }

    fn response(data: &[u8]) -> Vec<u8> {
        let mut result = vec![STX, data.len() as u8];
        result.extend_from_slice(data);
        result.push(
            result[1..]
                .iter()
                .fold(0u8, |sum, byte| sum.wrapping_sub(*byte)),
        );
        result.push(ETX);
        result
    }

    fn frame_error(status: u8, data_frame: bool) -> Vec<u8> {
        // ST2 is irrelevant when ST1 reports a data-frame reception error.
        if data_frame {
            Self::response(&[status, 0])
        } else {
            Self::response(&[status])
        }
    }

    fn range(&self, bytes: &[u8]) -> Option<(usize, usize)> {
        if bytes.len() != 6 {
            return None;
        }
        let address =
            |a: &[u8]| usize::from(a[0]) | usize::from(a[1]) << 8 | usize::from(a[2]) << 16;
        let start = address(&bytes[..3]);
        let end = address(&bytes[3..]) + 1;
        (start % BLOCK == 0 && end % BLOCK == 0 && start < end && end <= self.flash.len())
            .then_some((start, end))
    }

    /// Validate serialized state before restoring a checkpoint.
    pub fn valid(&self) -> bool {
        let valid_frame = self.frame.is_empty()
            || ((self.frame[0] == SOH || self.transfer.is_some() && self.frame[0] == STX)
                && (self.frame.len() == 1 || {
                    let size = match self.frame[1] {
                        0 if self.frame[0] == STX => CHUNK,
                        0 => return false,
                        n => usize::from(n),
                    };
                    self.frame.len() < size + 4
                }));
        self.config.flash_bytes == self.flash.len()
            && self.flash.len() >= BLOCK
            && self.flash.len() <= 0x100000
            && self.flash.len() % BLOCK == 0
            && valid_frame
            && (!self.ready || self.baud_set)
            && self.transfer.as_ref().is_none_or(|t| {
                self.ready
                    && t.next < t.end
                    && t.next % CHUNK == 0
                    && t.end % BLOCK == 0
                    && t.end <= self.flash.len()
            })
    }

    /// Consume one serial byte and return a complete response, if available.
    pub fn receive(&mut self, byte: u8) -> Vec<u8> {
        if self.frame.is_empty() && byte != SOH && !(self.transfer.is_some() && byte == STX) {
            return Vec::new();
        }
        self.frame.push(byte);
        let data_frame = self.frame[0] == STX;
        if self.frame.len() < 2 {
            return Vec::new();
        }
        let mut length = usize::from(self.frame[1]);
        if length == 0 && data_frame {
            length = CHUNK;
        }
        if length == 0 {
            self.frame.clear();
            return Self::response(&[NACK]);
        }
        if self.frame.len() < length + 4 {
            return Vec::new();
        }
        let frame = std::mem::take(&mut self.frame);
        if frame[length + 3] != ETX && !(data_frame && frame[length + 3] == ETB) {
            return Self::frame_error(NACK, data_frame);
        }
        if frame[1..=length + 2]
            .iter()
            .fold(0u8, |sum, b| sum.wrapping_add(*b))
            != 0
        {
            return Self::frame_error(CHECKSUM_ERROR, data_frame);
        }
        if data_frame {
            return self.receive_data(&frame[2..length + 2], frame[length + 3]);
        }
        if self.transfer.is_some()
            && !matches!(&frame[2..length + 2], [0x00] | [0x9a, 0..=3, 0x15..=0x37])
        {
            return Self::response(&[NACK]);
        }
        match &frame[2..length + 2] {
            // A virtual 32 MHz, full-speed response; physical clock is unmeasured.
            [0x9a, baud @ 0..=3, voltage] if (0x15..=0x37).contains(voltage) => {
                let _ = baud;
                self.baud_set = true;
                self.ready = false;
                self.transfer = None;
                Self::response(&[ACK, 32, 0])
            }
            [0x00] if self.baud_set => {
                self.ready = true;
                self.transfer = None;
                Self::response(&[ACK])
            }
            [0xc0] if self.ready => {
                let mut signature = [0u8; SIGNATURE_BYTES];
                signature[3..13].copy_from_slice(b"VIRTUAL   ");
                signature[13..16]
                    .copy_from_slice(&(self.flash.len() as u32 - 1).to_le_bytes()[..3]);
                let mut response = Self::response(&[ACK]);
                response.extend(Self::response(&signature));
                response
            }
            [0x32, a, b, c, d, e, f, 0] if self.ready => {
                let Some((start, end)) = self.range(&[*a, *b, *c, *d, *e, *f]) else {
                    return Self::response(&[PARAMETER_ERROR]);
                };
                Self::response(&[if self.flash[start..end].iter().all(|b| *b == 0xff) {
                    ACK
                } else {
                    BLANK_ERROR
                }])
            }
            [0x22, a, b, c] if self.ready => {
                let start = usize::from(*a) | usize::from(*b) << 8 | usize::from(*c) << 16;
                if start % BLOCK != 0
                    || start
                        .checked_add(BLOCK)
                        .is_none_or(|end| end > self.flash.len())
                {
                    return Self::response(&[PARAMETER_ERROR]);
                }
                self.flash[start..start + BLOCK].fill(0xff);
                Self::response(&[ACK])
            }
            [command @ (0x40 | 0x13), a, b, c, d, e, f] if self.ready => {
                let Some((start, end)) = self.range(&[*a, *b, *c, *d, *e, *f]) else {
                    return Self::response(&[PARAMETER_ERROR]);
                };
                self.transfer = Some(Transfer {
                    verify: *command == 0x13,
                    next: start,
                    end,
                    mismatch: false,
                });
                Self::response(&[ACK])
            }
            [0x00 | 0x9a | 0xc0 | 0x32 | 0x22 | 0x40 | 0x13, ..] => Self::response(&[NACK]),
            _ => Self::response(&[COMMAND_ERROR]),
        }
    }

    fn receive_data(&mut self, data: &[u8], terminator: u8) -> Vec<u8> {
        let t = self.transfer.as_mut().expect("data requires a transfer");
        let Some(end) = t.next.checked_add(data.len()) else {
            return Self::response(&[PARAMETER_ERROR]);
        };
        if data.len() != CHUNK || end > t.end || (end == t.end) != (terminator == ETX) {
            return Self::frame_error(NACK, true);
        }
        let previous = &mut self.flash[t.next..end];
        let status = if t.verify {
            t.mismatch |= previous != data;
            if end == t.end && t.mismatch {
                VERIFY_ERROR
            } else {
                ACK
            }
        } else if previous
            .iter()
            .zip(data)
            .any(|(old, new)| old & new != *new)
        {
            WRITE_ERROR
        } else {
            previous.copy_from_slice(data);
            ACK
        };
        if status != ACK && !t.verify {
            self.transfer = None;
            return Self::response(&[ACK, status]);
        }
        t.next = end;
        let mut response = Self::response(&[ACK, status]);
        if end == t.end {
            let programming = !t.verify;
            self.transfer = None;
            if programming {
                response.extend(Self::response(&[ACK]));
            }
        }
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn send(peer: &mut Peer, bytes: &[u8]) -> Vec<u8> {
        bytes.iter().flat_map(|b| peer.receive(*b)).collect()
    }
    fn command(payload: &[u8]) -> Vec<u8> {
        let mut bytes = vec![SOH, payload.len() as u8];
        bytes.extend_from_slice(payload);
        bytes.push(bytes[1..].iter().fold(0u8, |sum, b| sum.wrapping_sub(*b)));
        bytes.push(ETX);
        bytes
    }
    fn data(payload: &[u8], last: bool) -> Vec<u8> {
        let mut bytes = vec![STX, payload.len() as u8];
        bytes.extend_from_slice(payload);
        bytes.push(bytes[1..].iter().fold(0u8, |sum, b| sum.wrapping_sub(*b)));
        bytes.push(if last { ETX } else { ETB });
        bytes
    }
    fn ready() -> Peer {
        let mut p = Peer::new(Config {
            flash_bytes: 16 * 1024,
        })
        .unwrap();
        send(&mut p, &command(&[0x9a, 0, 0x21]));
        send(&mut p, &command(&[0]));
        p
    }
    #[test]
    fn baud_and_reset_are_framed_and_sequenced() {
        let mut p = Peer::new(Config {
            flash_bytes: 48 * 1024,
        })
        .unwrap();
        assert_eq!(send(&mut p, &command(&[0])), Peer::response(&[NACK]));
        assert_eq!(
            send(&mut p, &command(&[0x9a, 0, 0x21])),
            Peer::response(&[ACK, 32, 0])
        );
        assert_eq!(send(&mut p, &command(&[0])), Peer::response(&[ACK]));
    }
    #[test]
    fn bad_checksum_and_commands_do_not_succeed() {
        let mut p = Peer::new(Config {
            flash_bytes: 16 * 1024,
        })
        .unwrap();
        let mut bad = command(&[0x9a, 0, 0x21]);
        bad[4] ^= 1;
        assert_eq!(send(&mut p, &bad), Peer::response(&[CHECKSUM_ERROR]));
        assert_eq!(send(&mut p, &command(&[0])), Peer::response(&[NACK]));
        assert_eq!(
            send(&mut p, &command(&[0xff])),
            Peer::response(&[COMMAND_ERROR])
        );
        assert!(Peer::new(Config { flash_bytes: 1000 }).is_err());
    }
    #[test]
    fn command_with_etb_is_rejected_and_can_be_retried() {
        let mut p = Peer::new(Config {
            flash_bytes: 16 * 1024,
        })
        .unwrap();
        let mut bad = command(&[0x9a, 0, 0x21]);
        *bad.last_mut().unwrap() = ETB;
        assert_eq!(send(&mut p, &bad), Peer::response(&[NACK]));
        assert!(!p.baud_set);
        assert!(p.valid());
        assert_eq!(
            send(&mut p, &command(&[0x9a, 0, 0x21])),
            Peer::response(&[ACK, 32, 0])
        );
    }

    #[test]
    fn zero_length_command_is_rejected_and_can_be_retried() {
        let mut p = Peer::new(Config {
            flash_bytes: 16 * 1024,
        })
        .unwrap();
        assert_eq!(send(&mut p, &[SOH, 0, 0, ETX]), Peer::response(&[NACK]));
        assert!(!p.baud_set);
        assert!(p.valid());
        assert_eq!(
            send(&mut p, &command(&[0x9a, 0, 0x21])),
            Peer::response(&[ACK, 32, 0])
        );
    }

    #[test]
    fn signature_and_real_flash_program_verify_and_erase() {
        let mut p = ready();
        let reply = send(&mut p, &command(&[0xc0]));
        assert_eq!(&reply[..5], &Peer::response(&[ACK]));
        assert_eq!(&reply[5 + 2 + 13..5 + 2 + 16], &[0xff, 0x3f, 0]);
        let range = [0, 0, 0, 0xff, 3, 0];
        assert_eq!(
            send(&mut p, &command(&[0x32, 0, 0, 0, 0xff, 3, 0, 0])),
            Peer::response(&[ACK])
        );
        let mut begin = vec![0x40];
        begin.extend_from_slice(&range);
        assert_eq!(send(&mut p, &command(&begin)), Peer::response(&[ACK]));
        for index in 0..4 {
            let reply = send(&mut p, &data(&vec![index; CHUNK], index == 3));
            let mut expected = Peer::response(&[ACK, ACK]);
            if index == 3 {
                expected.extend(Peer::response(&[ACK]));
            }
            assert_eq!(reply, expected);
        }
        assert_eq!(
            send(&mut p, &command(&[0x32, 0, 0, 0, 0xff, 3, 0, 0])),
            Peer::response(&[BLANK_ERROR])
        );
        let saved = fujimod_emulator_devices::checkpoint::Checkpoint::encode("rl78", &p).unwrap();
        p = saved.decode("rl78").unwrap();
        assert!(p.valid());
        let mut verify = vec![0x13];
        verify.extend_from_slice(&range);
        assert_eq!(send(&mut p, &command(&verify)), Peer::response(&[ACK]));
        for index in 0..4 {
            assert_eq!(
                send(&mut p, &data(&vec![index; CHUNK], index == 3)),
                Peer::response(&[ACK, ACK])
            );
        }
        assert_eq!(
            send(&mut p, &command(&[0x22, 0, 0, 0])),
            Peer::response(&[ACK])
        );
        assert!(p.flash[..BLOCK].iter().all(|b| *b == 0xff));
        assert!(p.valid());
    }
    #[test]
    fn bad_ranges_frames_and_flash_conflicts_are_not_acknowledged() {
        let mut p = ready();
        assert_eq!(
            send(&mut p, &command(&[0x22, 1, 0, 0])),
            Peer::response(&[PARAMETER_ERROR])
        );
        assert_eq!(
            send(&mut p, &command(&[0x40, 0, 0, 0, 0xff, 0xff, 0])),
            Peer::response(&[PARAMETER_ERROR])
        );
        let begin = command(&[0x40, 0, 0, 0, 0xff, 3, 0]);
        send(&mut p, &begin);
        let mut bad = data(&vec![0; CHUNK], false);
        bad[20] ^= 1;
        assert_eq!(send(&mut p, &bad), Peer::response(&[CHECKSUM_ERROR, 0]));
        assert!(p.flash[..BLOCK].iter().all(|b| *b == 0xff));
        for i in 0..4 {
            send(&mut p, &data(&vec![0; CHUNK], i == 3));
        }
        send(&mut p, &begin);
        assert_eq!(
            send(&mut p, &data(&vec![0xff; CHUNK], false)),
            Peer::response(&[ACK, WRITE_ERROR])
        );
        assert_eq!(p.flash[0], 0);
        let verify = command(&[0x13, 0, 0, 0, 0xff, 3, 0]);
        send(&mut p, &verify);
        for i in 0..4 {
            let reply = send(&mut p, &data(&vec![0xff; CHUNK], i == 3));
            assert_eq!(
                reply,
                Peer::response(&[ACK, if i == 3 { VERIFY_ERROR } else { ACK }])
            );
        }
    }

    #[test]
    fn malformed_data_can_be_retried_or_recovered_at_a_frame_boundary() {
        let begin = command(&[0x40, 0, 0, 0, 0xff, 3, 0]);
        for recovery in [command(&[0]), command(&[0x9a, 1, 0x21])] {
            let mut p = ready();
            assert_eq!(send(&mut p, &begin), Peer::response(&[ACK]));
            assert_eq!(
                send(&mut p, &data(&[0; 10], false)),
                Peer::response(&[NACK, 0])
            );
            assert_eq!(
                send(&mut p, &data(&[0; CHUNK], true)),
                Peer::response(&[NACK, 0])
            );
            assert_eq!(send(&mut p, &command(&[0x00, 0])), Peer::response(&[NACK]));
            assert!(p.transfer.is_some());
            let mut invalid = recovery.clone();
            let checksum = invalid.len() - 2;
            invalid[checksum] ^= 1;
            assert_eq!(send(&mut p, &invalid), Peer::response(&[CHECKSUM_ERROR]));
            assert!(p.transfer.is_some());
            let expected = if recovery[2] == 0 {
                vec![ACK]
            } else {
                vec![ACK, 32, 0]
            };
            assert_eq!(send(&mut p, &recovery), Peer::response(&expected));
            assert!(p.transfer.is_none());
            if recovery[2] == 0x9a {
                assert_eq!(send(&mut p, &begin), Peer::response(&[NACK]));
                assert_eq!(send(&mut p, &command(&[0])), Peer::response(&[ACK]));
            }
            assert_eq!(send(&mut p, &begin), Peer::response(&[ACK]));
            for _ in 0..3 {
                assert_eq!(
                    send(&mut p, &data(&[0; CHUNK], false)),
                    Peer::response(&[ACK, ACK])
                );
            }
            assert_eq!(
                send(&mut p, &data(&[0; CHUNK], false)),
                Peer::response(&[NACK, 0])
            );
            let mut expected = Peer::response(&[ACK, ACK]);
            expected.extend(Peer::response(&[ACK]));
            assert_eq!(send(&mut p, &data(&[0; CHUNK], true)), expected);
        }
    }
}
