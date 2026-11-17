use super::*;
use std::{sync::Arc, time::Duration};

struct Channel {
    tx: async_channel::Sender<Vec<u8>>,
    rx: async_channel::Receiver<Vec<u8>>,
}

impl Pipe for Channel {
    async fn send(&mut self, message: Vec<u8>) -> Result<()> {
        self.tx.send(message).await.map_err(|e| e.to_string())
    }
    async fn receive(&mut self) -> Result<Vec<u8>> {
        self.rx.recv().await.map_err(|e| e.to_string())
    }
}

fn pair() -> (Channel, Channel) {
    let (a, b) = async_channel::bounded(4);
    let (c, d) = async_channel::bounded(4);
    (Channel { tx: a, rx: d }, Channel { tx: c, rx: b })
}

fn scratch(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "fujimod-file-send-{label}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(path.join("in")).unwrap();
    std::fs::create_dir_all(path.join("out")).unwrap();
    path
}

fn bytes(length: usize, seed: u8) -> Vec<u8> {
    (0..length)
        .map(|i| (i as u8).wrapping_mul(31) ^ seed)
        .collect()
}

fn run(files: &[Source], out: &Path) -> (Result<()>, Result<Vec<String>>) {
    let (mut a, mut b) = pair();
    let destination = Destination::Directory(out.to_owned());
    futures_lite::future::block_on(futures_lite::future::zip(
        send(&mut a, files, &mut |_, _, _| {}),
        receive(&mut b, &destination, &mut |_, _, _| {}),
    ))
}

