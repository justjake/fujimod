use super::{Engine, Snapshots};
use sha2::{Digest, Sha256};
use std::cell::Cell;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const MAGIC: &[u8; 8] = b"FJDMP002";
const MAX_CAPTURE_BLOB_BYTES: u64 = 512 * 1024 * 1024;
const MAX_CAPTURE_ITEMS: usize = 1 << 20;
const MAX_COMMAND_WORDS: usize = 1024 * 1024 / 4;

static NEXT_CAPTURE: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static CAPTURE_SUPPRESSED: Cell<bool> = const { Cell::new(false) };
}

struct CaptureSuppression(bool);

impl CaptureSuppression {
    fn enter() -> Self {
        Self(CAPTURE_SUPPRESSED.with(|suppressed| suppressed.replace(true)))
    }
}

impl Drop for CaptureSuppression {
    fn drop(&mut self) {
        CAPTURE_SUPPRESSED.with(|suppressed| suppressed.set(self.0));
    }
}

struct Record {
    state: Vec<u8>,
    words: Vec<u32>,
    reads: Vec<(u64, Vec<u8>)>,
    snapshots: Vec<SnapshotDigest>,
    written_ranges: Vec<(u64, usize)>,
}

#[derive(PartialEq, Eq)]
struct SnapshotDigest {
    address: u64,
    length: u64,
    sha256: [u8; 32],
}

pub(super) fn capture_directory() -> Option<PathBuf> {
    if CAPTURE_SUPPRESSED.with(Cell::get) {
        return None;
    }
    std::env::var_os("FUJIMOD_DMP_CAPTURE_DIR").map(PathBuf::from)
}

pub(super) fn serialize_engine(engine: &Engine) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(engine, &mut bytes)
        .map_err(|error| format!("serialize DMP engine capture state: {error}"))?;
    Ok(bytes)
}

pub(super) fn write_capture(
    directory: &Path,
    state: &[u8],
    words: &[u32],
    reads: &[(u64, Vec<u8>)],
    snapshots: &Snapshots,
    written_ranges: &[(u64, usize)],
) -> Result<(), String> {
    fs::create_dir_all(directory)
        .map_err(|error| format!("create DMP capture directory: {error}"))?;
    let capture_id = NEXT_CAPTURE.fetch_add(1, Ordering::Relaxed);
    let stem = format!("list-{}-{capture_id:08}", std::process::id());
    let temporary_path = directory.join(format!("{stem}.tmp"));
    let capture_path = directory.join(format!("{stem}.dmpcap"));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary_path)
        .map_err(|error| format!("create DMP capture file: {error}"))?;
    let result = write_record(&mut file, state, words, reads, snapshots, written_ranges)
        .map_err(|error| format!("write DMP capture file: {error}"));
    drop(file);
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary_path);
        return Err(error);
    }
    fs::rename(&temporary_path, &capture_path)
        .map_err(|error| format!("publish DMP capture file: {error}"))
}

fn write_record(
    writer: &mut impl Write,
    state: &[u8],
    words: &[u32],
    reads: &[(u64, Vec<u8>)],
    snapshots: &Snapshots,
    written_ranges: &[(u64, usize)],
) -> io::Result<()> {
    writer.write_all(MAGIC)?;
    write_count(writer, words.len())?;
    write_count(writer, reads.len())?;
    write_count(writer, snapshots.len())?;
    write_count(writer, written_ranges.len())?;
    write_blob(writer, state)?;
    for word in words {
        writer.write_all(&word.to_le_bytes())?;
    }
    for (address, bytes) in reads {
        write_reader_range(writer, *address, bytes)?;
    }
    for (address, bytes) in snapshots {
        writer.write_all(&address.to_le_bytes())?;
        writer.write_all(&(bytes.len() as u64).to_le_bytes())?;
        let sha256 = Sha256::digest(bytes);
        writer.write_all(&sha256)?;
    }
    for (address, length) in written_ranges {
        writer.write_all(&address.to_le_bytes())?;
        writer.write_all(&(*length as u64).to_le_bytes())?;
    }
    Ok(())
}

