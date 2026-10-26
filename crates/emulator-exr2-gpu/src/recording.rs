//! Binary input recordings for the vector GPU replay benchmark.

use crate::{CommandProcessor, Profile};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::Path;
use std::sync::Arc;

const MAGIC_V1: &[u8; 8] = b"FMVGPU01";
const MAGIC_V2: &[u8; 8] = b"FMVGPU02";
const MAX_RECORD_BYTES: u64 = 2 << 30;

#[derive(Clone, Debug)]
pub struct Header {
    pub ram_bytes: u64,
    pub max_fill_bytes: u64,
    pub max_copy_bytes: u64,
    pub max_command_capture_bytes: u64,
    pub max_clear_bytes: u64,
    pub bytes_per_tick: u64,
    pub copy_profile: Option<Profile>,
    pub clear_profile: Option<Profile>,
}

impl From<&CommandProcessor> for Header {
    fn from(state: &CommandProcessor) -> Self {
        Self {
            ram_bytes: state.ram_bytes,
            max_fill_bytes: state.max_fill_bytes,
            max_copy_bytes: state.max_copy_bytes,
            max_command_capture_bytes: state.max_command_capture_bytes,
            max_clear_bytes: state.max_clear_bytes,
            bytes_per_tick: state.bytes_per_tick,
            copy_profile: state.copy_profile,
            clear_profile: state.clear_profile,
        }
    }
}

#[derive(Clone, Debug)]
pub struct MemoryRead {
    pub cpu: i64,
    pub address: u64,
    pub length: u64,
    pub result: std::result::Result<Vec<u8>, String>,
}

#[derive(Clone, Debug)]
pub enum TargetSample {
    Routed { count: usize, selected: i64 },
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct RegisterRead {
    pub now: u64,
    pub cpu: i64,
    pub address: u64,
    pub size: u8,
    pub result: std::result::Result<u32, String>,
}

#[derive(Clone, Debug)]
pub struct RegisterWrite {
    pub now: u64,
    pub cpu: i64,
    pub address: u64,
    pub size: u8,
    pub value: u64,
    pub result: std::result::Result<(), String>,
    pub memory_reads: Vec<MemoryRead>,
    pub target_samples: Vec<TargetSample>,
}

#[derive(Clone, Debug)]
pub struct OutputDigest {
    pub address: u64,
    pub length: u64,
    pub sha256: [u8; 32],
}

#[derive(Clone, Debug)]
pub struct Completion {
    pub now: u64,
    pub token: u64,
    pub outputs: Vec<OutputDigest>,
}

#[derive(Clone, Debug)]
pub enum Event {
    RegisterRead(RegisterRead),
    RegisterWrite(RegisterWrite),
    Completion(Completion),
}

pub struct Recorder {
    writer: BufWriter<File>,
    finished: bool,
}

impl Recorder {
    pub fn create(path: &Path, header: &Header) -> io::Result<Self> {
        let file = OpenOptions::new().write(true).create_new(true).open(path)?;
        let mut recorder = Self {
            writer: BufWriter::new(file),
            finished: false,
        };
        recorder.writer.write_all(MAGIC_V2)?;
        recorder.write_header(header)?;
        recorder.writer.flush()?;
        Ok(recorder)
    }

    fn write_header(&mut self, header: &Header) -> io::Result<()> {
        for value in [
            header.ram_bytes,
            header.max_fill_bytes,
            header.max_copy_bytes,
            header.max_command_capture_bytes,
            header.max_clear_bytes,
            header.bytes_per_tick,
        ] {
            self.writer.write_all(&value.to_le_bytes())?;
        }
        self.writer.write_all(&[profile_tag(header.copy_profile)])?;
        self.writer
            .write_all(&[profile_tag(header.clear_profile)])?;
        Ok(())
    }

    pub fn register_read(&mut self, event: &RegisterRead) -> io::Result<()> {
        self.write_frame(|record| {
            record.write_all(&[1])?;
            record.write_all(&event.now.to_le_bytes())?;
            record.write_all(&event.cpu.to_le_bytes())?;
            record.write_all(&event.address.to_le_bytes())?;
            record.write_all(&[event.size])?;
            match &event.result {
                Ok(value) => {
                    record.write_all(&[0])?;
                    record.write_all(&value.to_le_bytes())?;
                }
                Err(error) => {
                    record.write_all(&[1])?;
                    write_string(record, error)?;
                }
            }
            Ok(())
        })
    }

