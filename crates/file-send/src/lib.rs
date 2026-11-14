//! File transfer over the photo sync exchange (`protocol/photo-sync.proto`).
//!
//! The sender plays the camera's part: `Hello`, one `ManifestBatch`, then
//! `ChunkHeader` frames and a `FileEnd` with CRC32 and SHA-256 for each file the
//! receiver wants. The receiver answers `Welcome`, `Wants` with a resume offset
//! per file, and `Committed` once a file is verified and in place.
//!
//! [`sync`] adds a long-lived directory exchange. The server offers a complete
//! snapshot, transfers what the receiver lacks, and offers a new snapshot when
//! the directory changes. `tree` scans the offered directory; `sync` applies
//! snapshots on the receiver.
use fujimod_sync_protocol::{
    ChunkHeader, Committed, FileEnd, Frame, Hello, ManifestBatch, ManifestEntry, Wants, Welcome,
    committed::Status, decode, encode, frame::Body,
};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs::{File, Metadata},
    future::Future,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::SystemTime,
};

mod sync;
mod tree;
pub use sync::{SyncSummary, SyncTarget, prepare_sync_root, receive_snapshot, serve_peer, sync};
pub use tree::DirectorySource;

pub const PROTOCOL_VERSION: u32 = 1;
/// Raw file bytes per chunk message.
pub const CHUNK_BYTES: u32 = 1024 * 1024;
const CAPABILITIES: [&str; 2] = ["files", "sha256"];
pub(crate) const SYNC_CAPABILITY: &str = "sync";
pub(crate) const SYNC_MARKER: &str = ".fujimod-sync";
pub(crate) const SYNC_PARTS: &str = ".fujimod-sync-parts";
pub(crate) const SYNC_PARTS_PREFIX: &str = ".fujimod-sync-parts/";
const MANIFEST_BATCH: usize = 1024;

pub type Result<T> = std::result::Result<T, String>;

/// The receiver binary identity reported to the sender.
pub fn build_info() -> String {
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    format!(
        "fujimod-send {} {profile} {}-{}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

/// The receiver destination path and free space, when available.
pub fn path_info(path: &Path) -> String {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    match free_bytes(&absolute) {
        Some(free) => format!(
            "destination {} ({} free)",
            absolute.display(),
            format_bytes(free)
        ),
        None => format!("destination {}", absolute.display()),
    }
}

fn format_bytes(bytes: u64) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    const GIB: f64 = 1024.0 * MIB;
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.1} GiB", bytes as f64 / GIB)
    } else {
        format!("{:.1} MiB", bytes as f64 / MIB)
    }
}

#[cfg(unix)]
fn free_bytes(path: &Path) -> Option<u64> {
    use nix::sys::statvfs::statvfs;
    let stats = statvfs(path).ok()?;
    Some(stats.fragment_size() as u64 * stats.blocks_available() as u64)
}

#[cfg(not(unix))]
fn free_bytes(_: &Path) -> Option<u64> {
    None
}

/// A reliable, ordered message pipe, such as a WebRTC data channel.
pub trait Pipe {
    fn send(&mut self, message: Vec<u8>) -> impl Future<Output = Result<()>>;
    fn receive(&mut self) -> impl Future<Output = Result<Vec<u8>>>;
}

impl Pipe for fujimod_webrtc::PeerSocket {
    async fn send(&mut self, message: Vec<u8>) -> Result<()> {
        fujimod_webrtc::PeerSocket::send(self, message)
            .await
            .map_err(|e| e.to_string())
    }
    async fn receive(&mut self) -> Result<Vec<u8>> {
        fujimod_webrtc::PeerSocket::receive(self)
            .await
            .map_err(|e| e.to_string())
    }
}

/// Progress reports: (path, bytes of that file done, its size).
pub type Report<'a> = &'a mut dyn FnMut(&str, u64, u64);

pub(crate) async fn put(pipe: &mut impl Pipe, body: Body, payload: &[u8]) -> Result<()> {
    let frame = Frame {
        id: 0,
        body: Some(body),
    };
    pipe.send(encode(&frame, payload).map_err(str::to_owned)?)
        .await
}

pub(crate) async fn take(pipe: &mut impl Pipe) -> Result<(Body, Vec<u8>)> {
    let message = pipe.receive().await?;
    let (frame, payload) = decode(&message).map_err(str::to_owned)?;
    let payload = payload.to_vec();
    match frame.body {
        Some(Body::Error(error)) => Err(format!("peer error: {}", error.message)),
        Some(body) => Ok((body, payload)),
        None => Err("missing frame body".into()),
    }
}

