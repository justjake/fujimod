use super::{Engine, Snapshots};
use std::io::Cursor;

const REQUEST_MAGIC: &[u8; 8] = b"FJWGREQ1";
const RESULT_MAGIC: &[u8; 8] = b"FJWGRES1";
const MAX_BLOB_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ITEMS: usize = 1 << 20;
const MAX_COMMAND_WORDS: usize = 1024 * 1024 / 4;

pub struct WebGpuRequest {
    pub engine: Engine,
    pub words: Vec<u32>,
}

struct Input<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Input<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], String> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or("DMP WebGPU payload offset overflow")?;
        let bytes = self
            .bytes
            .get(self.offset..end)
            .ok_or("DMP WebGPU payload is truncated")?;
        self.offset = end;
        Ok(bytes)
    }

    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn blob(&mut self) -> Result<Vec<u8>, String> {
        let length = self.u64()?;
        if length > MAX_BLOB_BYTES {
            return Err("DMP WebGPU payload blob exceeds limit".into());
        }
        let length = usize::try_from(length).map_err(|_| "DMP WebGPU payload exceeds usize")?;
        Ok(self.take(length)?.to_vec())
    }

    fn is_empty(&self) -> bool {
        self.offset == self.bytes.len()
    }
}

fn push_count(output: &mut Vec<u8>, count: usize) -> Result<(), String> {
    let count = u32::try_from(count).map_err(|_| "DMP WebGPU item count exceeds u32")?;
    output.extend_from_slice(&count.to_le_bytes());
    Ok(())
}

fn push_blob(output: &mut Vec<u8>, bytes: &[u8]) {
    output.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    output.extend_from_slice(bytes);
}

fn encode_engine(engine: &Engine) -> Result<Vec<u8>, String> {
    let mut state = Vec::new();
    ciborium::ser::into_writer(engine, &mut state)
        .map_err(|error| format!("serialize DMP WebGPU state: {error}"))?;
    Ok(state)
}

fn decode_engine(bytes: Vec<u8>) -> Result<Engine, String> {
    ciborium::de::from_reader(Cursor::new(bytes))
        .map_err(|error| format!("restore DMP WebGPU state: {error}"))
}

pub fn encode_webgpu_request(engine: &Engine, words: &[u32]) -> Result<Vec<u8>, String> {
    if words.len() > MAX_COMMAND_WORDS {
        return Err("DMP WebGPU command list exceeds limit".into());
    }
    let state = encode_engine(engine)?;
    if state.len() as u64 > MAX_BLOB_BYTES {
        return Err("DMP WebGPU state exceeds limit".into());
    }
    let mut output = Vec::with_capacity(8 + 8 + 4 + state.len() + words.len() * 4);
    output.extend_from_slice(REQUEST_MAGIC);
    output.extend_from_slice(&(state.len() as u64).to_le_bytes());
    push_count(&mut output, words.len())?;
    output.extend_from_slice(&state);
    for word in words {
        output.extend_from_slice(&word.to_le_bytes());
    }
    Ok(output)
}

pub fn decode_webgpu_request(bytes: &[u8]) -> Result<WebGpuRequest, String> {
    let mut input = Input::new(bytes);
    if input.take(REQUEST_MAGIC.len())? != REQUEST_MAGIC {
        return Err("Unsupported DMP WebGPU request header".into());
    }
    let state_length = input.u64()?;
    if state_length > MAX_BLOB_BYTES {
        return Err("DMP WebGPU state exceeds limit".into());
    }
    let state_length =
        usize::try_from(state_length).map_err(|_| "DMP WebGPU state exceeds usize")?;
    let word_count = input.u32()? as usize;
    if word_count == 0 || word_count > MAX_COMMAND_WORDS {
        return Err("DMP WebGPU command list exceeds limit".into());
    }
    let engine = decode_engine(input.take(state_length)?.to_vec())?;
    let mut words = Vec::with_capacity(word_count);
    for _ in 0..word_count {
        words.push(input.u32()?);
    }
    if !input.is_empty() {
        return Err("DMP WebGPU request has trailing bytes".into());
    }
    Ok(WebGpuRequest { engine, words })
}

