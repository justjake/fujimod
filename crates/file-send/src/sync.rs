//! Long-lived directory snapshots over the photo sync exchange.
//!
//! The server introduces itself with the `sync` capability and sends each
//! complete snapshot as an ordinary `ManifestBatch` with `complete` set. The
//! receiver answers `Wants`, receives `Chunk` and `FileEnd` frames, and
//! applies removals for paths that an earlier snapshot created.
use crate::tree::{DirectorySource, relative};
use crate::{
    CHUNK_BYTES, Hello, Incoming, Pipe, Report, Result, SYNC_CAPABILITY, SYNC_MARKER, SYNC_PARTS,
    Wants, commit, fail, hash_file, manifest_entry, modified, path_info, put, receive_end,
    receive_manifest, resume, safe_path, send_manifest, take, transfer_wanted, welcome,
};
use fujimod_sync_protocol::committed::Status;
use fujimod_sync_protocol::{ManifestEntry, decode, frame::Body};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    time::SystemTime,
};
use walkdir::WalkDir;

const MARKER_CONTENT: &[u8] = b"fujimod-send sync destination\n";

/// What one complete snapshot changed on the receiver.
#[derive(Default, Debug, PartialEq, Eq)]
pub struct SyncSummary {
    pub stored: Vec<String>,
    pub removed: Vec<String>,
    pub skipped: usize,
    pub directories: usize,
}

/// Join a room as its owner and serve `source` to every receiver that joins.
pub async fn serve_peer(
    pipe: &mut impl Pipe,
    source: &DirectorySource,
    report: Report<'_>,
) -> Result<()> {
    let result = serve_connected(pipe, source, report).await;
    fail(pipe, result).await
}

enum Wake {
    Changed,
    Frame(Result<Vec<u8>>),
}

async fn serve_connected(
    pipe: &mut impl Pipe,
    source: &DirectorySource,
    report: Report<'_>,
) -> Result<()> {
    put(
        pipe,
        Body::Hello(Hello {
            protocol_version: crate::PROTOCOL_VERSION,
            capabilities: vec!["files".into(), "sha256".into(), SYNC_CAPABILITY.into()],
            ..Default::default()
        }),
        &[],
    )
    .await?;
    let (Body::Welcome(welcome), _) = take(pipe).await? else {
        return Err("expected Welcome".into());
    };
    if welcome.protocol_version != crate::PROTOCOL_VERSION {
        return Err(format!(
            "receiver speaks version {}",
            welcome.protocol_version
        ));
    }
    if !welcome.client.is_empty() {
        eprintln!("receiver: {}", welcome.client);
    }
    let chunk = welcome.max_chunk_bytes.clamp(1, CHUNK_BYTES) as usize;
    let updates = source.subscribe();
    let mut served = None;
    loop {
        let (generation, snapshot) = source.snapshot();
        if served != Some(generation) {
            send_manifest(pipe, snapshot.iter().map(manifest_entry).collect(), true).await?;
            let (Body::Wants(wants), _) = take(pipe).await? else {
                return Err("expected Wants".into());
            };
            transfer_wanted(pipe, &snapshot, &wants, chunk, &mut *report).await?;
            served = Some(generation);
            continue;
        }
        let wake = futures_lite::future::or(
            async {
                updates
                    .recv()
                    .await
                    .map(|_| Wake::Changed)
                    .map_err(|e| e.to_string())
            },
            async { Ok(Wake::Frame(pipe.receive().await)) },
        )
        .await?;
        match wake {
            Wake::Changed => (),
            Wake::Frame(message) => {
                let message = message?;
                let (frame, _) = decode(&message).map_err(str::to_owned)?;
                match frame.body {
                    Some(Body::Error(error)) => {
                        return Err(format!("peer error: {}", error.message));
                    }
                    Some(Body::Progress(_)) => (),
                    Some(_) => return Err("unexpected frame while waiting for changes".into()),
                    None => return Err("missing frame body".into()),
                }
            }
        }
    }
}