/// One file or directory to send, named by its path on the receiver's side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    /// Slash-separated path relative to the offered root.
    pub path: String,
    /// The file or directory on this computer.
    pub source: PathBuf,
    pub size: u64,
    pub directory: bool,
    /// Whole-file SHA-256 when known; empty for a one-shot send.
    pub sha256: Vec<u8>,
}

/// Name each file by its final path component; names must be unique.
pub fn sources(paths: &[PathBuf]) -> Result<Vec<Source>> {
    let mut seen = HashSet::new();
    paths
        .iter()
        .map(|path| {
            if !std::fs::metadata(path)
                .map_err(|e| format!("{}: {e}", path.display()))?
                .is_file()
            {
                return Err(format!("{}: not a regular file", path.display()));
            }
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| format!("{}: name is not UTF-8", path.display()))?
                .to_owned();
            if !seen.insert(name.clone()) {
                return Err(format!("two files are named {name}"));
            }
            let size = std::fs::metadata(path)
                .map_err(|e| format!("{}: {e}", path.display()))?
                .len();
            Ok(Source {
                path: name,
                source: path.clone(),
                size,
                directory: false,
                sha256: Vec::new(),
            })
        })
        .collect()
}

/// Tell the peer why this side is stopping, then return the failure.
pub(crate) async fn fail<T>(pipe: &mut impl Pipe, result: Result<T>) -> Result<T> {
    if let Err(message) = &result
        && !message.starts_with("peer error: ")
    {
        let error = fujimod_sync_protocol::Error {
            code: 1,
            message: message.clone(),
        };
        let _ = put(pipe, Body::Error(error), &[]).await;
    }
    result
}

/// Send `files` and return once the receiver has committed every wanted file.
pub async fn send(pipe: &mut impl Pipe, files: &[Source], report: Report<'_>) -> Result<()> {
    let result = send_files(pipe, files, report).await;
    fail(pipe, result).await
}

async fn send_files(pipe: &mut impl Pipe, files: &[Source], report: Report<'_>) -> Result<()> {
    put(
        pipe,
        Body::Hello(Hello {
            protocol_version: PROTOCOL_VERSION,
            capabilities: CAPABILITIES.map(str::to_owned).to_vec(),
            ..Default::default()
        }),
        &[],
    )
    .await?;
    let (Body::Welcome(welcome), _) = take(pipe).await? else {
        return Err("expected Welcome".into());
    };
    if welcome.protocol_version != PROTOCOL_VERSION {
        return Err(format!(
            "receiver speaks version {}",
            welcome.protocol_version
        ));
    }
    if !welcome.client.is_empty() {
        eprintln!("receiver: {}", welcome.client);
    }
    let chunk = welcome.max_chunk_bytes.clamp(1, CHUNK_BYTES) as usize;
    let entries: Vec<ManifestEntry> = files.iter().map(manifest_entry).collect();
    send_manifest(pipe, entries, false).await?;
    let (Body::Wants(wants), _) = take(pipe).await? else {
        return Err("expected Wants".into());
    };
    transfer_wanted(pipe, files, &wants, chunk, report).await
}

/// Send a manifest in bounded batches. Only the final batch is `last`, and a
/// complete snapshot marks only that final batch as `complete`.
pub(crate) async fn send_manifest(
    pipe: &mut impl Pipe,
    entries: Vec<ManifestEntry>,
    complete: bool,
) -> Result<()> {
    if entries.is_empty() {
        return put(
            pipe,
            Body::Manifest(ManifestBatch {
                entries: Vec::new(),
                last: true,
                complete,
            }),
            &[],
        )
        .await;
    }
    let count = entries.len().div_ceil(MANIFEST_BATCH);
    for (index, chunk) in entries.chunks(MANIFEST_BATCH).enumerate() {
        let last = index + 1 == count;
        put(
            pipe,
            Body::Manifest(ManifestBatch {
                entries: chunk.to_vec(),
                last,
                complete: complete && last,
            }),
            &[],
        )
        .await?;
    }
    Ok(())
}

pub(crate) fn manifest_entry(source: &Source) -> ManifestEntry {
    ManifestEntry {
        path: source.path.clone(),
        size_bytes: source.size,
        fat_timestamp: 0,
        directory: source.directory,
        sha256: source.sha256.clone(),
    }
}