pub fn encode_webgpu_result(engine: &Engine, snapshots: &Snapshots) -> Result<Vec<u8>, String> {
    let state = encode_engine(engine)?;
    if state.len() as u64 > MAX_BLOB_BYTES || snapshots.len() > MAX_ITEMS {
        return Err("DMP WebGPU result exceeds limit".into());
    }
    let ranges = engine.written_ranges();
    if ranges.len() > MAX_ITEMS {
        return Err("DMP WebGPU write ranges exceed limit".into());
    }
    let mut output = Vec::new();
    output.extend_from_slice(RESULT_MAGIC);
    output.extend_from_slice(&(state.len() as u64).to_le_bytes());
    push_count(&mut output, snapshots.len())?;
    push_count(&mut output, ranges.len())?;
    output.extend_from_slice(&state);
    for (address, bytes) in snapshots {
        output.extend_from_slice(&address.to_le_bytes());
        push_blob(&mut output, bytes);
    }
    for (address, length) in ranges {
        output.extend_from_slice(&address.to_le_bytes());
        output.extend_from_slice(&(*length as u64).to_le_bytes());
    }
    Ok(output)
}

fn decode_webgpu_result(bytes: &[u8]) -> Result<(Engine, Snapshots), String> {
    let mut input = Input::new(bytes);
    if input.take(RESULT_MAGIC.len())? != RESULT_MAGIC {
        return Err("Unsupported DMP WebGPU result header".into());
    }
    let state_length = input.u64()?;
    if state_length > MAX_BLOB_BYTES {
        return Err("DMP WebGPU result state exceeds limit".into());
    }
    let state_length =
        usize::try_from(state_length).map_err(|_| "DMP WebGPU result state exceeds usize")?;
    let snapshot_count = input.u32()? as usize;
    let range_count = input.u32()? as usize;
    if snapshot_count > MAX_ITEMS || range_count > MAX_ITEMS {
        return Err("DMP WebGPU result item count exceeds limit".into());
    }
    let mut engine = decode_engine(input.take(state_length)?.to_vec())?;
    let mut snapshots = Vec::with_capacity(snapshot_count);
    for _ in 0..snapshot_count {
        let address = input.u64()?;
        snapshots.push((address, input.blob()?));
    }
    let mut written_ranges = Vec::with_capacity(range_count);
    for _ in 0..range_count {
        let address = input.u64()?;
        let length =
            usize::try_from(input.u64()?).map_err(|_| "DMP WebGPU write range exceeds usize")?;
        written_ranges.push((address, length));
    }
    if !input.is_empty() {
        return Err("DMP WebGPU result has trailing bytes".into());
    }
    engine.written_ranges = written_ranges;
    Ok((engine, snapshots))
}

impl Engine {
    pub fn encode_webgpu_request(&self, words: &[u32]) -> Result<Vec<u8>, String> {
        encode_webgpu_request(self, words)
    }

    pub fn apply_webgpu_result(&mut self, bytes: &[u8]) -> Result<Snapshots, String> {
        let (engine, snapshots) = decode_webgpu_result(bytes)?;
        *self = engine;
        Ok(snapshots)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webgpu_request_round_trips_engine_and_command_words() {
        let mut engine = Engine::default();
        engine.regs.insert(0x250, 0x0302_0100);
        let words = [0x1234_5678, 0x9abc_def0];
        let bytes = encode_webgpu_request(&engine, &words).unwrap();
        let decoded = decode_webgpu_request(&bytes).unwrap();
        assert_eq!(decoded.engine.regs, engine.regs);
        assert_eq!(decoded.words, words);
    }

    #[test]
    fn webgpu_result_restores_written_ranges_and_snapshots() {
        let mut engine = Engine::default();
        engine.written_ranges = vec![(0x1000, 4)];
        let snapshots = vec![(0x1000, vec![1, 2, 3, 4])];
        let bytes = encode_webgpu_result(&engine, &snapshots).unwrap();
        let mut restored = Engine::default();
        assert_eq!(restored.apply_webgpu_result(&bytes).unwrap(), snapshots);
        assert_eq!(restored.written_ranges(), &[(0x1000, 4)]);
    }
}