/// Follow one server while this process keeps running.
pub async fn sync(pipe: &mut impl Pipe, root: &Path, report: Report<'_>) -> Result<()> {
    let result = sync_connected(pipe, root, report).await;
    fail(pipe, result).await
}

async fn sync_connected(pipe: &mut impl Pipe, root: &Path, report: Report<'_>) -> Result<()> {
    welcome(pipe, true, &path_info(root)).await?;
    let mut target = SyncTarget::new(root);
    loop {
        let summary = target.receive(pipe, &mut *report).await?;
        report_summary(&summary);
    }
}

fn report_summary(summary: &SyncSummary) {
    if summary.stored.is_empty() && summary.removed.is_empty() {
        return;
    }
    eprintln!(
        "synced {} file(s), skipped {}, removed {} path(s)",
        summary.stored.len(),
        summary.skipped,
        summary.removed.len()
    );
}

/// Receive one complete snapshot from an already welcomed sender.
pub async fn receive_snapshot(
    pipe: &mut impl Pipe,
    root: &Path,
    report: Report<'_>,
) -> Result<SyncSummary> {
    SyncTarget::new(root).receive(pipe, report).await
}

#[derive(Clone, Debug)]
struct LocalFile {
    size: u64,
    modified: Option<SystemTime>,
    digest: Vec<u8>,
}

/// A receiver directory with a cache of local file digests.
pub struct SyncTarget {
    root: PathBuf,
    cache: HashMap<String, LocalFile>,
}

