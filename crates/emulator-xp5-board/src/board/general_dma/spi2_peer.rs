//! Experimental word-oriented SPI2 peer; response is selected after the preceding word.
use fujimod_emulator_devices::checkpoint::Checkpoint;
use fujimod_emulator_parts_io::serial::{Frame, Wire};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const IDLE: u32 = 0xc33ca55a;
const START: u32 = 0xa55a3cc3; // bytes sent by the companion for its little-endian start word
const REPLY: u32 = 0x0a008064; // synthetic payload, observed frame shape and five-bit sum

#[derive(Clone, Serialize, Deserialize)]
pub struct Peer {
    index: u8,
    received: u32,
    response: u32,
    pending: u32,
}
impl Default for Peer {
    fn default() -> Self {
        Self {
            index: 0,
            received: 0,
            response: IDLE,
            pending: REPLY,
        }
    }
}
impl Peer {
    fn valid(&self) -> Result<(), String> {
        if self.index > 3 {
            return Err("invalid SPI2 peer byte index".into());
        }
        Ok(())
    }
    fn frame(frame: Frame) -> Result<(), String> {
        if frame.bits != 8 || frame.mode > 3 || frame.word > 255 {
            return Err("SPI2 peer requires byte frames".into());
        }
        Ok(())
    }
}
impl Wire for Peer {
    fn begin(&mut self, frame: Frame) -> Result<u64, String> {
        Self::frame(frame)?;
        Ok(0)
    }
    fn finish(&mut self, token: u64, frame: Frame, _: u64) -> Result<u16, String> {
        Self::frame(frame)?;
        if token != 0 {
            return Err("invalid SPI2 peer token".into());
        }
        let out = ((self.response >> (24 - self.index * 8)) & 255) as u16;
        self.received = (self.received << 8) | u32::from(frame.word);
        self.index += 1;
        if self.index == 4 {
            if self.received == START {
                self.response = self.pending;
            } else if self.received != 0
                && self.received != IDLE
                && self.received & 0xf8000000 != 0x08000000
            {
                let mut reply = 0x0a008000 | (self.received & 0x7fc0);
                let sum: u32 = [7, 12, 17, 22, 27]
                    .into_iter()
                    .map(|shift| (reply >> shift) & 31)
                    .sum::<u32>()
                    + ((reply >> 6) & 1);
                reply |= (sum & 31) << 1;
                self.pending = reply;
                if self.response != IDLE {
                    self.response = reply;
                }
            }
            self.received = 0;
            self.index = 0;
        }
        Ok(out)
    }
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        Checkpoint::encode("xh2s-spi2-peer", self)
    }
    fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        let state: Self = saved.decode("xh2s-spi2-peer")?;
        if !saved.children().is_empty() || !saved.blobs().is_empty() {
            return Err("invalid SPI2 peer checkpoint".into());
        }
        state.valid()
    }
    fn restore_checkpoint(&mut self, saved: &Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        *self = saved.decode("xh2s-spi2-peer")?;
        Ok(())
    }
    fn validate_pending(&self, saved: &Checkpoint, token: u64, frame: Frame) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        Self::frame(frame)?;
        if token != 0 {
            return Err("invalid SPI2 peer token".into());
        }
        Ok(())
    }
    fn save(&self) -> Value {
        serde_json::to_value(self).expect("serializable peer")
    }
    fn restore(&mut self, state: &Value) -> Result<(), String> {
        let next: Self = serde_json::from_value(state.clone()).map_err(|e| e.to_string())?;
        next.valid()?;
        *self = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn exchange(peer: &mut Peer, word: u32) -> u32 {
        word.to_be_bytes().into_iter().fold(0, |out, byte| {
            let f = Frame {
                bits: 8,
                mode: 0,
                word: byte.into(),
            };
            let token = peer.begin(f).unwrap();
            (out << 8) | u32::from(peer.finish(token, f, 0).unwrap())
        })
    }
    #[test]
    fn spi2_peer_handshake_and_pipelined_reply() {
        let mut peer = Peer::default();
        let sum: u32 = [7, 12, 17, 22, 27]
            .into_iter()
            .map(|shift| (REPLY >> shift) & 31)
            .sum::<u32>()
            + ((REPLY >> 6) & 1);
        assert_eq!((REPLY >> 1) & 31, sum & 31);
        assert_eq!(exchange(&mut peer, START), IDLE);
        assert_eq!(exchange(&mut peer, 0), 0x0a008064);
        assert_eq!(exchange(&mut peer, 0x08008030), 0x0a008064);
        assert_eq!(exchange(&mut peer, 0), REPLY);
    }
    #[test]
    fn pipelined_reply_preserves_request_selector_across_handshake() {
        let mut peer = Peer::default();
        let selector = 0x15;
        let mut request = selector << 6;
        let check: u32 = [7, 12, 17, 22, 27]
            .into_iter()
            .map(|shift| (request >> shift) & 31)
            .sum::<u32>()
            + ((request >> 6) & 1);
        request |= (check & 31) << 1;
        assert_eq!(exchange(&mut peer, request), IDLE);
        assert_eq!(exchange(&mut peer, START), IDLE);
        let reply = exchange(&mut peer, request);
        assert_eq!((reply >> 6) & 0x1ff, selector);
        let sum: u32 = [7, 12, 17, 22, 27]
            .into_iter()
            .map(|shift| (reply >> shift) & 31)
            .sum::<u32>()
            + ((reply >> 6) & 1);
        assert_eq!((reply >> 1) & 31, sum & 31);
        exchange(&mut peer, 0x08000000);
        assert_eq!((exchange(&mut peer, 0) >> 6) & 0x1ff, selector);
    }
    #[test]
    fn checkpoint_mid_word_and_invalid_frames() {
        let mut peer = Peer::default();
        let frame = Frame {
            bits: 8,
            mode: 3,
            word: 0xa5,
        };
        let token = peer.begin(frame).unwrap();
        peer.finish(token, frame, 0).unwrap();
        let checkpoint = peer.checkpoint().unwrap();
        let mut restored = Peer::default();
        restored.restore_checkpoint(&checkpoint).unwrap();
        assert_eq!(restored.save(), peer.save());
        let saved = peer.save();
        restored.restore(&saved).unwrap();
        assert_eq!(restored.save(), saved);
        assert!(
            restored
                .begin(Frame {
                    bits: 16,
                    mode: 3,
                    word: 0
                })
                .is_err()
        );
        assert!(
            restored
                .begin(Frame {
                    bits: 8,
                    mode: 4,
                    word: 0
                })
                .is_err()
        );
        assert!(restored.validate_pending(&checkpoint, 1, frame).is_err());
    }
}