fn write_count(writer: &mut impl Write, count: usize) -> io::Result<()> {
    let count = u32::try_from(count)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "capture count exceeds u32"))?;
    writer.write_all(&count.to_le_bytes())
}

fn write_blob(writer: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    writer.write_all(&(bytes.len() as u64).to_le_bytes())?;
    writer.write_all(bytes)
}

fn write_reader_range(writer: &mut impl Write, address: u64, bytes: &[u8]) -> io::Result<()> {
    writer.write_all(&address.to_le_bytes())?;
    write_blob(writer, bytes)
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

fn read_blob(reader: &mut impl Read) -> io::Result<Vec<u8>> {
    let length = read_u64(reader)?;
    if length > MAX_CAPTURE_BLOB_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "capture blob exceeds limit",
        ));
    }
    let length = usize::try_from(length)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "capture blob exceeds usize"))?;
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn read_reader_range(reader: &mut impl Read) -> io::Result<(u64, Vec<u8>)> {
    Ok((read_u64(reader)?, read_blob(reader)?))
}

fn read_snapshot_digest(reader: &mut impl Read) -> io::Result<SnapshotDigest> {
    let address = read_u64(reader)?;
    let length = read_u64(reader)?;
    if length > MAX_CAPTURE_BLOB_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "snapshot extent exceeds limit",
        ));
    }
    let mut sha256 = [0; 32];
    reader.read_exact(&mut sha256)?;
    Ok(SnapshotDigest {
        address,
        length,
        sha256,
    })
}

fn snapshot_digests(snapshots: &Snapshots) -> Vec<SnapshotDigest> {
    snapshots
        .iter()
        .map(|(address, bytes)| SnapshotDigest {
            address: *address,
            length: bytes.len() as u64,
            sha256: Sha256::digest(bytes).into(),
        })
        .collect()
}

fn read_record(path: &Path) -> Result<Record, String> {
    let mut file = File::open(path).map_err(|error| format!("open {}: {error}", path.display()))?;
    let mut magic = [0; 8];
    file.read_exact(&mut magic)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    if &magic != MAGIC {
        return Err(format!(
            "{} has an unsupported capture header",
            path.display()
        ));
    }
    let word_count = read_u32(&mut file).map_err(|error| error.to_string())? as usize;
    let read_count = read_u32(&mut file).map_err(|error| error.to_string())? as usize;
    let snapshot_count = read_u32(&mut file).map_err(|error| error.to_string())? as usize;
    let range_count = read_u32(&mut file).map_err(|error| error.to_string())? as usize;
    if word_count > MAX_COMMAND_WORDS
        || read_count > MAX_CAPTURE_ITEMS
        || snapshot_count > MAX_CAPTURE_ITEMS
        || range_count > MAX_CAPTURE_ITEMS
    {
        return Err(format!("{} exceeds capture item limits", path.display()));
    }
    let state = read_blob(&mut file).map_err(|error| error.to_string())?;
    let mut words = Vec::with_capacity(word_count);
    for _ in 0..word_count {
        words.push(read_u32(&mut file).map_err(|error| error.to_string())?);
    }
    let mut reads = Vec::with_capacity(read_count);
    for _ in 0..read_count {
        reads.push(read_reader_range(&mut file).map_err(|error| error.to_string())?);
    }
    let mut snapshots = Vec::with_capacity(snapshot_count);
    for _ in 0..snapshot_count {
        snapshots.push(read_snapshot_digest(&mut file).map_err(|error| error.to_string())?);
    }
    let mut written_ranges = Vec::with_capacity(range_count);
    for _ in 0..range_count {
        let address = read_u64(&mut file).map_err(|error| error.to_string())?;
        let length = usize::try_from(read_u64(&mut file).map_err(|error| error.to_string())?)
            .map_err(|_| format!("{} range exceeds usize", path.display()))?;
        written_ranges.push((address, length));
    }
    let mut trailing = [0];
    if file
        .read(&mut trailing)
        .map_err(|error| error.to_string())?
        != 0
    {
        return Err(format!("{} has trailing capture bytes", path.display()));
    }
    Ok(Record {
        state,
        words,
        reads,
        snapshots,
        written_ranges,
    })
}

