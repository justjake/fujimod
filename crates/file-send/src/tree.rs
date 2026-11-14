//! A directory offered by a live file server.
//!
//! A background scan publishes a new snapshot when the tree changes. File
//! digests are cached by size and modification time so unchanged files are not
//! read again on every scan.
use crate::{Result, Source, hash_file};
use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex, Weak},
    thread,
    time::{Duration, SystemTime},
};
use walkdir::WalkDir;

const DEFAULT_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone, Debug)]
struct Cached {
    size: u64,
    modified: Option<SystemTime>,
    digest: Vec<u8>,
}

struct Snapshot {
    generation: u64,
    sources: Arc<Vec<Source>>,
}

struct Shared {
    root: PathBuf,
    current: Mutex<Snapshot>,
    cache: Mutex<HashMap<String, Cached>>,
    subscribers: Mutex<Vec<async_channel::Sender<u64>>>,
}

/// A directory whose current contents can be offered to many receivers.
pub struct DirectorySource {
    shared: Arc<Shared>,
}

impl DirectorySource {
    /// Scan `root` and start watching it once per second.
    pub fn open(root: impl AsRef<Path>) -> Result<Arc<Self>> {
        Self::open_with_interval(root, DEFAULT_INTERVAL)
    }

    pub fn open_with_interval(root: impl AsRef<Path>, interval: Duration) -> Result<Arc<Self>> {
        let root = std::fs::canonicalize(root.as_ref())
            .map_err(|e| format!("{}: {e}", root.as_ref().display()))?;
        if !root.is_dir() {
            return Err(format!("{}: not a directory", root.display()));
        }
        let mut cache = HashMap::new();
        let sources = scan(&root, &mut cache)?;
        let shared = Arc::new(Shared {
            root: root.clone(),
            current: Mutex::new(Snapshot {
                generation: 1,
                sources: Arc::new(sources),
            }),
            cache: Mutex::new(cache),
            subscribers: Mutex::new(Vec::new()),
        });
        let source = Arc::new(Self {
            shared: shared.clone(),
        });
        let weak: Weak<Shared> = Arc::downgrade(&shared);
        thread::Builder::new()
            .name("fujimod-file-server-scan".into())
            .spawn(move || {
                loop {
                    thread::sleep(interval);
                    let Some(shared) = weak.upgrade() else {
                        break;
                    };
                    if let Err(error) = refresh_shared(&shared) {
                        eprintln!("fujimod-send: scanning {}: {error}", shared.root.display());
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(source)
    }

    pub fn root(&self) -> &Path {
        &self.shared.root
    }

    /// Rescan now instead of waiting for the watcher.
    pub fn refresh(&self) -> Result<bool> {
        refresh_shared(&self.shared)
    }

    /// The current generation and its complete snapshot.
    pub fn snapshot(&self) -> (u64, Arc<Vec<Source>>) {
        let current = self.shared.current.lock().expect("snapshot lock");
        (current.generation, current.sources.clone())
    }

    /// Wake this receiver when a later generation is published.
    pub fn subscribe(&self) -> async_channel::Receiver<u64> {
        let (sender, receiver) = async_channel::unbounded();
        self.shared
            .subscribers
            .lock()
            .expect("subscriber lock")
            .push(sender);
        receiver
    }
}

fn refresh_shared(shared: &Shared) -> Result<bool> {
    let mut cache = std::mem::take(&mut *shared.cache.lock().expect("cache lock"));
    let scanned = scan(&shared.root, &mut cache);
    let sources = match scanned {
        Ok(sources) => {
            *shared.cache.lock().expect("cache lock") = cache;
            sources
        }
        Err(error) => {
            *shared.cache.lock().expect("cache lock") = cache;
            return Err(error);
        }
    };
    let changed = {
        let mut current = shared.current.lock().expect("snapshot lock");
        if *current.sources == sources {
            false
        } else {
            current.generation += 1;
            current.sources = Arc::new(sources);
            true
        }
    };
    if changed {
        let generation = shared.current.lock().expect("snapshot lock").generation;
        shared
            .subscribers
            .lock()
            .expect("subscriber lock")
            .retain(|sender| sender.try_send(generation).is_ok());
    }
    Ok(changed)
}

fn scan(root: &Path, cache: &mut HashMap<String, Cached>) -> Result<Vec<Source>> {
    let mut sources = Vec::new();
    let mut next = HashMap::new();
    for item in WalkDir::new(root).follow_links(false).sort_by_file_name() {
        let item = item.map_err(|e| e.to_string())?;
        if item.path() == root {
            continue;
        }
        let file_type = item.file_type();
        if file_type.is_symlink() {
            continue;
        }
        let path = relative(root, item.path())?;
        if file_type.is_dir() {
            sources.push(Source {
                path,
                source: item.path().to_path_buf(),
                size: 0,
                directory: true,
                sha256: Vec::new(),
            });
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let metadata = item
            .metadata()
            .map_err(|e| format!("{}: {e}", item.path().display()))?;
        let size = metadata.len();
        let modified = metadata.modified().ok();
        let digest = match cache.get(&path) {
            Some(cached) if cached.size == size && cached.modified == modified => {
                cached.digest.clone()
            }
            _ => hash_file(item.path())?,
        };
        next.insert(
            path.clone(),
            Cached {
                size,
                modified,
                digest: digest.clone(),
            },
        );
        sources.push(Source {
            path,
            source: item.path().to_path_buf(),
            size,
            directory: false,
            sha256: digest,
        });
    }
    sources.sort_by(|a, b| a.path.cmp(&b.path));
    *cache = next;
    Ok(sources)
}

pub(crate) fn relative(root: &Path, path: &Path) -> Result<String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| format!("{}: outside {}", path.display(), root.display()))?;
    let mut segments = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(segment) => segments.push(
                segment
                    .to_str()
                    .ok_or_else(|| format!("{}: name is not UTF-8", path.display()))?,
            ),
            Component::CurDir => (),
            _ => return Err(format!("{}: unsupported path", path.display())),
        }
    }
    Ok(segments.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_use_slashes() {
        let root = Path::new("/tmp/root");
        assert_eq!(relative(root, Path::new("/tmp/root/a/b")).unwrap(), "a/b");
        assert!(relative(root, Path::new("/tmp/other")).is_err());
    }
}
