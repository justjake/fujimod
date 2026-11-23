//! Host-only frame publication. No guest RAM access occurs on this worker.
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
pub struct Frame {
    pub number: u64,
    pub sequence: u64,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}
pub struct Writer {
    sender: Option<mpsc::Sender<Frame>>,
    done: Arc<AtomicU64>,
    submitted: AtomicU64,
    error: Arc<Mutex<Option<String>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Writer {
    pub fn new(directory: PathBuf, bank: usize) -> Result<Self, String> {
        let (sender, receiver) = mpsc::channel::<Frame>();
        let done = Arc::new(AtomicU64::new(0));
        let published = done.clone();
        let error = Arc::new(Mutex::new(None));
        let failure = error.clone();
        let thread = std::thread::Builder::new()
            .name(format!("png-bank{bank}"))
            .spawn(move || {
                for frame in receiver {
                    let result = (|| {
                        let png = rgba_png(frame.width, frame.height, &frame.rgba)?;
                        std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
                        let file = directory.join(format!("bank{bank}-{:06}.png", frame.number));
                        let mut output = std::fs::OpenOptions::new()
                            .write(true)
                            .create_new(true)
                            .open(file)
                            .map_err(|e| e.to_string())?;
                        output.write_all(&png).map_err(|e| e.to_string())?;
                        let temporary = directory.join(format!(".bank{bank}-latest.tmp"));
                        std::fs::write(&temporary, png).map_err(|e| e.to_string())?;
                        std::fs::rename(
                            temporary,
                            directory.join(format!("latest-bank{bank}.png")),
                        )
                        .map_err(|e| e.to_string())?;
                        let nonblack = frame.rgba.chunks_exact(4).any(|p| p[..3] != [0, 0, 0]);
                        let publication = serde_json::json!({"frames":frame.number,"sequence":frame.sequence,"width":frame.width,"height":frame.height,"nonblack":nonblack});
                        let temporary = directory.join(format!(".bank{bank}-publication.tmp"));
                        std::fs::write(&temporary, publication.to_string()).map_err(|e| e.to_string())?;
                        std::fs::rename(temporary, directory.join(format!("bank{bank}-publication.json"))).map_err(|e| e.to_string())?;
                        Ok::<_, String>(())
                    })();
                    if let Err(e) = result {
                        *failure.lock().unwrap() = Some(e);
                        break;
                    }
                    published.store(frame.number, Ordering::Release);
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            sender: Some(sender),
            done,
            submitted: AtomicU64::new(0),
            error,
            thread: Some(thread),
        })
    }
    pub fn submit(&self, frame: Frame) -> Result<(), String> {
        if let Some(e) = self.error.lock().unwrap().clone() {
            return Err(e);
        }
        self.submitted.store(frame.number, Ordering::Release);
        self.sender
            .as_ref()
            .unwrap()
            .send(frame)
            .map_err(|e| e.to_string())
    }
    pub fn flush(&self) -> Result<(), String> {
        let target = self.submitted.load(Ordering::Acquire);
        while self.published() < target {
            if let Some(e) = self.error() {
                return Err(e);
            }
            if self.thread.as_ref().is_some_and(|t| t.is_finished()) {
                return Err("PNG worker stopped before publication".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        Ok(())
    }
    pub fn published(&self) -> u64 {
        self.done.load(Ordering::Acquire)
    }
    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap().clone()
    }
}
impl Drop for Writer {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// PNG RGBA8 with a zlib stream of stored DEFLATE blocks; no codec dependency.
pub fn rgba_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    if width == 0
        || height == 0
        || width > 8192
        || height > 8192
        || rgba.len() != width as usize * height as usize * 4
    {
        return Err("invalid RGBA frame dimensions".into());
    }
    let mut raw = Vec::with_capacity(rgba.len() + height as usize);
    for row in rgba.chunks_exact(width as usize * 4) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut z = vec![0x78, 0x01];
    let count = raw.len().div_ceil(65535);
    for (n, b) in raw.chunks(65535).enumerate() {
        z.push(u8::from(n + 1 == count));
        let len = b.len() as u16;
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(b);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &v in &raw {
        a = (a + v as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut png, b"IHDR", &ihdr);
    chunk(&mut png, b"IDAT", &z);
    chunk(&mut png, b"IEND", &[]);
    Ok(png)
}
fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(bytes);
    let mut crc = !0u32;
    for &b in kind.iter().chain(bytes) {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ if crc & 1 != 0 { 0xedb88320 } else { 0 };
        }
    }
    out.extend_from_slice(&(!crc).to_be_bytes());
}