impl SyncTarget {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            cache: HashMap::new(),
        }
    }

    pub async fn receive(
        &mut self,
        pipe: &mut impl Pipe,
        report: Report<'_>,
    ) -> Result<SyncSummary> {
        let entries = validate(receive_manifest(pipe, true).await?)?;
        prepare_sync_root(&self.root, false)?;
        let mut keep = keep_paths(&entries);
        let mut wanted = Vec::new();
        let mut summary = SyncSummary::default();
        for entry in &entries {
            if entry.directory {
                summary.directories += 1;
                continue;
            }
            let finished = self.root.join(&entry.path);
            if self.local_matches(&finished, entry)? {
                summary.skipped += 1;
                continue;
            }
            let file = wanted_file(&self.root, entry)?;
            keep.insert(relative(&self.root, &file.part)?);
            keep.insert(relative(&self.root, &file.meta)?);
            wanted.push(file);
        }
        summary.removed = clean(&self.root, &keep)?;
        for entry in &entries {
            if entry.directory {
                ensure_directory(&self.root, entry)?;
            }
        }
        put(
            pipe,
            Body::Wants(Wants {
                entries: wanted
                    .iter()
                    .map(|file| fujimod_sync_protocol::wants::Entry {
                        path: file.name.clone(),
                        resume_offset: file.resume_offset,
                    })
                    .collect(),
            }),
            &[],
        )
        .await?;
        for wanted in wanted {
            let mut file = open_incoming(&wanted)?;
            report(&file.name, file.written, file.size);
            let end = receive_end(pipe, &mut file, &mut *report).await?;
            let sha = file.sha.finalize();
            let verified = end.total_bytes == file.size
                && file.written == file.size
                && end.crc32 == file.crc.finalize()
                && end.sha256 == sha.as_slice()
                && end.sha256 == file.expected_sha256;
            let mut status = Status::Failed;
            if verified {
                file.output
                    .flush()
                    .map_err(|e| format!("{}: {e}", file.name))?;
                drop(file.output);
                let finished = self.root.join(&file.name);
                remove_if_present(&finished)?;
                if let Some(part) = &file.part {
                    std::fs::rename(part, &finished).map_err(|e| format!("{}: {e}", file.name))?;
                }
                if let Some(meta) = &file.meta {
                    let _ = std::fs::remove_file(meta);
                }
                status = Status::Stored;
            } else {
                drop(file.output);
                if let Some(part) = &file.part {
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
            summary.stored.push(file.name);
        }
        Ok(summary)
    }

    fn local_matches(&mut self, path: &Path, entry: &Entry) -> Result<bool> {
        let metadata = match std::fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(format!("{}: {error}", path.display())),
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() != entry.size
        {
            return Ok(false);
        }
        let modified = modified(&metadata);
        if let Some(cached) = self.cache.get(&entry.path)
            && cached.size == entry.size
            && cached.modified == modified
        {
            return Ok(cached.digest == entry.sha256);
        }
        let digest = hash_file(path)?;
        self.cache.insert(
            entry.path.clone(),
            LocalFile {
                size: metadata.len(),
                modified,
                digest: digest.clone(),
            },
        );
        Ok(digest == entry.sha256)
    }
}

#[derive(Clone, Debug)]
struct Entry {
    path: String,
    size: u64,
    directory: bool,
    sha256: Vec<u8>,
}

fn validate(entries: Vec<ManifestEntry>) -> Result<Vec<Entry>> {
    let mut result = Vec::new();
    let mut kinds = HashMap::new();
    for entry in entries {
        let path = safe_path(&entry.path)?.to_owned();
        if entry.sha256.len() != 32 && !entry.directory {
            return Err(format!("{}: sender did not send a SHA-256", entry.path));
        }
        if entry.directory && entry.size_bytes != 0 {
            return Err(format!("{}: directory has a nonzero size", entry.path));
        }
        if entry.directory && !entry.sha256.is_empty() {
            return Err(format!("{}: directory carries a digest", entry.path));
        }
        if kinds.insert(path.clone(), entry.directory).is_some() {
            return Err(format!("sender offers {path} twice"));
        }
        result.push(Entry {
            path,
            size: entry.size_bytes,
            directory: entry.directory,
            sha256: entry.sha256,
        });
    }
    for entry in &result {
        let parts: Vec<&str> = entry.path.split('/').collect();
        let mut prefix = String::new();
        for segment in &parts[..parts.len().saturating_sub(1)] {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(segment);
            if kinds.get(&prefix) == Some(&false) {
                return Err(format!("{}: is inside a file", entry.path));
            }
        }
    }
    Ok(result)
}

fn keep_paths(entries: &[Entry]) -> HashSet<String> {
    let mut keep = HashSet::new();
    keep.insert(SYNC_MARKER.to_owned());
    keep.insert(SYNC_PARTS.to_owned());
    for entry in entries {
        keep.insert(entry.path.clone());
        let parts: Vec<&str> = entry.path.split('/').collect();
        let mut prefix = String::new();
        for segment in &parts[..parts.len().saturating_sub(1)] {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(segment);
            keep.insert(prefix.clone());
        }
    }
    keep
}

struct Wanted {
    name: String,
    size: u64,
    part: PathBuf,
    meta: PathBuf,
    expected_sha256: Vec<u8>,
    resume_offset: u64,
}

/// Inspect a partial file without creating or opening anything. The part is
/// opened only when its transfer starts, so a directory with thousands of
/// missing files does not fill the process descriptor limit up front.
fn wanted_file(root: &Path, entry: &Entry) -> Result<Wanted> {
    ensure_parents(root, &entry.path)?;
    let part = state_path(root, &entry.path, ".part");
    let meta = state_path(root, &entry.path, ".part.meta");
    let expected = crate::hex(&entry.sha256);
    let mut resume_offset = 0;
    let meta_matches = match std::fs::symlink_metadata(&meta) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            std::fs::read_to_string(&meta)
                .map(|stored| stored == expected)
                .unwrap_or(false)
        }
        _ => false,
    };
    if meta_matches {
        match std::fs::symlink_metadata(&part) {
            Ok(metadata) if metadata.is_file() && metadata.len() <= entry.size => {
                resume_offset = metadata.len();
            }
            Ok(_) => {
                remove_if_present(&part)?;
                remove_if_present(&meta)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                remove_if_present(&meta)?;
            }
            Err(error) => return Err(format!("{}: {error}", part.display())),
        }
    } else if std::fs::symlink_metadata(&meta).is_ok() || std::fs::symlink_metadata(&part).is_ok() {
        remove_if_present(&part)?;
        remove_if_present(&meta)?;
    }
    Ok(Wanted {
        name: entry.path.clone(),
        size: entry.size,
        part,
        meta,
        expected_sha256: entry.sha256.clone(),
        resume_offset,
    })
}