    pub fn register_write(&mut self, event: &RegisterWrite) -> io::Result<()> {
        self.write_frame(|record| {
            record.write_all(&[2])?;
            record.write_all(&event.now.to_le_bytes())?;
            record.write_all(&event.cpu.to_le_bytes())?;
            record.write_all(&event.address.to_le_bytes())?;
            record.write_all(&[event.size])?;
            record.write_all(&event.value.to_le_bytes())?;
            match &event.result {
                Ok(()) => record.write_all(&[0])?,
                Err(error) => {
                    record.write_all(&[1])?;
                    write_string(record, error)?;
                }
            }
            write_count(record, event.memory_reads.len())?;
            for read in &event.memory_reads {
                record.write_all(&read.cpu.to_le_bytes())?;
                record.write_all(&read.address.to_le_bytes())?;
                record.write_all(&read.length.to_le_bytes())?;
                match &read.result {
                    Ok(bytes) => {
                        record.write_all(&[0])?;
                        write_bytes(record, bytes)?;
                    }
                    Err(error) => {
                        record.write_all(&[1])?;
                        write_string(record, error)?;
                    }
                }
            }
            write_count(record, event.target_samples.len())?;
            for sample in &event.target_samples {
                match sample {
                    TargetSample::Routed { count, selected } => {
                        record.write_all(&[0])?;
                        record.write_all(&usize_to_u64(*count)?.to_le_bytes())?;
                        record.write_all(&selected.to_le_bytes())?;
                    }
                    TargetSample::Failed(error) => {
                        record.write_all(&[1])?;
                        write_string(record, error)?;
                    }
                }
            }
            Ok(())
        })?;
        if is_submission(event.address, event.value) {
            self.writer.flush()?;
        }
        Ok(())
    }

    pub fn completion(
        &mut self,
        now: u64,
        token: u64,
        output: &[(u64, Arc<Vec<u8>>)],
    ) -> io::Result<()> {
        self.write_frame(|record| {
            record.write_all(&[3])?;
            record.write_all(&now.to_le_bytes())?;
            record.write_all(&token.to_le_bytes())?;
            write_count(record, output.len())?;
            for (address, bytes) in output {
                record.write_all(&address.to_le_bytes())?;
                record.write_all(&usize_to_u64(bytes.len())?.to_le_bytes())?;
                record.write_all(&Sha256::digest(bytes.as_slice()))?;
            }
            Ok(())
        })
    }

    fn write_frame(
        &mut self,
        encode: impl FnOnce(&mut Vec<u8>) -> io::Result<()>,
    ) -> io::Result<()> {
        let mut record = Vec::new();
        encode(&mut record)?;
        let length = usize_to_u64(record.len())?;
        self.writer.write_all(&length.to_le_bytes())?;
        self.writer.write_all(&record)
    }

