//! Long-lived directory snapshots over the photo sync exchange.
//!
//! The server introduces itself with the `sync` capability and sends each
//! complete snapshot as an ordinary `ManifestBatch` with `complete` set. The
//! receiver answers `Wants`, receives `Chunk` and `FileEnd` frames, and
//! applies removals for paths that an earlier snapshot created.
use crate::tree::{DirectorySource, relative};
use crate::{
    CHUNK_BYTES, Hello, Incoming, ManifestBatch, Pipe, Report, Result, SYNC_CAPABILITY,
    SYNC_MARKER, Wants, appended, commit, fail, hash_file, manifest_entry, modified, put,
    receive_end, receive_manifest, resume, safe_path, take, transfer_wanted, welcome,
};
use fujimod_sync_protocol::committed::Status;
use fujimod_sync_protocol::{ManifestEntry, decode, frame::Body};
use sha2::Digest;
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
    let chunk = welcome.max_chunk_bytes.clamp(1, CHUNK_BYTES) as usize;
    let updates = source.subscribe();
    let mut served = None;
    loop {
        let (generation, snapshot) = source.snapshot();
        if served != Some(generation) {
            put(
                pipe,
                Body::Manifest(ManifestBatch {
                    entries: snapshot.iter().map(manifest_entry).collect(),
                    last: true,
                    complete: true,
                }),
                &[],
            )
            .await?;
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
    welcome(pipe, true).await?;
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
        let mut incoming = Vec::new();
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
            let file = incoming_file(&self.root, entry)?;
            if let Some(part) = &file.part {
                keep.insert(relative(&self.root, part)?);
            }
            if let Some(meta) = &file.meta {
                keep.insert(relative(&self.root, meta)?);
            }
            incoming.push(file);
        }
        summary.removed = clean(&self.root, &keep)?;
        for entry in &entries {
            if entry.directory {
                ensure_directory(&self.root, entry)?;
            }
        }
        for entry in &entries {
            if !entry.directory {
                ensure_parents(&self.root, &entry.path)?;
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
        for mut file in incoming {
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

fn incoming_file(root: &Path, entry: &Entry) -> Result<Incoming> {
    let finished = root.join(&entry.path);
    ensure_parents(root, &entry.path)?;
    let part = appended(&finished, ".part");
    let meta = appended(&finished, ".part.meta");
    let expected = crate::hex(&entry.sha256);
    let resumable = match std::fs::read_to_string(&meta) {
        Ok(stored) => stored == expected,
        Err(_) => false,
    };
    if !resumable {
        remove_if_present(&part)?;
        remove_if_present(&meta)?;
    }
    if let Ok(metadata) = std::fs::symlink_metadata(&part)
        && !metadata.is_file()
    {
        remove_if_present(&part)?;
    }
    let (written, file, crc, sha) = resume(&part, entry.size)?;
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    let mut meta_file = options
        .open(&meta)
        .map_err(|e| format!("{}: {e}", meta.display()))?;
    meta_file
        .write_all(expected.as_bytes())
        .and_then(|()| meta_file.sync_all())
        .map_err(|e| format!("{}: {e}", meta.display()))?;
    Ok(Incoming {
        name: entry.path.clone(),
        size: entry.size,
        part: Some(part),
        meta: Some(meta),
        expected_sha256: entry.sha256.clone(),
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
