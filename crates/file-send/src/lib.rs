//! File transfer over the photo sync exchange (`protocol/photo-sync.proto`).
//!
//! The sender plays the camera's part: `Hello`, one `ManifestBatch`, then
//! `ChunkHeader` frames and a `FileEnd` with CRC32 and SHA-256 for each file the
//! receiver wants. The receiver answers `Welcome`, `Wants` with a resume offset
//! per file, and `Committed` once a file is verified and in place.
use fujimod_sync_protocol::{
    ChunkHeader, Committed, FileEnd, Frame, Hello, ManifestBatch, ManifestEntry, Wants, Welcome,
    committed::Status, decode, encode, frame::Body,
};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::File,
    future::Future,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

pub const PROTOCOL_VERSION: u32 = 1;
/// Raw file bytes per chunk message.
pub const CHUNK_BYTES: u32 = 1024 * 1024;
const CAPABILITIES: [&str; 2] = ["files", "sha256"];

pub type Result<T> = std::result::Result<T, String>;

/// A reliable, ordered message pipe, such as a WebRTC data channel.
pub trait Pipe {
    fn send(&mut self, message: Vec<u8>) -> impl Future<Output = Result<()>>;
    fn receive(&mut self) -> impl Future<Output = Result<Vec<u8>>>;
}

impl Pipe for fujimod_usb_proxy::PeerSocket {
    async fn send(&mut self, message: Vec<u8>) -> Result<()> {
        fujimod_usb_proxy::PeerSocket::send(self, message)
            .await
            .map_err(|e| e.to_string())
    }
    async fn receive(&mut self) -> Result<Vec<u8>> {
        fujimod_usb_proxy::PeerSocket::receive(self)
            .await
            .map_err(|e| e.to_string())
    }
}

/// Progress reports: (path, bytes of that file done, its size).
pub type Report<'a> = &'a mut dyn FnMut(&str, u64, u64);

async fn put(pipe: &mut impl Pipe, body: Body, payload: &[u8]) -> Result<()> {
    let frame = Frame {
        id: 0,
        body: Some(body),
    };
    pipe.send(encode(&frame, payload).map_err(str::to_owned)?)
        .await
}

async fn take(pipe: &mut impl Pipe) -> Result<(Body, Vec<u8>)> {
    let message = pipe.receive().await?;
    let (frame, payload) = decode(&message).map_err(str::to_owned)?;
    let payload = payload.to_vec();
    match frame.body {
        Some(Body::Error(error)) => Err(format!("peer error: {}", error.message)),
        Some(body) => Ok((body, payload)),
        None => Err("missing frame body".into()),
    }
}

/// One file to send, named by its path on the receiver's side.
pub struct Source {
    pub name: String,
    pub path: PathBuf,
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
            Ok(Source {
                name,
                path: path.clone(),
            })
        })
        .collect()
}

