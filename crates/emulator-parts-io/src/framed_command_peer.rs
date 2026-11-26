//! Peer of a framed full-duplex command link (8-bit SPI mode 3, no chip select).
//!
//! The master shifts 32-bit big-endian words in pairs. In the first word of a
//! pair both sides send a frame; in the second, each side acknowledges the
//! frame it received in the first. A frame keeps a 5-bit check in bits 1..5:
//! the sum of bits 27..31, 22..26, 17..21, 12..16, 7..11 and bit 6, modulo 32.
//! An acknowledgment sets bit 27, carries a 3-bit sequence number in bits
//! 24..26 and the acknowledged frame's bits 6..14 plus 0x200 in bits 6..15.
//!
//! This peer's own frame repeats the last nonempty frame it received (a
//! command), starting from a declared initial frame, and it acknowledges every
//! valid frame. The master's second frame after a command therefore reads the
//! command back. A reply carrying data is not modeled (UNVERIFIED: the
//! camera's peer held the polled command's frame in idle REC). Its sequence number counts its acknowledgments modulo 8
//! (UNVERIFIED: the camera's acknowledgments carried 2 and then 1 in idle REC).
//! The link's resynchronization words, a frame with a failing check, a second
//! word that is not an acknowledgment, payload transfers and any other word
//! are refused. Reply timing is not modeled: each byte is answered as it
//! shifts.
use super::serial::{Endpoint, Frame};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Resynchronization words of the link.
const RESYNC: [u32; 3] = [0xc33c_5aa5, 0xc33c_a55a, 0xa55a_3cc3];

