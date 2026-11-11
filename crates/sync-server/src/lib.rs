//! Photo sync receiver.
//!
//! The engine consumes the same length-delimited protobuf messages the camera
//! sends and produces the replies the transport should send back. It owns
//! manifest selection, chunk ordering and per-file state. A destination owns
//! policy and durable storage.

use std::collections::{HashMap, VecDeque};

use fujimod_sync_protocol as protocol;
use protocol::frame::Body;
use protocol::wants;

pub mod folder;

pub use folder::FolderDestination;

/// Protocol version in the shared photo-sync schema.
pub const PROTOCOL_VERSION: u32 = 1;
/// The camera's nanopb schema bounds a manifest or wants list to eight entries.
pub const MAX_BATCH_ENTRIES: usize = 8;
/// The camera's message workspace leaves room for this many file bytes.
pub const MAX_CHUNK_BYTES: u32 = 896;
/// Advertised receive window. The client sends one message at a time today.
pub const DEFAULT_WINDOW_BYTES: u32 = 1 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraId {
    pub model: String,
    pub firmware: String,
    pub sd_card_volume_serial: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaEntry {
    pub path: String,
    pub size_bytes: u64,
    pub fat_timestamp: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Accept { resume_offset: u64 },
    Reject,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetReference(pub String);

#[derive(Debug, thiserror::Error)]
pub enum DestinationError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Invalid(String),
    #[error("CRC32 mismatch: expected {expected:#010x}, got {actual:#010x}")]
    Crc { expected: u32, actual: u32 },
}

/// One accepted file being written by the receiver.
///
/// Commit must verify expected_crc32 over the complete file, including a
/// resumed prefix, before making the asset durable.
pub trait Upload {
    fn write(&mut self, offset: u64, bytes: &[u8]) -> Result<(), DestinationError>;
    fn commit(self: Box<Self>, expected_crc32: u32) -> Result<AssetReference, DestinationError>;
    fn abort(self: Box<Self>) -> Result<(), DestinationError> {
        Ok(())
    }
}

/// Receiver-owned policy and storage.
pub trait MediaSyncDestination {
    /// Decide which manifest entries to fetch and where each one resumes.
    fn classify(
        &mut self,
        camera: &CameraId,
        entries: &[MediaEntry],
    ) -> Result<Vec<Decision>, DestinationError>;

    /// Open or create the destination object for one accepted entry.
    fn begin(
        &mut self,
        camera: &CameraId,
        entry: &MediaEntry,
        resume_offset: u64,
    ) -> Result<Box<dyn Upload>, DestinationError>;

    fn high_water_path(&self, _camera: &CameraId) -> Option<String> {
        None
    }

    fn max_chunk_bytes(&self) -> u32 {
        MAX_CHUNK_BYTES
    }

    fn window_bytes(&self) -> u32 {
        DEFAULT_WINDOW_BYTES
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("receiver state error: {0}")]
    State(String),
    #[error(transparent)]
    Destination(#[from] DestinationError),
}

impl From<&'static str> for SyncError {
    fn from(value: &'static str) -> Self {
        Self::Protocol(value.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    AwaitHello,
    AwaitManifest,
    Streaming,
    Done,
}

#[derive(Debug)]
struct WantedEntry {
    entry: MediaEntry,
    resume_offset: u64,
}

struct ActiveUpload {
    wanted: WantedEntry,
    upload: Box<dyn Upload>,
    next_offset: u64,
}

/// Transport-neutral receiver state machine.
pub struct SyncReceiver<D: MediaSyncDestination> {
    destination: D,
    phase: Phase,
    camera: Option<CameraId>,
    manifest: Vec<MediaEntry>,
    known_paths: HashMap<String, usize>,
    pending: VecDeque<WantedEntry>,
    active_batch: VecDeque<WantedEntry>,
    active: Option<ActiveUpload>,
    stored: Vec<AssetReference>,
}

impl<D: MediaSyncDestination> SyncReceiver<D> {
    pub fn new(destination: D) -> Self {
        Self {
            destination,
            phase: Phase::AwaitHello,
            camera: None,
            manifest: Vec::new(),
            known_paths: HashMap::new(),
            pending: VecDeque::new(),
            active_batch: VecDeque::new(),
            active: None,
            stored: Vec::new(),
        }
    }

    pub fn destination(&self) -> &D {
        &self.destination
    }

    pub fn stored(&self) -> &[AssetReference] {
        &self.stored
    }

    pub fn is_complete(&self) -> bool {
        self.phase == Phase::Done
    }

    /// Decode one camera message and return the messages to send back, in
    /// order. Chunk and manifest batches normally produce no reply.
    pub fn handle(&mut self, message: &[u8]) -> Result<Vec<Vec<u8>>, SyncError> {
        let (frame, payload) =
            protocol::decode(message).map_err(|error| SyncError::Protocol(error.to_string()))?;
        let body = frame
            .body
            .ok_or_else(|| SyncError::Protocol("missing body".to_string()))?;
        match body {
            Body::Hello(hello) => self.hello(frame.id, hello),
            Body::Manifest(batch) => self.manifest(batch),
            Body::Chunk(chunk) => {
                self.chunk(chunk, payload)?;
                Ok(Vec::new())
            }
            Body::FileEnd(end) => self.file_end(frame.id, end),
            Body::Error(error) => Err(SyncError::Protocol(format!(
                "camera error {}: {}",
                error.code, error.message
            ))),
            _ => Err(SyncError::Protocol(
                "message is not part of the photo sync exchange".to_string(),
            )),
        }
    }

    fn hello(&mut self, id: u32, hello: protocol::Hello) -> Result<Vec<Vec<u8>>, SyncError> {
        if self.phase != Phase::AwaitHello {
            return Err(SyncError::State("unexpected Hello".to_string()));
        }
        if hello.protocol_version != PROTOCOL_VERSION {
            return Err(SyncError::Protocol(format!(
                "unsupported protocol version {}",
                hello.protocol_version
            )));
        }
        let camera = CameraId {
            model: hello.camera_model,
            firmware: hello.firmware_version,
            sd_card_volume_serial: hello.sd_card_volume_serial,
        };
        let max_chunk = self
            .destination
            .max_chunk_bytes()
            .min(MAX_CHUNK_BYTES)
            .max(64);
        let welcome = protocol::Welcome {
            protocol_version: PROTOCOL_VERSION,
            max_chunk_bytes: max_chunk,
            window_bytes: self.destination.window_bytes(),
            high_water_path: self
                .destination
                .high_water_path(&camera)
                .unwrap_or_default(),
        };
        self.camera = Some(camera);
        self.phase = Phase::AwaitManifest;
        encode(protocol::Frame {
            id,
            body: Some(Body::Welcome(welcome)),
        })
        .map(|message| vec![message])
    }

    fn manifest(&mut self, batch: protocol::ManifestBatch) -> Result<Vec<Vec<u8>>, SyncError> {
        if self.phase != Phase::AwaitManifest {
            return Err(SyncError::State("unexpected manifest".to_string()));
        }
        for entry in batch.entries {
            if self.known_paths.contains_key(&entry.path) {
                continue;
            }
            self.known_paths
                .insert(entry.path.clone(), self.manifest.len());
            self.manifest.push(MediaEntry {
                path: entry.path,
                size_bytes: entry.size_bytes,
                fat_timestamp: entry.fat_timestamp,
            });
        }
        if !batch.last {
            return Ok(Vec::new());
        }
        let camera = self
            .camera
            .as_ref()
            .ok_or_else(|| SyncError::State("manifest before Hello".to_string()))?;
        let decisions = self.destination.classify(camera, &self.manifest)?;
        if decisions.len() != self.manifest.len() {
            return Err(SyncError::Destination(DestinationError::Invalid(
                "classify returned a different number of decisions".to_string(),
            )));
        }
        for (entry, decision) in self.manifest.iter().cloned().zip(decisions) {
            match decision {
                Decision::Accept { resume_offset } => {
                    if resume_offset > entry.size_bytes {
                        return Err(SyncError::Destination(DestinationError::Invalid(format!(
                            "resume offset for {} exceeds its size",
                            entry.path
                        ))));
                    }
                    self.pending.push_back(WantedEntry {
                        entry,
                        resume_offset,
                    });
                }
                Decision::Reject => {}
            }
        }
        self.phase = Phase::Streaming;
        self.next_wants()
    }

    fn chunk(&mut self, chunk: protocol::ChunkHeader, payload: &[u8]) -> Result<(), SyncError> {
        if self.phase != Phase::Streaming {
            return Err(SyncError::State("unexpected chunk".to_string()));
        }
        if chunk.length as usize != payload.len() {
            return Err(SyncError::Protocol(
                "chunk length does not match payload".to_string(),
            ));
        }
        if self.active.is_none() {
            self.begin_active(&chunk.path)?;
        }
        let active = self
            .active
            .as_mut()
            .ok_or_else(|| SyncError::State("chunk without an upload".to_string()))?;
        if active.wanted.entry.path != chunk.path {
            return Err(SyncError::Protocol(
                "chunk path is out of order".to_string(),
            ));
        }
        if active.next_offset != chunk.offset {
            return Err(SyncError::Protocol(format!(
                "chunk offset {} does not follow {}",
                chunk.offset, active.next_offset
            )));
        }
        active.upload.write(chunk.offset, payload)?;
        active.next_offset += payload.len() as u64;
        Ok(())
    }

    fn begin_active(&mut self, path: &str) -> Result<(), SyncError> {
        let wanted = self
            .active_batch
            .pop_front()
            .ok_or_else(|| SyncError::State("chunk for an unrequested file".to_string()))?;
        if wanted.entry.path != path {
            return Err(SyncError::Protocol(format!(
                "camera sent {} but {} was requested",
                path, wanted.entry.path
            )));
        }
        let camera = self
            .camera
            .as_ref()
            .ok_or_else(|| SyncError::State("chunk before Hello".to_string()))?;
        let upload = self
            .destination
            .begin(camera, &wanted.entry, wanted.resume_offset)?;
        self.active = Some(ActiveUpload {
            next_offset: wanted.resume_offset,
            wanted,
            upload,
        });
        Ok(())
    }

    fn file_end(&mut self, id: u32, end: protocol::FileEnd) -> Result<Vec<Vec<u8>>, SyncError> {
        if self.phase != Phase::Streaming {
            return Err(SyncError::State("unexpected FileEnd".to_string()));
        }
        if self.active.is_none() {
            self.begin_active(&end.path)?;
        }
        let active = self
            .active
            .take()
            .ok_or_else(|| SyncError::State("FileEnd without an upload".to_string()))?;
        if active.wanted.entry.path != end.path {
            return Err(SyncError::Protocol(
                "FileEnd path is out of order".to_string(),
            ));
        }
        if active.wanted.entry.size_bytes != end.total_bytes {
            return Err(SyncError::Protocol(format!(
                "FileEnd size {} does not match manifest size {}",
                end.total_bytes, active.wanted.entry.size_bytes
            )));
        }
        if active.next_offset != end.total_bytes {
            return Err(SyncError::Protocol(format!(
                "received {} bytes, expected {}",
                active.next_offset, end.total_bytes
            )));
        }
        let reference = active.upload.commit(end.crc32)?;
        self.stored.push(reference.clone());
        let committed = protocol::Committed {
            path: end.path,
            status: protocol::committed::Status::Stored as i32,
            asset_reference: reference.0,
        };
        let mut messages = vec![encode(protocol::Frame {
            id,
            body: Some(Body::Committed(committed)),
        })?];
        if self.active_batch.is_empty() {
            messages.extend(self.next_wants()?);
        }
        Ok(messages)
    }

    fn next_wants(&mut self) -> Result<Vec<Vec<u8>>, SyncError> {
        self.active_batch.clear();
        while self.active_batch.len() < MAX_BATCH_ENTRIES {
            let Some(wanted) = self.pending.pop_front() else {
                break;
            };
            self.active_batch.push_back(wanted);
        }
        let entries = self
            .active_batch
            .iter()
            .map(|wanted| wants::Entry {
                path: wanted.entry.path.clone(),
                resume_offset: wanted.resume_offset,
            })
            .collect();
        let wants = protocol::Wants { entries };
        if wants.entries.is_empty() {
            self.phase = Phase::Done;
        }
        encode(protocol::Frame {
            id: 2,
            body: Some(Body::Wants(wants)),
        })
        .map(|message| vec![message])
    }
}

fn encode(frame: protocol::Frame) -> Result<Vec<u8>, SyncError> {
    protocol::encode(&frame, &[]).map_err(|error| SyncError::Protocol(error.to_string()))
}