fn replay_record(path: &Path) -> Result<(), String> {
    let record = read_record(path)?;
    let mut engine: Engine = ciborium::de::from_reader(record.state.as_slice())
        .map_err(|error| format!("restore DMP engine state from {}: {error}", path.display()))?;
    let verify_gpu = std::env::var("FUJIMOD_DMP_VERIFY_GPU").is_ok_and(|value| value == "1");
    let mut used_reads = vec![false; record.reads.len()];
    let mut reader = |address: u64, length: usize| {
        let end = address
            .checked_add(length as u64)
            .ok_or_else(|| format!("{} replay reader range overflows", path.display()))?;
        let Some((recorded_address, bytes)) =
            record.reads.iter().find(|(recorded_address, bytes)| {
                let Some(recorded_end) = recorded_address.checked_add(bytes.len() as u64) else {
                    return false;
                };
                address >= *recorded_address && end <= recorded_end
            })
        else {
            return Err(format!(
                "{} replay requested unrecorded reader range {address:#x}+{length:#x}",
                path.display()
            ));
        };
        let offset = usize::try_from(address - recorded_address)
            .map_err(|_| format!("{} replay reader offset exceeds usize", path.display()))?;
        let result = bytes[offset..offset + length].to_vec();
        for (index, (recorded_address, bytes)) in record.reads.iter().enumerate() {
            let Some(recorded_end) = recorded_address.checked_add(bytes.len() as u64) else {
                continue;
            };
            if address < *recorded_address || end > recorded_end {
                continue;
            }
            let offset = usize::try_from(address - recorded_address)
                .map_err(|_| format!("{} replay reader offset exceeds usize", path.display()))?;
            if bytes[offset..offset + length] == result {
                used_reads[index] = true;
            }
        }
        Ok(result)
    };
    let actual_snapshots = if verify_gpu {
        engine.execute_host_gpu_verified(&record.words, &mut reader)?
    } else {
        engine.execute_host_gpu(&record.words, &mut reader)?
    };
    drop(reader);
    if let Some(index) = used_reads.iter().position(|used| !used) {
        return Err(format!(
            "{} replay did not use recorded reader range {index} of {}",
            path.display(),
            used_reads.len(),
        ));
    }
    if snapshot_digests(&actual_snapshots) != record.snapshots {
        return Err(format!("{} replay snapshot hashes differ", path.display()));
    }
    if engine.written_ranges() != record.written_ranges {
        return Err(format!("{} replay written ranges differ", path.display()));
    }
    Ok(())
}

pub(super) fn replay_directory(directory: &Path) -> Result<usize, String> {
    let _suppression = CaptureSuppression::enter();
    let entries = fs::read_dir(directory).map_err(|error| {
        format!(
            "read DMP capture directory {}: {error}",
            directory.display()
        )
    })?;
    let mut captures = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "dmpcap")
        })
        .collect::<Vec<_>>();
    captures.sort();
    if captures.is_empty() {
        return Err(format!("{} contains no DMP captures", directory.display()));
    }
    for path in &captures {
        replay_record(path)?;
    }
    Ok(captures.len())
}

#[cfg(test)]
mod tests {
    use super::write_record;

    #[test]
    fn snapshots_are_stored_as_digests() {
        let snapshots = vec![(0x1000, vec![0xa5; 2 * 1024 * 1024])];
        let mut capture = Vec::new();
        write_record(&mut capture, &[], &[], &[], &snapshots, &[]).unwrap();

        assert_eq!(capture.len(), 80);
    }
}
