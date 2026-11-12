use super::*;

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
    (0..length).map(|i| (i as u8).wrapping_mul(31) ^ seed).collect()
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
    assert!(safe_name("../x").is_err());
    assert!(safe_name("a/b").is_err());
    assert!(safe_name("..").is_err());
    assert!(safe_name("ok.tar.xz").is_ok());
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