/// Tell the peer why this side is stopping, then return the failure.
async fn fail<T>(pipe: &mut impl Pipe, result: Result<T>) -> Result<T> {
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
    let chunk = welcome.max_chunk_bytes.clamp(1, CHUNK_BYTES) as usize;
    let mut entries = Vec::new();
    for file in files {
        entries.push(ManifestEntry {
            path: file.name.clone(),
            size_bytes: std::fs::metadata(&file.path)
                .map_err(|e| format!("{}: {e}", file.path.display()))?
                .len(),
            fat_timestamp: 0,
        });
    }
    put(
        pipe,
        Body::Manifest(ManifestBatch {
            entries: entries.clone(),
            last: true,
        }),
        &[],
    )
    .await?;
    let (Body::Wants(wants), _) = take(pipe).await? else {
        return Err("expected Wants".into());
    };
    for want in &wants.entries {
        let index = entries
            .iter()
            .position(|entry| entry.path == want.path)
            .ok_or_else(|| format!("receiver wants unknown file {}", want.path))?;
        let size = entries[index].size_bytes;
        if want.resume_offset > size {
            return Err(format!("{}: resume offset past the end", want.path));
        }
        let mut input =
            File::open(&files[index].path).map_err(|e| format!("{}: {e}", want.path))?;
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
        put(
            pipe,
            Body::FileEnd(FileEnd {
                path: want.path.clone(),
                total_bytes: size,
                crc32: crc.finalize(),
                sha256: sha.finalize().to_vec(),
            }),
            &[],
        )
        .await?;
    }
    for want in &wants.entries {
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

/// A name that stays inside the destination directory.
fn safe_name(name: &str) -> Result<&str> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains(['/', '\\', '\0'])
        || name.chars().any(char::is_control)
    {
        return Err(format!("refusing file name {name:?}"));
    }
    Ok(name)
}

struct Incoming {
    name: String,
    size: u64,
    part: Option<PathBuf>,
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
    let (Body::Hello(hello), _) = take(pipe).await? else {
        return Err("expected Hello".into());
    };
    if hello.protocol_version != PROTOCOL_VERSION {
        return Err(format!("sender speaks version {}", hello.protocol_version));
    }
    if !hello.capabilities.iter().any(|c| c == "sha256") {
        return Err("sender does not send SHA-256 digests".into());
    }
    put(
        pipe,
        Body::Welcome(Welcome {
            protocol_version: PROTOCOL_VERSION,
            max_chunk_bytes: CHUNK_BYTES,
            window_bytes: 0,
            high_water_path: String::new(),
        }),
        &[],
    )
    .await?;
    let mut entries = Vec::new();
    loop {
        let (Body::Manifest(batch), _) = take(pipe).await? else {
            return Err("expected ManifestBatch".into());
        };
        entries.extend(batch.entries);
        if batch.last {
            break;
        }
    }
    let mut names = HashSet::new();
    for entry in &entries {
        if !names.insert(safe_name(&entry.path)?) {
            return Err(format!("sender offers {} twice", entry.path));
        }
    }
    let mut incoming = Vec::new();
    match destination {
        Destination::Stdout => {
            if entries.len() != 1 {
                return Err(format!(
                    "standard output takes one file; the sender offers {}",
                    entries.len()
                ));
            }
            incoming.push(Incoming {
                name: entries[0].path.clone(),
                size: entries[0].size_bytes,
                part: None,
                output: Box::new(std::io::stdout().lock()),
                written: 0,
                crc: crc32fast::Hasher::new(),
                sha: Sha256::new(),
            });
        }
        Destination::Directory(directory) => {
            for entry in &entries {
                let finished = directory.join(&entry.path);
                if finished.exists() {
                    return Err(format!("{} already exists", finished.display()));
                }
                let part = directory.join(format!("{}.part", entry.path));
                let (written, file, crc, sha) = resume(&part, entry.size_bytes)?;
                incoming.push(Incoming {
                    name: entry.path.clone(),
                    size: entry.size_bytes,
                    part: Some(part),
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
        let end = loop {
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
                (Body::FileEnd(end), _) if end.path == file.name => break end,
                _ => return Err(format!("{}: unexpected frame", file.name)),
            }
        };
        let sha = file.sha.finalize();
        let verified = end.total_bytes == file.size
            && file.written == file.size
            && end.crc32 == file.crc.finalize()
            && end.sha256 == sha.as_slice();
        let mut status = Status::Failed;
        if verified {
            file.output
                .flush()
                .map_err(|e| format!("{}: {e}", file.name))?;
            drop(file.output);
            if let (Some(part), Destination::Directory(directory)) = (&file.part, destination) {
                std::fs::rename(part, directory.join(&file.name))
                    .map_err(|e| format!("{}: {e}", file.name))?;
            }
            status = Status::Stored;
        } else if let Some(part) = &file.part {
            // A corrupt prefix must not be resumed.
            let _ = std::fs::remove_file(part);
        }
        put(
            pipe,
            Body::Committed(Committed {
                path: file.name.clone(),
                status: status as i32,
                asset_reference: String::new(),
            }),
            &[],
        )
        .await?;
        if !verified {
            return Err(format!("{}: digest mismatch", file.name));
        }
        stored.push(file.name);
    }
    Ok(stored)
}

/// Lowercase hexadecimal.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests;