/// The 5-bit frame check over every field but bits 0..5.
pub fn check(word: u32) -> u32 {
    ((word >> 27)
        + (word >> 22 & 31)
        + (word >> 17 & 31)
        + (word >> 12 & 31)
        + (word >> 7 & 31)
        + (word >> 6 & 1))
        & 31
}
fn checked(word: u32) -> bool {
    word >> 1 & 31 == check(word)
}
/// The acknowledgment of `frame` with sequence number `sequence`.
pub fn acknowledgment(frame: u32, sequence: u8) -> u32 {
    let word = 1 << 27 | u32::from(sequence & 7) << 24 | ((frame >> 6 & 0x1ff) + 0x200) << 6;
    word | check(word) << 1
}
fn is_acknowledgment(word: u32) -> bool {
    word >> 16 & !0x7e0 == 0x800 && checked(word)
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
enum Half {
    #[default]
    Frame,
    Acknowledgment,
}
#[derive(Default, Serialize, Deserialize)]
struct State {
    half: Half,
    /// Bytes of the current word already shifted.
    shifted: u8,
    received: u32,
    sending: u32,
    sequence: u8,
    frames: u64,
    /// The frame sent after the current pair: the last nonempty frame received.
    own: u32,
}
pub struct Peer {
    initial_frame: u32,
    state: State,
}
impl Peer {
    pub fn new(initial_frame: u32) -> Result<Self, String> {
        if !checked(initial_frame)
            || initial_frame & 1 << 27 != 0
            || RESYNC.contains(&initial_frame)
        {
            return Err(format!(
                "framed command peer initial frame {initial_frame:#010x} is not a valid frame"
            ));
        }
        Ok(Self {
            initial_frame,
            state: State {
                sending: initial_frame,
                own: initial_frame,
                ..State::default()
            },
        })
    }
    fn refusal(&self, word: u32) -> Option<String> {
        if RESYNC.contains(&word) {
            return Some(format!(
                "framed command link resynchronization {word:#010x} is unmodeled"
            ));
        }
        match self.state.half {
            Half::Frame if word & 1 << 27 != 0 || !checked(word) => Some(format!(
                "framed command link word {word:#010x} is not a frame"
            )),
            Half::Acknowledgment if !is_acknowledgment(word) => Some(format!(
                "framed command link word {word:#010x} is not an acknowledgment"
            )),
            _ => None,
        }
    }
}
impl Endpoint for Peer {
    fn name(&self) -> &'static str {
        "framed-command-peer"
    }
    fn validate(&self, frame: Frame) -> Result<(), String> {
        if frame.bits != 8 || frame.mode != 3 || frame.word > 255 {
            return Err("framed command link requires 8-bit mode 3 bytes".into());
        }
        if self.state.shifted == 3
            && let Some(refusal) = self.refusal(self.state.received << 8 | u32::from(frame.word))
        {
            return Err(refusal);
        }
        Ok(())
    }
    /// The link has no chip select, so a transaction never restarts a word.
    fn reset_transaction(&mut self) {}
    fn transfer(&mut self, frame: Frame, _: u64) -> u16 {
        let s = &mut self.state;
        let reply = s.sending >> (24 - 8 * u32::from(s.shifted)) & 255;
        s.received = s.received << 8 | u32::from(frame.word);
        s.shifted += 1;
        if s.shifted == 4 {
            s.shifted = 0;
            match s.half {
                Half::Frame => {
                    if s.received != 0 {
                        s.own = s.received;
                    }
                    s.sending = acknowledgment(s.received, s.sequence);
                    s.half = Half::Acknowledgment;
                }
                Half::Acknowledgment => {
                    s.sequence = (s.sequence + 1) & 7;
                    s.frames += 1;
                    s.sending = s.own;
                    s.half = Half::Frame;
                }
            }
            s.received = 0;
        }
        reply as u16
    }
    fn save(&self) -> Value {
        json!({"initial_frame": self.initial_frame, "state": self.state})
    }
    fn restore(&mut self, v: &Value) -> Result<(), String> {
        if v["initial_frame"] != json!(self.initial_frame) {
            return Err("framed command peer initial frame differs".into());
        }
        let state: State = serde_json::from_value(v["state"].clone()).map_err(|e| e.to_string())?;
        if state.shifted > 3
            || state.sequence > 7
            || state.received >> (8 * u32::from(state.shifted)) != 0
            || !checked(state.own)
            || state.own & 1 << 27 != 0
        {
            return Err("invalid framed command peer checkpoint state".into());
        }
        self.state = state;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Words measured on an X-H2S in idle REC: the peer's frame and its
    // acknowledgments of the master's empty frame with sequence 2 and 1.
    const IDLE: u32 = 0x0000_0a6a;

    fn word(part: &mut Peer, value: u32) -> Result<u32, String> {
        let mut reply = 0;
        for byte in value.to_be_bytes() {
            let frame = Frame {
                bits: 8,
                mode: 3,
                word: byte.into(),
            };
            part.validate(frame)?;
            reply = reply << 8 | u32::from(part.transfer(frame, 0));
        }
        Ok(reply)
    }

    #[test]
    fn measured_words_keep_their_check() {
        assert!(checked(IDLE));
        assert_eq!(acknowledgment(0, 2), 0x0a00_8022);
        assert_eq!(acknowledgment(0, 1), 0x0900_801a);
    }

    #[test]
    fn empty_frames_are_answered_by_the_initial_frame_then_acknowledged() {
        let mut part = Peer::new(IDLE).unwrap();
        let ours = acknowledgment(IDLE, 0);
        for sequence in 0..9u8 {
            assert_eq!(word(&mut part, 0).unwrap(), IDLE);
            assert_eq!(
                word(&mut part, ours).unwrap(),
                acknowledgment(0, sequence & 7)
            );
        }
    }

    /// X-H2S 7.30 in normal REC (camera reads 2026-11-17):
    /// the main core polls requests 0xa40, 0xa80 and 0xac0 through the M-core,
    /// whose forwarded-reply and receive words held 0x0a6a, request 0xa40 with
    /// its check. At REC entry the main core sends request 0x500 and requires
    /// the reply's bits 6..14 to be 0x14 (0x1717cec).
    #[test]
    fn the_second_frame_after_a_command_reads_the_command_back() {
        let mut part = Peer::new(IDLE).unwrap();
        let ours = |frame| acknowledgment(frame, 0);
        let command = 0x500 | check(0x500) << 1;
        assert_eq!(command, 0x0000_0514);
        assert_eq!(word(&mut part, command).unwrap(), IDLE);
        assert_eq!(
            word(&mut part, ours(IDLE)).unwrap(),
            acknowledgment(command, 0)
        );
        let reply = word(&mut part, 0).unwrap();
        assert_eq!(reply >> 6 & 0x1ff, 0x14);
        assert_eq!(reply, command);
        assert_eq!(word(&mut part, ours(reply)).unwrap(), acknowledgment(0, 1));
        // The peer keeps the last command until another arrives.
        assert_eq!(word(&mut part, 0).unwrap(), command);
        word(&mut part, ours(command)).unwrap();
        let poll = 0xa40 | check(0xa40) << 1;
        assert_eq!(poll, IDLE);
        word(&mut part, poll).unwrap();
        word(&mut part, ours(command)).unwrap();
        assert_eq!(word(&mut part, 0).unwrap(), IDLE);
    }

    #[test]
    fn resynchronization_bad_checks_and_out_of_place_words_refuse() {
        let mut part = Peer::new(IDLE).unwrap();
        let error = word(&mut part, 0xc33c_5aa5).unwrap_err();
        assert!(error.contains("resynchronization"), "{error}");
        let mut part = Peer::new(IDLE).unwrap();
        assert!(
            word(&mut part, IDLE ^ 2)
                .unwrap_err()
                .contains("not a frame")
        );
        let mut part = Peer::new(IDLE).unwrap();
        word(&mut part, 0).unwrap();
        assert!(
            word(&mut part, 0)
                .unwrap_err()
                .contains("not an acknowledgment")
        );
        let error = Peer::new(IDLE ^ 2).err().unwrap();
        assert!(error.contains("not a valid frame"), "{error}");
        let frame = Frame {
            bits: 16,
            mode: 3,
            word: 0,
        };
        assert!(Peer::new(IDLE).unwrap().validate(frame).is_err());
    }

    #[test]
    fn checkpoint_keeps_a_word_in_flight() {
        let mut part = Peer::new(IDLE).unwrap();
        word(&mut part, 0).unwrap();
        part.transfer(
            Frame {
                bits: 8,
                mode: 3,
                word: 0x08,
            },
            0,
        );
        let mut copy = Peer::new(IDLE).unwrap();
        copy.restore(&part.save()).unwrap();
        assert_eq!(copy.save(), part.save());
        assert!(Peer::new(0).unwrap().restore(&part.save()).is_err());
    }
}
