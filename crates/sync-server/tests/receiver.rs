use std::sync::{Arc, Mutex};

use fujimod_sync_protocol as protocol;
use fujimod_sync_server::{
    AssetReference, CameraId, Decision, DestinationError, FolderDestination, MediaEntry,
    MediaSyncDestination, SyncReceiver, Upload,
};
use protocol::frame::Body;

#[derive(Debug, Clone)]
struct Stored {
    path: String,
    bytes: Vec<u8>,
}

struct MemoryDestination {
    source: Vec<u8>,
    resume: u64,
    stored: Arc<Mutex<Vec<Stored>>>,
}

impl MediaSyncDestination for MemoryDestination {
    fn classify(
        &mut self,
        _camera: &CameraId,
        entries: &[MediaEntry],
    ) -> Result<Vec<Decision>, DestinationError> {
        Ok(entries
            .iter()
            .map(|_| Decision::Accept {
                resume_offset: self.resume,
            })
            .collect())
    }

    fn begin(
        &mut self,
        _camera: &CameraId,
        entry: &MediaEntry,
        resume_offset: u64,
    ) -> Result<Box<dyn Upload>, DestinationError> {
        if resume_offset as usize > self.source.len() {
            return Err(DestinationError::Invalid(
                "resume offset exceeds the source".to_string(),
            ));
        }
        Ok(Box::new(MemoryUpload {
            path: entry.path.clone(),
            data: self.source[..resume_offset as usize].to_vec(),
            stored: Arc::clone(&self.stored),
        }))
    }
}

struct MemoryUpload {
    path: String,
    data: Vec<u8>,
    stored: Arc<Mutex<Vec<Stored>>>,
}

impl Upload for MemoryUpload {
    fn write(&mut self, offset: u64, bytes: &[u8]) -> Result<(), DestinationError> {
        if offset != self.data.len() as u64 {
            return Err(DestinationError::Invalid(
                "write is not contiguous".to_string(),
            ));
        }
        self.data.extend_from_slice(bytes);
        Ok(())
    }

    fn commit(self: Box<Self>, expected_crc32: u32) -> Result<AssetReference, DestinationError> {
        let actual = crc32fast::hash(&self.data);
        if actual != expected_crc32 {
            return Err(DestinationError::Crc {
                expected: expected_crc32,
                actual,
            });
        }
        self.stored.lock().unwrap().push(Stored {
            path: self.path.clone(),
            bytes: self.data.clone(),
        });
        Ok(AssetReference(self.path.clone()))
    }
}

fn hello() -> protocol::Frame {
    protocol::Frame {
        id: 1,
        body: Some(Body::Hello(protocol::Hello {
            protocol_version: 1,
            camera_model: "X70".to_string(),
            firmware_version: "1.10".to_string(),
            sd_card_volume_serial: 0x1234_5678,
            capabilities: vec!["manifest".to_string(), "crc32".to_string()],
        })),
    }
}

fn manifest(path: &str, size: u64) -> protocol::Frame {
    protocol::Frame {
        id: 0,
        body: Some(Body::Manifest(protocol::ManifestBatch {
            entries: vec![protocol::ManifestEntry {
                path: path.to_string(),
                size_bytes: size,
                fat_timestamp: 0x5a2b_0000,
            }],
            last: true,
        })),
    }
}

fn chunk(path: &str, offset: u64, bytes: &[u8]) -> Vec<u8> {
    let frame = protocol::Frame {
        id: 0,
        body: Some(Body::Chunk(protocol::ChunkHeader {
            path: path.to_string(),
            offset,
            length: bytes.len() as u32,
        })),
    };
    protocol::encode(&frame, bytes).unwrap()
}

fn file_end(path: &str, total_bytes: u64, crc32: u32) -> protocol::Frame {
    protocol::Frame {
        id: 9,
        body: Some(Body::FileEnd(protocol::FileEnd {
            path: path.to_string(),
            total_bytes,
            crc32,
        })),
    }
}

fn encode(frame: protocol::Frame) -> Vec<u8> {
    protocol::encode(&frame, &[]).unwrap()
}

fn decode(message: &[u8]) -> protocol::Frame {
    protocol::decode(message).unwrap().0
}