/// Send every wanted file and wait for each file's commit before starting the
/// next one. Waiting per file keeps the two directions from filling a bounded
/// pipe during a long directory transfer.
pub(crate) async fn transfer_wanted(
    pipe: &mut impl Pipe,
    files: &[Source],
    wants: &Wants,
    chunk: usize,
    report: Report<'_>,
) -> Result<()> {
    let sources: HashMap<&str, &Source> = files
        .iter()
        .map(|source| (source.path.as_str(), source))
        .collect();
    for want in &wants.entries {
        let source = sources
            .get(want.path.as_str())
            .copied()
            .ok_or_else(|| format!("receiver wants unknown file {}", want.path))?;
        if source.directory {
            return Err(format!("{}: receiver wants a directory", want.path));
        }
        let size = source.size;
        if want.resume_offset > size {
            return Err(format!("{}: resume offset past the end", want.path));
        }
        let mut input = File::open(&source.source).map_err(|e| format!("{}: {e}", want.path))?;
        if input.metadata().map_err(|e| e.to_string())?.len() != size {
            return Err(format!("{}: changed since the manifest", want.path));
        }
        let mut crc = crc32fast::Hasher::new();
        let mut sha = Sha256::new();
        let mut buffer = vec![0; chunk];
        // The digests cover the whole file, including bytes the receiver has.
        let mut offset = 0;
        while offset < size {
            let length = (size - offset).min(chunk as u64) as usize;
            input
                .read_exact(&mut buffer[..length])
                .map_err(|e| format!("{}: {e}", want.path))?;
            crc.update(&buffer[..length]);
            sha.update(&buffer[..length]);
            if offset + length as u64 > want.resume_offset {
                let skip = want.resume_offset.saturating_sub(offset) as usize;
                put(
                    pipe,
                    Body::Chunk(ChunkHeader {
                        path: want.path.clone(),
                        offset: offset + skip as u64,
                        length: (length - skip) as u32,
                    }),
                    &buffer[skip..length],
                )
                .await?;
            }
            offset += length as u64;
            report(&want.path, offset, size);
        }
        let digest = sha.finalize().to_vec();
        if !source.sha256.is_empty() && source.sha256 != digest {
            return Err(format!("{}: changed while it was being sent", want.path));
        }
        put(
            pipe,
            Body::FileEnd(FileEnd {
                path: want.path.clone(),
                total_bytes: size,
                crc32: crc.finalize(),
                sha256: digest,
            }),
            &[],
        )
        .await?;
        match take(pipe).await? {
            (Body::Committed(done), _) if done.path == want.path => {
                if done.status() != Status::Stored {
                    return Err(format!("{}: receiver did not store it", want.path));
                }
            }
            _ => return Err(format!("{}: expected Committed", want.path)),
        }
    }
    Ok(())
}

/// Where received files go.
pub enum Destination {
    /// Each file at its name inside this directory. A `NAME.part` file left
    /// by an interrupted transfer is resumed.
    Directory(PathBuf),
    /// Standard output; the sender must offer exactly one file.
    Stdout,
}

/// A one-line description of where a receiver stores files.
pub fn destination_info(destination: &Destination) -> String {
    match destination {
        Destination::Directory(path) => path_info(path),
        Destination::Stdout => "destination standard output".into(),
    }
}

/// A slash-separated path that stays inside the destination directory.
pub(crate) fn safe_path(name: &str) -> Result<&str> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name == SYNC_MARKER
        || name == SYNC_PARTS
        || name.starts_with(SYNC_PARTS_PREFIX)
        || name.starts_with('/')
        || name.ends_with('/')
        || name.contains(['\\', '\0', ':'])
        || name.chars().any(char::is_control)
    {
        return Err(format!("refusing path {name:?}"));
    }
    for segment in name.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(format!("refusing path {name:?}"));
        }
    }
    Ok(name)
}

struct Incoming {
    name: String,
    size: u64,
    part: Option<PathBuf>,
    meta: Option<PathBuf>,
    expected_sha256: Vec<u8>,
    output: Box<dyn Write>,
    written: u64,
    crc: crc32fast::Hasher,
    sha: Sha256,
}