#[test]
fn sends_several_files_with_digests() {
    let root = scratch("several");
    let one = bytes(3 * CHUNK_BYTES as usize + 17, 1);
    let two = bytes(0, 2);
    std::fs::write(root.join("in/one.bin"), &one).unwrap();
    std::fs::write(root.join("in/two.bin"), &two).unwrap();
    let files = sources(&[root.join("in/one.bin"), root.join("in/two.bin")]).unwrap();
    let (sent, received) = run(&files, &root.join("out"));
    sent.unwrap();
    assert_eq!(received.unwrap(), ["one.bin", "two.bin"]);
    assert_eq!(std::fs::read(root.join("out/one.bin")).unwrap(), one);
    assert_eq!(std::fs::read(root.join("out/two.bin")).unwrap(), two);
    assert!(!root.join("out/one.bin.part").exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn resumes_a_partial_file() {
    let root = scratch("resume");
    let data = bytes(2 * CHUNK_BYTES as usize + 5, 3);
    std::fs::write(root.join("in/big.bin"), &data).unwrap();
    let prefix = CHUNK_BYTES as usize + 9;
    std::fs::write(root.join("out/big.bin.part"), &data[..prefix]).unwrap();
    let files = sources(&[root.join("in/big.bin")]).unwrap();
    let (mut a, mut b) = pair();
    let destination = Destination::Directory(root.join("out"));
    let mut first = None;
    let (sent, received) = futures_lite::future::block_on(futures_lite::future::zip(
        send(&mut a, &files, &mut |_, _, _| {}),
        receive(&mut b, &destination, &mut |_, done, _| {
            first.get_or_insert(done);
        }),
    ));
    sent.unwrap();
    received.unwrap();
    assert_eq!(first, Some(prefix as u64));
    assert_eq!(std::fs::read(root.join("out/big.bin")).unwrap(), data);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_corrupt_partial_file_fails_and_is_removed() {
    let root = scratch("corrupt");
    let data = bytes(CHUNK_BYTES as usize + 5, 4);
    std::fs::write(root.join("in/x.bin"), &data).unwrap();
    std::fs::write(root.join("out/x.bin.part"), vec![0xee; 100]).unwrap();
    let files = sources(&[root.join("in/x.bin")]).unwrap();
    let (sent, received) = run(&files, &root.join("out"));
    assert!(sent.is_err());
    assert!(received.unwrap_err().contains("digest mismatch"));
    assert!(!root.join("out/x.bin").exists());
    assert!(!root.join("out/x.bin.part").exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn refuses_names_that_leave_the_directory() {
    assert!(safe_path("../x").is_err());
    assert!(safe_path("a/../b").is_err());
    assert!(safe_path("..").is_err());
    assert!(safe_path("a/b").is_ok());
    assert!(safe_path("ok.tar.xz").is_ok());
}

#[test]
fn refuses_duplicate_names_and_existing_outputs() {
    let root = scratch("dupes");
    std::fs::create_dir_all(root.join("in/sub")).unwrap();
    std::fs::write(root.join("in/a.bin"), b"1").unwrap();
    std::fs::write(root.join("in/sub/a.bin"), b"2").unwrap();
    assert!(sources(&[root.join("in/a.bin"), root.join("in/sub/a.bin")]).is_err());
    std::fs::write(root.join("out/a.bin"), b"old").unwrap();
    let files = sources(&[root.join("in/a.bin")]).unwrap();
    let (_, received) = run(&files, &root.join("out"));
    assert!(received.unwrap_err().contains("already exists"));
    assert_eq!(std::fs::read(root.join("out/a.bin")).unwrap(), b"old");
    std::fs::remove_dir_all(root).unwrap();
}

fn sync_once(source: Arc<DirectorySource>, out: &Path) -> Result<SyncSummary> {
    let (mut host, mut controller) = pair();
    let server = std::thread::spawn(move || {
        futures_lite::future::block_on(serve_peer(&mut host, &source, &mut |_, _, _| {}))
    });
    let received = futures_lite::future::block_on(async {
        welcome(&mut controller, true, "").await?;
        receive_snapshot(&mut controller, out, &mut |_, _, _| {}).await
    });
    drop(controller);
    let _ = server.join();
    received
}

#[test]
fn syncs_a_directory_tree_and_follows_changes() {
    let root = scratch("sync-tree");
    let input = root.join("in");
    let out = root.join("out");
    std::fs::create_dir_all(input.join("sub")).unwrap();
    std::fs::create_dir_all(input.join("empty")).unwrap();
    std::fs::write(input.join("a.txt"), b"one").unwrap();
    std::fs::write(input.join("sub/b.txt"), b"two").unwrap();
    std::fs::write(input.join("keep.txt"), b"unchanged").unwrap();
    let source = DirectorySource::open_with_interval(&input, Duration::from_millis(20)).unwrap();

    let first = sync_once(source.clone(), &out).unwrap();
    assert_eq!(first.stored, ["a.txt", "keep.txt", "sub/b.txt"]);
    assert_eq!(std::fs::read(out.join("a.txt")).unwrap(), b"one");
    assert_eq!(std::fs::read(out.join("sub/b.txt")).unwrap(), b"two");
    assert!(out.join("empty").is_dir());

    std::fs::write(input.join("a.txt"), b"one, changed").unwrap();
    std::fs::write(input.join("c.txt"), b"three").unwrap();
    std::fs::remove_file(input.join("sub/b.txt")).unwrap();
    std::fs::remove_dir(input.join("empty")).unwrap();
    assert!(source.refresh().unwrap());

    let second = sync_once(source.clone(), &out).unwrap();
    assert_eq!(second.stored, ["a.txt", "c.txt"]);
    assert_eq!(std::fs::read(out.join("a.txt")).unwrap(), b"one, changed");
    assert_eq!(std::fs::read(out.join("c.txt")).unwrap(), b"three");
    assert!(!out.join("sub/b.txt").exists());
    assert!(!out.join("empty").exists());
    assert!(second.skipped >= 1);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn sync_refuses_to_manage_an_unmarked_directory() {
    let root = scratch("sync-guard");
    let out = root.join("out");
    std::fs::write(out.join("keep.txt"), b"personal").unwrap();
    assert!(prepare_sync_root(&out, false).is_err());
    assert!(prepare_sync_root(&out, true).is_ok());
    assert!(out.join(SYNC_MARKER).is_file());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_connected_receiver_gets_later_snapshots() {
    let root = scratch("sync-live");
    let input = root.join("in");
    let out = root.join("out");
    std::fs::write(input.join("first.txt"), b"first").unwrap();
    let source = DirectorySource::open_with_interval(&input, Duration::from_secs(600)).unwrap();
    let (mut host, mut controller) = pair();
    let server_source = source.clone();
    let server = std::thread::spawn(move || {
        futures_lite::future::block_on(serve_peer(&mut host, &server_source, &mut |_, _, _| {}))
    });
    let received = futures_lite::future::block_on(async {
        welcome(&mut controller, true, "").await?;
        let mut target = SyncTarget::new(&out);
        let first = target.receive(&mut controller, &mut |_, _, _| {}).await?;
        std::fs::write(input.join("second.txt"), b"second").unwrap();
        source.refresh()?;
        let second = target.receive(&mut controller, &mut |_, _, _| {}).await?;
        Ok::<_, String>((first, second))
    });
    drop(controller);
    let _ = server.join();
    let (first, second) = received.unwrap();
    assert_eq!(first.stored, ["first.txt"]);
    assert_eq!(second.stored, ["second.txt"]);
    assert_eq!(std::fs::read(out.join("first.txt")).unwrap(), b"first");
    assert_eq!(std::fs::read(out.join("second.txt")).unwrap(), b"second");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn sends_large_manifests_in_batches() {
    let (mut sender, mut receiver) = pair();
    let entries: Vec<ManifestEntry> = (0..MANIFEST_BATCH * 2 + 1)
        .map(|index| ManifestEntry {
            path: format!("file-{index}"),
            size_bytes: index as u64,
            fat_timestamp: 0,
            directory: false,
            sha256: vec![index as u8; 32],
        })
        .collect();
    let sent = futures_lite::future::block_on(send_manifest(&mut sender, entries.clone(), true));
    let received = futures_lite::future::block_on(receive_manifest(&mut receiver, true));
    sent.unwrap();
    assert_eq!(received.unwrap().len(), entries.len());
}
