//! Alternate AVC stream input, frame DMA and replay state.
use super::*;
use fujimod_emulator_image_codecs::h264::decode::{self as host, Decoder, Sequence};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct DecodeInput {
    source: u64,
    bytes: usize,
    pub sha256: String,
    spans: Vec<(u64, usize)>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct DecodeOutput {
    pub destination: u64,
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub packing: String,
    pub bytes: usize,
    pub sha256: String,
    pub input_sha256: String,
    pub completed_tick: u64,
    pub slot: u32,
}
#[derive(Default, Serialize, Deserialize)]
pub(super) struct DecodeState {
    pub frames: u64,
    pub last_input: Option<DecodeInput>,
    pub last_output: Option<DecodeOutput>,
    pub sequence: Option<Sequence>,
    pub parameter_bytes: usize,
    pub input_bytes: usize,
    pub pending_input_bytes: usize,
    pub history_lengths: Vec<usize>,
    pub next_slot: u32,
}
#[derive(Default)]
pub(super) struct DecodeRuntime {
    pub input: Arc<[u8]>,
    pub pending_input: Arc<[u8]>,
    pub parameters: Arc<[u8]>,
    pub history: Vec<Arc<[u8]>>,
    pub codec: Option<Decoder>,
}

impl AvcCodec {
    pub(super) fn decode_frame(&mut self, cx: &Cx<'_>) -> Result<(), String> {
        if self.state.config.is_some() {
            return Err("AVC mode change requires initialization".into());
        }
        if self.state.pending.is_some() {
            return Err("AVC decode submitted while busy".into());
        }
        // These fields select progressive AVC syntax and the three frame
        // slots. Encoder E00..EF0 fields are inactive in alternate mode.
        if self.r(0xf14) != 0xf7f9
            || self.r(0xf18) != 0xd8ff
            || self.r(0xf1c) != 3
            || self.r(0x440) != 0
            || self.r(0x444) != 0
            || self.r(0x4fc) != 0
            || self.r(0x4a4) & 0x10 == 0
        {
            return Err("AVC alternate stream configuration is unsupported".into());
        }
        let sizes = [self.r(0x494) as usize, self.r(0x49c) as usize];
        if sizes.iter().any(|&n| n == 0 || n > 0x100000 || n % 64 != 0) {
            return Err("AVC input requires bounded aligned stream banks".into());
        }
        let bases = [
            physical(self.r(0x490).into(), sizes[0])?,
            physical(self.r(0x498).into(), sizes[1])?,
        ];
        if bases.iter().any(|n| n % 64 != 0) {
            return Err("AVC stream bank address is unaligned".into());
        }
        let bank = ((self.r(0x4a4) >> 5) & 1) as usize;
        let cursor = self.r(0x4ac) as usize;
        if cursor >= sizes[bank] || !self.state.bank_available[bank] {
            return Err("AVC input bank is unavailable or exhausted".into());
        }
        let first = sizes[bank] - cursor;
        let mut input = cx.ram_bytes(bases[bank] + cursor as u64, first)?.to_vec();
        // A bank may be aliased for a bounded preview. Do not reread that
        // alias as an imaginary successor stream when the current span ends.
        if bases[0] != bases[1] && self.state.bank_available[bank ^ 1] {
            input.extend_from_slice(&cx.ram_bytes(bases[bank ^ 1], sizes[bank ^ 1])?);
        }
        let (consumed, idr) = host::access_unit(&input)?;
        let packet: Arc<[u8]> = input[..consumed].into();
        let parameters = host::parameter_sets(&packet)?;
        let sequence = host::packet_sequence(&packet)?
            .or(self.state.decode.sequence)
            .ok_or("AVC input has no bounded sequence header")?;
        let slot_count = self.r(0xf1c) as usize;
        let slot = self.state.decode.next_slot as usize % slot_count;
        let destination = self.r(0x400 + slot * 4) as u64;
        let length = sequence.width * sequence.height * 3 / 2;
        let address = physical(destination, length)?;
        if !destination.is_multiple_of(8)
            || bases.iter().zip(sizes).any(|(base, size)| {
                address < *base + size as u64 && *base < address + length as u64
            })
        {
            return Err("AVC output overlaps its stream or is unaligned".into());
        }
        cx.ram_bytes(address, length)?;
        if self.state.decode.sequence.is_some_and(|s| s != sequence) {
            return Err("AVC sequence dimensions changed without reset".into());
        }
        let headers: Arc<[u8]> = if parameters.is_empty() {
            self.decode_runtime.parameters.clone()
        } else {
            parameters.into()
        };
        let mut replay_packet = packet.to_vec();
        if idr && host::parameter_sets(&packet)?.is_empty() {
            replay_packet = headers
                .iter()
                .copied()
                .chain(packet.iter().copied())
                .collect();
        }
        if idr {
            self.decode_runtime.history.clear();
            self.decode_runtime.codec = None;
        }
        if self.decode_runtime.history.len() >= 120
            || self
                .decode_runtime
                .history
                .iter()
                .map(|p| p.len())
                .sum::<usize>()
                + replay_packet.len()
                > 16 * 1024 * 1024
        {
            return Err("AVC GOP exceeds bounded replay history".into());
        }
        if self.decode_runtime.codec.is_none() {
            self.decode_runtime.codec = Some(Decoder::new(&self.decode_runtime.history)?);
        }
        let picture = self
            .decode_runtime
            .codec
            .as_mut()
            .unwrap()
            .decode(&replay_packet)?;
        let deadline = due(
            cx.now(),
            (sequence.width * sequence.height).div_ceil(64) as u64,
        )?;
        let digest = fujimod_emulator_devices::fingerprint::bytes(&picture.i420);
        let input_digest = fujimod_emulator_devices::fingerprint::bytes(&packet);
        let output = DecodeOutput {
            destination,
            width: sequence.width,
            height: sequence.height,
            stride: sequence.width,
            packing: "i420".into(),
            bytes: picture.i420.len(),
            sha256: digest["sha256"].as_str().unwrap().into(),
            completed_tick: deadline,
            slot: slot as u32,
            input_sha256: input_digest["sha256"].as_str().unwrap().into(),
        };
        let mut available = self.state.bank_available;
        let mut next_bank = bank;
        let mut next_cursor = cursor + consumed;
        let mut flags = 1 << 11;
        if next_cursor >= sizes[bank] {
            available[bank] = false;
            next_cursor -= sizes[bank];
            next_bank ^= 1;
            flags |= 1 << (9 + bank);
        }
        let mut spans = vec![(bases[bank] + cursor as u64, consumed.min(first))];
        if consumed > first {
            spans.push((bases[bank ^ 1], consumed - first));
        }
        let receipt = DecodeInput {
            source: bases[bank] + cursor as u64,
            bytes: consumed,
            sha256: input_digest["sha256"].as_str().unwrap().into(),
            spans,
        };
        self.state.decode.sequence = Some(sequence);
        self.decode_runtime.pending_input = packet;
        self.decode_runtime.parameters = headers;
        self.decode_runtime.history.push(replay_packet.into());
        self.state.decode.parameter_bytes = self.decode_runtime.parameters.len();
        self.state.decode.pending_input_bytes = self.decode_runtime.pending_input.len();
        self.state.decode.history_lengths = self
            .decode_runtime
            .history
            .iter()
            .map(|p| p.len())
            .collect();
        self.state.mode = 2;
        self.state.bank_available = available;
        self.state.pending = Some(Pending {
            deadline,
            writes: vec![(address, length)],
            cursor: next_cursor as u32,
            bank: next_bank as u32,
            bits: 0,
            flags,
            mode: 2,
            decoded: Some(output),
            decode_input: Some(receipt),
        });
        self.payload = picture.i420.into();
        self.state.payload_length = self.payload.len();
        Ok(())
    }
}