fn state_path(root: &Path, name: &str, suffix: &str) -> PathBuf {
    let mut sha = Sha256::new();
    sha.update(name.as_bytes());
    let digest = sha.finalize();
    root.join(SYNC_PARTS)
        .join(format!("{}{suffix}", crate::hex(&digest)))
}

fn open_incoming(wanted: &Wanted) -> Result<Incoming> {
    let parent = wanted
        .part
        .parent()
        .ok_or_else(|| format!("{}: no parent", wanted.part.display()))?;
    std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    if let Ok(metadata) = std::fs::symlink_metadata(&wanted.part)
        && !metadata.is_file()
    {
        remove_if_present(&wanted.part)?;
    }
    let (written, file, crc, sha) = resume(&wanted.part, wanted.size)?;
    if written != wanted.resume_offset {
        return Err(format!("{}: partial file changed after Wants", wanted.name));
    }
    let expected = crate::hex(&wanted.expected_sha256);
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    let mut meta_file = options
        .open(&wanted.meta)
        .map_err(|e| format!("{}: {e}", wanted.meta.display()))?;
    meta_file
        .write_all(expected.as_bytes())
        .and_then(|()| meta_file.sync_all())
        .map_err(|e| format!("{}: {e}", wanted.meta.display()))?;
    Ok(Incoming {
        name: wanted.name.clone(),
        size: wanted.size,
        part: Some(wanted.part.clone()),
        meta: Some(wanted.meta.clone()),
        expected_sha256: wanted.expected_sha256.clone(),
        output: Box::new(file),
        written,
        crc,
        sha,
    })
}

fn clean(root: &Path, keep: &HashSet<String>) -> Result<Vec<String>> {
    let mut extras = Vec::new();
    for item in WalkDir::new(root).follow_links(false).min_depth(1) {
        let item = item.map_err(|e| e.to_string())?;
        let path = relative(root, item.path())?;
        if path == SYNC_MARKER || keep.contains(&path) {
            continue;
        }
        extras.push((
            item.depth(),
            path,
            item.path().to_path_buf(),
            item.file_type().is_dir(),
        ));
    }
    extras.sort_by_key(|(depth, ..)| std::cmp::Reverse(*depth));
    let mut removed = Vec::new();
    for (_, path, full, directory) in extras {
        let result = if directory {
            std::fs::remove_dir(&full)
        } else {
            std::fs::remove_file(&full)
        };
        match result {
            Ok(()) => removed.push(path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(format!("{}: {error}", full.display())),
        }
    }
    Ok(removed)
}

fn ensure_directory(root: &Path, entry: &Entry) -> Result<()> {
    ensure_parents(root, &entry.path)?;
    let path = root.join(&entry.path);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => return Ok(()),
        Ok(_) => remove_if_present(&path)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    }
    std::fs::create_dir_all(&path).map_err(|e| format!("{}: {e}", path.display()))
}

fn ensure_parents(root: &Path, path: &str) -> Result<()> {
    let parts: Vec<&str> = path.split('/').collect();
    let mut full = root.to_path_buf();
    for segment in &parts[..parts.len().saturating_sub(1)] {
        full.push(segment);
        match std::fs::symlink_metadata(&full) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => (),
            Ok(_) => {
                remove_if_present(&full)?;
                std::fs::create_dir_all(&full).map_err(|e| format!("{}: {e}", full.display()))?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir_all(&full).map_err(|e| format!("{}: {e}", full.display()))?;
            }
            Err(error) => return Err(format!("{}: {error}", full.display())),
        }
    }
    Ok(())
}