#[test]
fn receives_a_resumed_file_and_verifies_its_crc() {
    let source: Vec<u8> = (0..300u32).map(|value| (value * 7 + 3) as u8).collect();
    let stored = Arc::new(Mutex::new(Vec::new()));
    let destination = MemoryDestination {
        source: source.clone(),
        resume: 3,
        stored: Arc::clone(&stored),
    };
    let mut receiver = SyncReceiver::new(destination);

    let welcome = receiver.handle(&encode(hello())).unwrap();
    assert_eq!(welcome.len(), 1);
    match decode(&welcome[0]).body.unwrap() {
        Body::Welcome(welcome) => {
            assert_eq!(welcome.protocol_version, 1);
            assert_eq!(welcome.max_chunk_bytes, 896);
        }
        other => panic!("expected Welcome, got {other:?}"),
    }

    let wants = receiver
        .handle(&encode(manifest("DCIM/100_FUJI/DSCF0001.JPG", 300)))
        .unwrap();
    assert_eq!(wants.len(), 1);
    match decode(&wants[0]).body.unwrap() {
        Body::Wants(wants) => {
            assert_eq!(wants.entries.len(), 1);
            assert_eq!(wants.entries[0].path, "DCIM/100_FUJI/DSCF0001.JPG");
            assert_eq!(wants.entries[0].resume_offset, 3);
        }
        other => panic!("expected Wants, got {other:?}"),
    }

    let mut offset = 3u64;
    for bytes in source[3..].chunks(100) {
        assert!(
            receiver
                .handle(&chunk("DCIM/100_FUJI/DSCF0001.JPG", offset, bytes,))
                .unwrap()
                .is_empty()
        );
        offset += bytes.len() as u64;
    }

    let messages = receiver
        .handle(&encode(file_end(
            "DCIM/100_FUJI/DSCF0001.JPG",
            300,
            crc32fast::hash(&source),
        )))
        .unwrap();
    assert_eq!(messages.len(), 2);
    match decode(&messages[0]).body.unwrap() {
        Body::Committed(committed) => {
            assert_eq!(committed.status, protocol::committed::Status::Stored as i32);
            assert_eq!(committed.asset_reference, "DCIM/100_FUJI/DSCF0001.JPG");
        }
        other => panic!("expected Committed, got {other:?}"),
    }
    match decode(&messages[1]).body.unwrap() {
        Body::Wants(wants) => assert!(wants.entries.is_empty()),
        other => panic!("expected empty Wants, got {other:?}"),
    }
    assert!(receiver.is_complete());
    let stored = stored.lock().unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].path, "DCIM/100_FUJI/DSCF0001.JPG");
    assert_eq!(stored[0].bytes, source);
}

#[test]
fn folder_destination_writes_the_file_and_rejects_duplicates() {
    let directory = tempfile::tempdir().unwrap();
    let source: Vec<u8> = (0..50u32).map(|value| (value * 11 + 5) as u8).collect();
    let path = "DCIM/100_FUJI/DSCF0002.JPG";
    let mut receiver = SyncReceiver::new(FolderDestination::new(directory.path()));

    let _ = receiver.handle(&encode(hello())).unwrap();
    let wants = receiver
        .handle(&encode(manifest(path, source.len() as u64)))
        .unwrap();
    match decode(&wants[0]).body.unwrap() {
        Body::Wants(wants) => assert_eq!(wants.entries.len(), 1),
        other => panic!("expected Wants, got {other:?}"),
    }
    assert!(
        receiver
            .handle(&chunk(path, 0, &source[..20]))
            .unwrap()
            .is_empty()
    );
    assert!(
        receiver
            .handle(&chunk(path, 20, &source[20..]))
            .unwrap()
            .is_empty()
    );
    let messages = receiver
        .handle(&encode(file_end(
            path,
            source.len() as u64,
            crc32fast::hash(&source),
        )))
        .unwrap();
    assert_eq!(messages.len(), 2);
    let final_path = directory.path().join(path);
    assert_eq!(std::fs::read(&final_path).unwrap(), source);
    assert!(!directory.path().join(format!("{path}.part")).exists());

    let mut duplicate = SyncReceiver::new(FolderDestination::new(directory.path()));
    let _ = duplicate.handle(&encode(hello())).unwrap();
    let wants = duplicate
        .handle(&encode(manifest(path, source.len() as u64)))
        .unwrap();
    match decode(&wants[0]).body.unwrap() {
        Body::Wants(wants) => assert!(wants.entries.is_empty()),
        other => panic!("expected empty Wants, got {other:?}"),
    }
    assert!(duplicate.is_complete());
}