/// Hash the bytes of a partial file and open it for appending.
fn resume(part: &Path, size: u64) -> Result<(u64, File, crc32fast::Hasher, Sha256)> {
    let mut crc = crc32fast::Hasher::new();
    let mut sha = Sha256::new();
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .open(part)
        .map_err(|e| format!("{}: {e}", part.display()))?;
    let mut length = file.metadata().map_err(|e| e.to_string())?.len();
    if length > size {
        file.set_len(0).map_err(|e| e.to_string())?;
        length = 0;
    }
    file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let mut buffer = vec![0; CHUNK_BYTES as usize];
    let mut remaining = length;
    while remaining > 0 {
        let read = file
            .read(&mut buffer[..remaining.min(CHUNK_BYTES as u64) as usize])
            .map_err(|e| e.to_string())?;
        if read == 0 {
            return Err(format!("{}: shrank while reading", part.display()));
        }
        crc.update(&buffer[..read]);
        sha.update(&buffer[..read]);
        remaining -= read as u64;
    }
    Ok((length, file, crc, sha))
}

pub(crate) fn hash_file(path: &Path) -> Result<Vec<u8>> {
    for _ in 0..3 {
        let before = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut sha = Sha256::new();
        let mut buffer = vec![0; CHUNK_BYTES as usize];
        loop {
            let read = file
                .read(&mut buffer)
                .map_err(|e| format!("{}: {e}", path.display()))?;
            if read == 0 {
                break;
            }
            sha.update(&buffer[..read]);
        }
        let after = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if same_file(&before, &after) {
            return Ok(sha.finalize().to_vec());
        }
    }
    Err(format!("{}: changed while being read", path.display()))
}

fn same_file(before: &Metadata, after: &Metadata) -> bool {
    before.is_file()
        && after.is_file()
        && before.len() == after.len()
        && before.modified().ok() == after.modified().ok()
}

pub(crate) fn modified(metadata: &Metadata) -> Option<SystemTime> {
    metadata.modified().ok()
}

/// Receive every offered file into `destination`. Returns the names stored.
pub async fn receive(
    pipe: &mut impl Pipe,
    destination: &Destination,
    report: Report<'_>,
) -> Result<Vec<String>> {
    let result = receive_files(pipe, destination, report).await;
    fail(pipe, result).await
}

async fn receive_files(
    pipe: &mut impl Pipe,
    destination: &Destination,
    report: Report<'_>,
) -> Result<Vec<String>> {
    welcome(pipe, false, &destination_info(destination)).await?;
    let entries = receive_manifest(pipe, false).await?;
    let mut names = HashSet::new();
    for entry in &entries {
        if !names.insert(safe_path(&entry.path)?) {
            return Err(format!("sender offers {} twice", entry.path));
        }
    }
    let mut incoming = Vec::new();
    match destination {
        Destination::Stdout => {
            if entries.len() != 1 || entries[0].directory {
                return Err(format!(
                    "standard output takes one file; the sender offers {}",
                    entries.len()
                ));
            }
            incoming.push(Incoming {
                name: entries[0].path.clone(),
                size: entries[0].size_bytes,
                part: None,
                meta: None,
                expected_sha256: Vec::new(),
                output: Box::new(std::io::stdout().lock()),
                written: 0,
                crc: crc32fast::Hasher::new(),
                sha: Sha256::new(),
            });
        }
        Destination::Directory(directory) => {
            for entry in &entries {
                let path = safe_path(&entry.path)?;
                let finished = directory.join(path);
                if entry.directory {
                    std::fs::create_dir_all(&finished)
                        .map_err(|e| format!("{}: {e}", finished.display()))?;
                    continue;
                }
                if finished.exists() {
                    return Err(format!("{} already exists", finished.display()));
                }
                if let Some(parent) = finished.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| format!("{}: {e}", parent.display()))?;
                }
                let part = appended(&finished, ".part");
                let (written, file, crc, sha) = resume(&part, entry.size_bytes)?;
                incoming.push(Incoming {
                    name: entry.path.clone(),
                    size: entry.size_bytes,
                    part: Some(part),
                    meta: None,
                    expected_sha256: Vec::new(),
                    output: Box::new(file),
                    written,
                    crc,
                    sha,
                });
            }
        }
    }
    put(
        pipe,
        Body::Wants(Wants {
            entries: incoming
                .iter()
                .map(|file| fujimod_sync_protocol::wants::Entry {
                    path: file.name.clone(),
                    resume_offset: file.written,
                })
                .collect(),
        }),
        &[],
    )
    .await?;
    let mut stored = Vec::new();
    for mut file in incoming {
        report(&file.name, file.written, file.size);
        let end = receive_end(pipe, &mut file, report).await?;
        let sha = file.sha.finalize();
        let verified = end.total_bytes == file.size
            && file.written == file.size
            && end.crc32 == file.crc.finalize()
            && end.sha256 == sha.as_slice()
            && (file.expected_sha256.is_empty() || file.expected_sha256 == end.sha256);
        let mut status = Status::Failed;
        if verified {
            file.output
                .flush()
                .map_err(|e| format!("{}: {e}", file.name))?;
            drop(file.output);
            if let (Some(part), Destination::Directory(directory)) = (&file.part, destination) {
                let finished = directory.join(&file.name);
                if finished.exists() {
                    let _ = std::fs::remove_file(&finished);
                }
                std::fs::rename(part, finished).map_err(|e| format!("{}: {e}", file.name))?;
            }
            if let Some(meta) = &file.meta {
                let _ = std::fs::remove_file(meta);
            }
            status = Status::Stored;
        } else {
            drop(file.output);
            if let Some(part) = &file.part {
                // A corrupt prefix must not be resumed.
                let _ = std::fs::remove_file(part);
            }
            if let Some(meta) = &file.meta {
                let _ = std::fs::remove_file(meta);
            }
        }
        commit(pipe, &file.name, status).await?;
        if !verified {
            return Err(format!("{}: digest mismatch", file.name));
        }
        stored.push(file.name);
    }
    Ok(stored)
}