fn remove_if_present(path: &Path) -> Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let result = if metadata.is_dir() && !metadata.file_type().is_symlink() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    result.map_err(|e| format!("{}: {e}", path.display()))
}

/// Mark `root` as a managed sync destination.
pub fn prepare_sync_root(root: &Path, force: bool) -> Result<()> {
    if root.exists() && !root.is_dir() {
        return Err(format!("{}: not a directory", root.display()));
    }
    std::fs::create_dir_all(root).map_err(|e| format!("{}: {e}", root.display()))?;
    let marker = root.join(SYNC_MARKER);
    if let Ok(metadata) = std::fs::symlink_metadata(&marker) {
        if metadata.is_file() && !metadata.file_type().is_symlink() {
            return Ok(());
        }
        if !force {
            return Err(format!(
                "{}: {} is not a regular file; choose another directory or pass --force",
                root.display(),
                SYNC_MARKER
            ));
        }
        remove_if_present(&marker)?;
    }
    let mut entries = std::fs::read_dir(root).map_err(|e| format!("{}: {e}", root.display()))?;
    if entries.next().is_some() && !force {
        return Err(format!(
            "{} is not empty; choose a new directory or pass --force to let sync manage it",
            root.display()
        ));
    }
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    let mut file = options
        .open(&marker)
        .map_err(|e| format!("{}: {e}", marker.display()))?;
    file.write_all(MARKER_CONTENT)
        .and_then(|()| file.sync_all())
        .map_err(|e| format!("{}: {e}", marker.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn scratch(label: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "fujimod-sync-{label}-{}-{stamp}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn wanted_files_open_parts_only_when_the_transfer_starts() {
        let root = scratch("lazy");
        let entry = Entry {
            path: "a.bin".into(),
            size: 4,
            directory: false,
            sha256: vec![7; 32],
        };
        let wanted = wanted_file(&root, &entry).unwrap();
        assert_eq!(wanted.resume_offset, 0);
        assert!(!wanted.part.exists());
        assert!(!wanted.meta.exists());

        let incoming = open_incoming(&wanted).unwrap();
        assert!(wanted.part.is_file());
        assert!(wanted.meta.is_file());
        drop(incoming);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_matching_part_resumes_without_being_opened_early() {
        let root = scratch("resume");
        let entry = Entry {
            path: "a.bin".into(),
            size: 4,
            directory: false,
            sha256: vec![7; 32],
        };
        let wanted = wanted_file(&root, &entry).unwrap();
        std::fs::create_dir_all(wanted.part.parent().unwrap()).unwrap();
        std::fs::write(&wanted.part, b"ab").unwrap();
        std::fs::write(&wanted.meta, crate::hex(&entry.sha256)).unwrap();
        let wanted = wanted_file(&root, &entry).unwrap();
        assert_eq!(wanted.resume_offset, 2);
        let incoming = open_incoming(&wanted).unwrap();
        assert_eq!(incoming.written, 2);
        drop(incoming);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_names_cannot_collide_with_resume_state() {
        let root = scratch("collision");
        let first = Entry {
            path: "a".into(),
            size: 0,
            directory: false,
            sha256: vec![1; 32],
        };
        let second = Entry {
            path: "a.part".into(),
            size: 0,
            directory: false,
            sha256: vec![2; 32],
        };
        let first = wanted_file(&root, &first).unwrap();
        let second = wanted_file(&root, &second).unwrap();
        assert_ne!(first.part, second.part);
        assert_eq!(first.part.parent().unwrap(), root.join(SYNC_PARTS));
        assert!(safe_path(".fujimod-sync-parts").is_err());
        assert!(safe_path(".fujimod-sync-parts/anything").is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn validates_directory_size() {
        let error = validate(vec![ManifestEntry {
            path: "empty".into(),
            size_bytes: 1,
            fat_timestamp: 0,
            directory: true,
            sha256: Vec::new(),
        }])
        .unwrap_err();
        assert!(error.contains("nonzero size"));
    }
}