    fn finish(&mut self) -> io::Result<()> {
        if !self.finished {
            self.writer.write_all(&0u64.to_le_bytes())?;
            self.writer.flush()?;
            self.finished = true;
        }
        Ok(())
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

pub struct RecordingReader {
    reader: BufReader<File>,
    pub header: Header,
    framed: bool,
    ended: bool,
}

impl RecordingReader {
    pub fn open(path: &Path) -> io::Result<Self> {
        let mut reader = BufReader::new(File::open(path)?);
        let mut magic = [0; 8];
        reader.read_exact(&mut magic)?;
        let framed = if &magic == MAGIC_V1 {
            false
        } else if &magic == MAGIC_V2 {
            true
        } else {
            return Err(invalid_data("not a supported vector GPU recording"));
        };
        let header = Header {
            ram_bytes: read_u64(&mut reader)?,
            max_fill_bytes: read_u64(&mut reader)?,
            max_copy_bytes: read_u64(&mut reader)?,
            max_command_capture_bytes: read_u64(&mut reader)?,
            max_clear_bytes: read_u64(&mut reader)?,
            bytes_per_tick: read_u64(&mut reader)?,
            copy_profile: read_profile(&mut reader)?,
            clear_profile: read_profile(&mut reader)?,
        };
        Ok(Self {
            reader,
            header,
            framed,
            ended: false,
        })
    }

    pub fn next_event(&mut self) -> io::Result<Option<Event>> {
        if self.ended {
            return Ok(None);
        }
        if self.framed {
            self.next_framed_event()
        } else {
            self.next_legacy_event()
        }
    }

    fn next_framed_event(&mut self) -> io::Result<Option<Event>> {
        let mut length_bytes = [0; 8];
        match self.reader.read(&mut length_bytes[..1]) {
            Ok(0) => {
                self.ended = true;
                return Ok(None);
            }
            Ok(1) => {}
            Ok(_) => unreachable!("one-byte read returned more than one byte"),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {
                return self.next_framed_event();
            }
            Err(error) => return Err(error),
        }
        if let Err(error) = self.reader.read_exact(&mut length_bytes[1..]) {
            if error.kind() == io::ErrorKind::UnexpectedEof {
                self.ended = true;
                return Ok(None);
            }
            return Err(error);
        }
        let length = u64::from_le_bytes(length_bytes);
        if length == 0 {
            self.ended = true;
            return Ok(None);
        }
        if length > MAX_RECORD_BYTES {
            return Err(invalid_data(
                "vector GPU recording record exceeds its bound",
            ));
        }
        let mut record = (&mut self.reader).take(length);
        match read_event(&mut record) {
            Ok(event) if record.limit() == 0 => Ok(Some(event)),
            Ok(_) => Err(invalid_data(
                "vector GPU recording record has trailing bytes",
            )),
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
                self.ended = true;
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    fn next_legacy_event(&mut self) -> io::Result<Option<Event>> {
        let tag = match read_u8(&mut self.reader) {
            Ok(0) => {
                self.ended = true;
                return Ok(None);
            }
            Ok(tag) => tag,
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
                self.ended = true;
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        match read_event_body(&mut self.reader, tag) {
            Ok(event) => Ok(Some(event)),
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
                self.ended = true;
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }
}

fn read_event(reader: &mut impl Read) -> io::Result<Event> {
    let tag = read_u8(reader)?;
    read_event_body(reader, tag)
}

fn read_event_body(reader: &mut impl Read, tag: u8) -> io::Result<Event> {
    match tag {
        1 => {
            let now = read_u64(reader)?;
            let cpu = read_i64(reader)?;
            let address = read_u64(reader)?;
            let size = read_u8(reader)?;
            let result = match read_u8(reader)? {
                0 => Ok(read_u32(reader)?),
                1 => Err(read_string(reader)?),
                _ => return Err(invalid_data("invalid vector GPU read result tag")),
            };
            Ok(Event::RegisterRead(RegisterRead {
                now,
                cpu,
                address,
                size,
                result,
            }))
        }
        2 => {
            let now = read_u64(reader)?;
            let cpu = read_i64(reader)?;
            let address = read_u64(reader)?;
            let size = read_u8(reader)?;
            let value = read_u64(reader)?;
            let result = match read_u8(reader)? {
                0 => Ok(()),
                1 => Err(read_string(reader)?),
                _ => return Err(invalid_data("invalid vector GPU write result tag")),
            };
            let memory_count = read_count(reader)?;
            let mut memory_reads = Vec::with_capacity(memory_count);
            for _ in 0..memory_count {
                let cpu = read_i64(reader)?;
                let address = read_u64(reader)?;
                let length = read_u64(reader)?;
                let result = match read_u8(reader)? {
                    0 => Ok(read_bytes(reader)?),
                    1 => Err(read_string(reader)?),
                    _ => return Err(invalid_data("invalid vector GPU DMA read tag")),
                };
                memory_reads.push(MemoryRead {
                    cpu,
                    address,
                    length,
                    result,
                });
            }
            let target_count = read_count(reader)?;
            let mut target_samples = Vec::with_capacity(target_count);
            for _ in 0..target_count {
                target_samples.push(match read_u8(reader)? {
                    0 => TargetSample::Routed {
                        count: u64_to_usize(read_u64(reader)?)?,
                        selected: read_i64(reader)?,
                    },
                    1 => TargetSample::Failed(read_string(reader)?),
                    _ => return Err(invalid_data("invalid vector GPU IRQ target tag")),
                });
            }
            Ok(Event::RegisterWrite(RegisterWrite {
                now,
                cpu,
                address,
                size,
                value,
                result,
                memory_reads,
                target_samples,
            }))
        }
        3 => {
            let now = read_u64(reader)?;
            let token = read_u64(reader)?;
            let output_count = read_count(reader)?;
            let mut outputs = Vec::with_capacity(output_count);
            for _ in 0..output_count {
                let address = read_u64(reader)?;
                let length = read_u64(reader)?;
                let mut sha256 = [0; 32];
                reader.read_exact(&mut sha256)?;
                outputs.push(OutputDigest {
                    address,
                    length,
                    sha256,
                });
            }
            Ok(Event::Completion(Completion {
                now,
                token,
                outputs,
            }))
        }
        _ => Err(invalid_data("unknown vector GPU recording event")),
    }
}

fn is_submission(address: u64, value: u64) -> bool {
    value != 0
        && crate::offset(address, 4).is_ok_and(|offset| {
            crate::TRIGGERS.contains(&offset) || [0x1c, 0xc18].contains(&offset)
        })
}

fn profile_tag(profile: Option<Profile>) -> u8 {
    match profile {
        None => 0,
        Some(Profile::GpuCopyProfile) => 1,
        Some(Profile::GpuClearProfile) => 2,
        Some(Profile::GpuUiProfile) => 3,
    }
}

fn read_profile(reader: &mut impl Read) -> io::Result<Option<Profile>> {
    match read_u8(reader)? {
        0 => Ok(None),
        1 => Ok(Some(Profile::GpuCopyProfile)),
        2 => Ok(Some(Profile::GpuClearProfile)),
        3 => Ok(Some(Profile::GpuUiProfile)),
        _ => Err(invalid_data("invalid vector GPU profile tag")),
    }
}

fn write_count(writer: &mut impl Write, count: usize) -> io::Result<()> {
    let count = u32::try_from(count).map_err(|_| invalid_input("record has too many entries"))?;
    writer.write_all(&count.to_le_bytes())
}

fn read_count(reader: &mut impl Read) -> io::Result<usize> {
    let count = read_u32(reader)?;
    if count > 1_000_000 {
        return Err(invalid_data("vector GPU recording count exceeds its bound"));
    }
    Ok(count as usize)
}

fn write_bytes(writer: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    writer.write_all(&usize_to_u64(bytes.len())?.to_le_bytes())?;
    writer.write_all(bytes)
}

fn read_bytes(reader: &mut impl Read) -> io::Result<Vec<u8>> {
    let length = read_u64(reader)?;
    let length = usize::try_from(length).map_err(|_| invalid_data("DMA read is too large"))?;
    if length > 0x40000000 {
        return Err(invalid_data("DMA read exceeds the device memory bound"));
    }
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn write_string(writer: &mut impl Write, value: &str) -> io::Result<()> {
    let bytes = value.as_bytes();
    let length =
        u32::try_from(bytes.len()).map_err(|_| invalid_input("record string is too long"))?;
    writer.write_all(&length.to_le_bytes())?;
    writer.write_all(bytes)
}

fn read_string(reader: &mut impl Read) -> io::Result<String> {
    let length = read_u32(reader)? as usize;
    if length > 1_048_576 {
        return Err(invalid_data(
            "vector GPU recording string exceeds its bound",
        ));
    }
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes)?;
    String::from_utf8(bytes).map_err(|_| invalid_data("record string is not UTF-8"))
}

fn usize_to_u64(value: usize) -> io::Result<u64> {
    u64::try_from(value).map_err(|_| invalid_input("record value exceeds 64 bits"))
}

fn u64_to_usize(value: u64) -> io::Result<usize> {
    usize::try_from(value).map_err(|_| invalid_data("record value exceeds host address space"))
}

fn read_u8(reader: &mut impl Read) -> io::Result<u8> {
    let mut bytes = [0; 1];
    reader.read_exact(&mut bytes)?;
    Ok(bytes[0])
}

fn read_u32(reader: &mut impl Read) -> io::Result<u32> {
    let mut bytes = [0; 4];
    reader.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_u64(reader: &mut impl Read) -> io::Result<u64> {
    let mut bytes = [0; 8];
    reader.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

fn read_i64(reader: &mut impl Read) -> io::Result<i64> {
    let mut bytes = [0; 8];
    reader.read_exact(&mut bytes)?;
    Ok(i64::from_le_bytes(bytes))
}

fn invalid_data(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn invalid_input(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static NEXT_PATH: AtomicU64 = AtomicU64::new(0);

    fn path() -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "fujimod-gpu-recording-{}-{nonce}-{}.vgpu",
            std::process::id(),
            NEXT_PATH.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn header() -> Header {
        Header {
            ram_bytes: 4096,
            max_fill_bytes: 4096,
            max_copy_bytes: 4096,
            max_command_capture_bytes: 4096,
            max_clear_bytes: 4096,
            bytes_per_tick: 16,
            copy_profile: None,
            clear_profile: None,
        }
    }

    fn register_read(now: u64, value: u32) -> RegisterRead {
        RegisterRead {
            now,
            cpu: 0,
            address: crate::BASE + 0x34 * 2,
            size: 4,
            result: Ok(value),
        }
    }

    #[test]
    fn submission_flushes_a_complete_framed_record() {
        let path = path();
        let mut recorder = Recorder::create(&path, &header()).unwrap();
        let write = RegisterWrite {
            now: 10,
            cpu: 0,
            address: crate::BASE + 0x10d0 * 2,
            size: 4,
            value: 1,
            result: Ok(()),
            memory_reads: Vec::new(),
            target_samples: Vec::new(),
        };
        recorder.register_write(&write).unwrap();

        let mut reader = RecordingReader::open(&path).unwrap();
        assert!(matches!(
            reader.next_event().unwrap(),
            Some(Event::RegisterWrite(_))
        ));
        assert!(reader.next_event().unwrap().is_none());
        drop(reader);
        drop(recorder);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn framed_reader_stops_before_a_truncated_final_record() {
        let path = path();
        let mut recorder = Recorder::create(&path, &header()).unwrap();
        recorder.register_read(&register_read(1, 7)).unwrap();
        recorder.register_read(&register_read(2, 8)).unwrap();
        recorder.finish().unwrap();
        drop(recorder);

        let length = std::fs::metadata(&path).unwrap().len();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(length - 8 - 3)
            .unwrap();
        let mut reader = RecordingReader::open(&path).unwrap();
        assert!(matches!(
            reader.next_event().unwrap(),
            Some(Event::RegisterRead(_))
        ));
        assert!(reader.next_event().unwrap().is_none());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn legacy_reader_stops_before_a_truncated_final_record() {
        let path = path();
        let mut file = File::create(&path).unwrap();
        file.write_all(MAGIC_V1).unwrap();
        for value in [4096u64, 4096, 4096, 4096, 4096, 16] {
            file.write_all(&value.to_le_bytes()).unwrap();
        }
        file.write_all(&[0, 0]).unwrap();
        file.write_all(&[1]).unwrap();
        file.write_all(&1u64.to_le_bytes()).unwrap();
        file.write_all(&0i64.to_le_bytes()).unwrap();
        file.write_all(&(crate::BASE + 0x34 * 2).to_le_bytes())
            .unwrap();
        file.write_all(&[4, 0]).unwrap();
        file.write_all(&7u32.to_le_bytes()).unwrap();
        file.write_all(&[1, 2, 3]).unwrap();
        drop(file);

        let mut reader = RecordingReader::open(&path).unwrap();
        assert!(matches!(
            reader.next_event().unwrap(),
            Some(Event::RegisterRead(_))
        ));
        assert!(reader.next_event().unwrap().is_none());
        std::fs::remove_file(path).unwrap();
    }
}