pub(crate) async fn welcome(
    pipe: &mut impl Pipe,
    require_sync: bool,
    client: &str,
) -> Result<usize> {
    let (Body::Hello(hello), _) = take(pipe).await? else {
        return Err("expected Hello".into());
    };
    if hello.protocol_version != PROTOCOL_VERSION {
        return Err(format!("sender speaks version {}", hello.protocol_version));
    }
    if !hello.capabilities.iter().any(|c| c == "sha256") {
        return Err("sender does not send SHA-256 digests".into());
    }
    if require_sync && !hello.capabilities.iter().any(|c| c == SYNC_CAPABILITY) {
        return Err("sender does not serve a live directory".into());
    }
    put(
        pipe,
        Body::Welcome(Welcome {
            protocol_version: PROTOCOL_VERSION,
            max_chunk_bytes: CHUNK_BYTES,
            window_bytes: 0,
            high_water_path: String::new(),
            client: client.to_owned(),
        }),
        &[],
    )
    .await?;
    Ok(CHUNK_BYTES as usize)
}

pub(crate) async fn receive_manifest(
    pipe: &mut impl Pipe,
    require_complete: bool,
) -> Result<Vec<ManifestEntry>> {
    let mut entries = Vec::new();
    let complete = loop {
        let (Body::Manifest(batch), _) = take(pipe).await? else {
            return Err("expected ManifestBatch".into());
        };
        entries.extend(batch.entries);
        if batch.last {
            break batch.complete;
        }
    };
    if require_complete && !complete {
        return Err("sender offered a partial manifest".into());
    }
    Ok(entries)
}

pub(crate) async fn receive_end(
    pipe: &mut impl Pipe,
    file: &mut Incoming,
    report: Report<'_>,
) -> Result<FileEnd> {
    loop {
        match take(pipe).await? {
            (Body::Chunk(chunk), payload) => {
                if chunk.path != file.name || chunk.offset != file.written {
                    return Err(format!("{}: chunk out of order", file.name));
                }
                if file.written + payload.len() as u64 > file.size {
                    return Err(format!("{}: more bytes than announced", file.name));
                }
                file.output
                    .write_all(&payload)
                    .map_err(|e| format!("{}: {e}", file.name))?;
                file.crc.update(&payload);
                file.sha.update(&payload);
                file.written += payload.len() as u64;
                report(&file.name, file.written, file.size);
            }
            (Body::FileEnd(end), _) if end.path == file.name => return Ok(end),
            (Body::Progress(_), _) => (),
            _ => return Err(format!("{}: unexpected frame", file.name)),
        }
    }
}

pub(crate) async fn commit(pipe: &mut impl Pipe, name: &str, status: Status) -> Result<()> {
    put(
        pipe,
        Body::Committed(Committed {
            path: name.to_owned(),
            status: status as i32,
            asset_reference: String::new(),
        }),
        &[],
    )
    .await
}

pub(crate) fn appended(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Lowercase hexadecimal.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests;
