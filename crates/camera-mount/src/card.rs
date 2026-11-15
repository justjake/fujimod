//! `fujimod-card`: SD card files over the debug-USB file service, without a mount.
use fujimod_camera_mount::{
    CameraFs, Device, IO_BYTES, Node, files, read_only::ReadOnly, usb::UsbDevice,
};
use fujimod_usb::ff80::commands::memory::DebugMemoryLimits;
use sha2::{Digest, Sha256};
use std::{
    num::NonZeroU32,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

/// Set by SIGINT or SIGTERM. Killing the process mid-request leaves the camera
/// session's completion unknown, so the command instead finishes the file in
/// flight, skips the rest and closes the session normally.
static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn stop(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

fn stopped() -> Result<(), String> {
    if STOP.load(Ordering::Relaxed) {
        Err("interrupted; the session closed after the request in flight".into())
    } else {
        Ok(())
    }
}

const USAGE: &str = "Usage: fujimod-card --firmware ID [--device BUS-OR-SERIAL] COMMAND
  list PATTERN...                    print each matching file's or folder's name and size
  get [--limit BYTES] --out DIR PATTERN...
                                     save each matching file's first BYTES (default: all)
  put LOCAL=PATH...                  write each local file as the whole card file,
                                     creating or resizing it, and verify it by readback
  rm PATH...                         delete each card file
  batch [--write]                    run one command per stdin line, a JSON array
                                     such as [\"rm\",\"PATH\"], in one camera session;
                                     put and rm need --write
Paths are card paths such as DCIM/100_FUJI/DSCF*.JPG; only the last component
may hold * or ?. Each file prints one JSON line with its SHA-256.";

/// Files per camera transaction.
const BATCH_FILES: usize = 32;

fn fail(message: impl std::fmt::Display) -> ! {
    eprintln!("fujimod-card: {message}");
    std::process::exit(1)
}

fn errno(path: &str, code: i32) -> String {
    format!("{path}: {}", std::io::Error::from_raw_os_error(code))
}

fn hash(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}

/// One command against an open file system.
fn execute<D: Device>(
    fs: &mut CameraFs<D>,
    command: &str,
    args: Vec<String>,
) -> Result<(), String> {
    match command {
        "list" | "get" => {
            let mut args = args.into_iter();
            let (mut limit, mut out, mut patterns) = (None, None, vec![]);
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--limit" if command == "get" => {
                        limit = Some(
                            args.next()
                                .and_then(|v| v.parse().ok())
                                .filter(|v| *v > 0)
                                .ok_or(USAGE)?,
                        )
                    }
                    "--out" if command == "get" => {
                        out = Some(PathBuf::from(args.next().ok_or(USAGE)?))
                    }
                    _ => patterns.push(arg),
                }
            }
            if patterns.is_empty() || command == "get" && out.is_none() {
                return Err(USAGE.to_owned());
            }
            let mut nodes = Vec::new();
            for pattern in &patterns {
                nodes.extend(files::list(fs, pattern).map_err(|e| errno(pattern, e))?);
            }
            let Some(out) = out else {
                for node in nodes {
                    println!(
                        "{}",
                        serde_json::json!({"path": node.path, "bytes": node.size, "directory": node.directory})
                    );
                }
                return Ok(());
            };
            let save = |node: &Node, data: &[u8]| {
                let file = out.join(&node.name);
                std::fs::write(&file, data).map_err(|e| format!("{}: {e}", file.display()))?;
                println!(
                    "{}",
                    serde_json::json!({"path": node.path, "bytes": node.size,
                        "saved": data.len(), "file": file, "sha256": hash(data)})
                );
                Ok::<_, String>(())
            };
            let prefix = |node: &Node| limit.map_or(node.size, |l: u64| l.min(node.size));
            let mut rest = &nodes[..];
            while let Some(first) = rest.first() {
                stopped()?;
                // Files in a batch share one camera transaction; its bounds
                // also bound how long an interrupt waits.
                let (mut count, mut bytes) = (0, 0);
                while count < rest.len().min(BATCH_FILES)
                    && !rest[count].directory
                    && bytes + prefix(&rest[count]) <= IO_BYTES as u64
                {
                    bytes += prefix(&rest[count]);
                    count += 1;
                }
                if count == 0 {
                    if first.directory {
                        println!(
                            "{}",
                            serde_json::json!({"path": first.path, "bytes": first.size, "directory": true})
                        );
                    } else {
                        let data =
                            files::get(fs, first, limit).map_err(|e| errno(&first.name, e))?;
                        save(first, &data)?;
                    }
                    count = 1;
                } else {
                    let batch = &rest[..count];
                    let data =
                        files::get_many(fs, batch, limit).map_err(|e| errno(&first.name, e))?;
                    for (node, data) in batch.iter().zip(data) {
                        save(node, &data)?;
                    }
                }
                rest = &rest[count..];
            }
            Ok(())
        }
        "put" if !args.is_empty() => {
            for pair in &args {
                stopped()?;
                let (local, path) = pair.split_once('=').ok_or(USAGE)?;
                let data = std::fs::read(local).map_err(|e| format!("{local}: {e}"))?;
                let sha256 = files::put(fs, path, &data).map_err(|e| errno(path, e))?;
                println!(
                    "{}",
                    serde_json::json!({"path": path, "local": local, "bytes": data.len(), "sha256": sha256, "verified": true})
                );
            }
            Ok(())
        }
        "rm" if !args.is_empty() => {
            for path in &args {
                stopped()?;
                files::remove(fs, path).map_err(|e| errno(path, e))?;
                println!("{}", serde_json::json!({"path": path, "removed": true}));
            }
            Ok(())
        }
        _ => Err(USAGE.to_owned()),
    }
}

/// Runs one command per stdin line, a JSON array such as
/// `["get","--out","DIR","DCIM/100_FUJI/*.JPG"]`, in one camera session, and
/// prints `{"batch":N,"ok":...}` after each command's own lines. A failed
/// command does not end the batch; an interrupt ends it between commands
/// or between files.
fn batch<D: Device>(
    fs: &mut CameraFs<D>,
    lines: mpsc::Receiver<std::io::Result<String>>,
) -> Result<(), String> {
    let mut number = 0;
    loop {
        stopped()?;
        let line = match lines.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => line.map_err(|e| format!("stdin: {e}"))?,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
        };
        if line.trim().is_empty() {
            continue;
        }
        number += 1;
        let result = serde_json::from_str::<Vec<String>>(&line)
            .map_err(|e| format!("batch command {number}: {e}"))
            .and_then(|mut words| match words.first().map(String::as_str) {
                Some("list" | "get" | "put" | "rm") => {
                    let command = words.remove(0);
                    execute(fs, &command, words)
                }
                _ => Err(USAGE.to_owned()),
            });
        let mut report = serde_json::json!({"batch": number, "ok": result.is_ok()});
        if let Err(error) = &result {
            report["error"] = error.as_str().into();
        }
        println!("{report}");
        stopped()?;
    }
}

fn run<D: Device>(
    device: D,
    regions: Vec<fujimod_camera_mount::Region>,
    command: &str,
    args: Vec<String>,
) -> Result<(), String> {
    let mut fs = CameraFs::new(device, regions).map_err(|e| errno("card", e))?;
    fs.keep_listings();
    let result = if command == "batch" {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in std::io::stdin().lines() {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        batch(&mut fs, rx)
    } else {
        execute(&mut fs, command, args)
    };
    let started = Instant::now();
    let shutdown = fs.shutdown().map_err(|e| errno("shutdown", e));
    trace("shutdown", started);
    result.and(shutdown)
}

fn trace(step: &str, started: Instant) {
    if std::env::var_os("FUJIMOD_MOUNT_TRACE").is_some() {
        eprintln!(
            "{}",
            serde_json::json!({"card": step, "seconds": started.elapsed().as_secs_f64()})
        );
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let (mut firmware, mut device) = (None, std::env::var("FUJIMOD_USB_DEVICE").ok());
    let command = loop {
        match args.next().as_deref() {
            Some("--firmware") => firmware = args.next(),
            Some("--device") => device = args.next(),
            Some(command @ ("list" | "get" | "put" | "rm" | "batch")) => break command.to_owned(),
            Some("--help" | "-h") => {
                println!("{USAGE}");
                return;
            }
            _ => fail(USAGE),
        }
    };
    let firmware = firmware.unwrap_or_else(|| fail(USAGE));
    let rest: Vec<String> = args.collect();
    for signal in [libc::SIGINT, libc::SIGTERM] {
        // SAFETY: the handler only stores to an atomic.
        unsafe { libc::signal(signal, stop as *const () as libc::sighandler_t) };
    }
    let writes = match command.as_str() {
        "put" | "rm" => true,
        "batch" if rest.is_empty() => false,
        "batch" if rest == ["--write"] => true,
        "batch" => fail(USAGE),
        _ => false,
    };
    let page = NonZeroU32::new(256).expect("constant");
    let started = Instant::now();
    let (usb, regions) = UsbDevice::connect(
        firmware,
        device,
        DebugMemoryLimits {
            read: page,
            write: page,
        },
        None,
        true,
    )
    .unwrap_or_else(|e| fail(e));
    trace("connect", started);
    // Only put, rm and batch --write may change the card.
    let result = if writes {
        run(usb, regions, &command, rest)
    } else {
        run(ReadOnly(usb), regions, &command, rest)
    };
    if let Err(error) = result {
        fail(error);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_usb::operations::card::DirectoryEntry;
    use std::collections::BTreeMap;

    struct Card(BTreeMap<String, Vec<u8>>);
    impl Device for Card {
        fn has_card(&self) -> bool {
            true
        }
        fn list(&mut self, path: &str) -> fujimod_camera_mount::Result<Vec<DirectoryEntry>> {
            let prefix = format!("{}\\", path.trim_end_matches('\\'));
            Ok(self
                .0
                .iter()
                .filter_map(|(name, data)| {
                    Some(DirectoryEntry {
                        name: name.strip_prefix(&prefix)?.into(),
                        bytes: data.len() as u32,
                        directory: false,
                        read_only: false,
                    })
                })
                .collect())
        }
        fn read(
            &mut self,
            path: &str,
            offset: u32,
            size: u32,
        ) -> fujimod_camera_mount::Result<Vec<u8>> {
            let data = self.0.get(path).ok_or(libc::ENOENT)?;
            let offset = (offset as usize).min(data.len());
            Ok(data[offset..data.len().min(offset + size as usize)].to_vec())
        }
        fn write(
            &mut self,
            path: &str,
            offset: u32,
            data: &[u8],
        ) -> fujimod_camera_mount::Result<()> {
            let file = self.0.get_mut(path).ok_or(libc::ENOENT)?;
            file.resize(file.len().max(offset as usize + data.len()), 0);
            file[offset as usize..offset as usize + data.len()].copy_from_slice(data);
            Ok(())
        }
        fn create(&mut self, path: &str) -> fujimod_camera_mount::Result<()> {
            self.0.insert(path.into(), vec![]);
            Ok(())
        }
        fn delete(&mut self, path: &str) -> fujimod_camera_mount::Result<()> {
            self.0.remove(path).map(|_| ()).ok_or(libc::ENOENT)
        }
        fn shrink(&mut self, path: &str, size: u32) -> fujimod_camera_mount::Result<()> {
            self.0
                .get_mut(path)
                .ok_or(libc::ENOENT)?
                .truncate(size as usize);
            Ok(())
        }
        fn memory(&mut self, _: u32, _: u32) -> fujimod_camera_mount::Result<Vec<u8>> {
            Err(libc::ENOTSUP)
        }
        fn flash_size(&self) -> Option<u64> {
            None
        }
        fn flash(&mut self, _: u64, _: u32) -> fujimod_camera_mount::Result<Vec<u8>> {
            Err(libc::ENOTSUP)
        }
        fn synchronize(&mut self) -> fujimod_camera_mount::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_batch_runs_every_command_in_one_file_system_and_stops_on_interrupt() {
        let local = std::env::temp_dir().join(format!("fujimod-card-batch-{}", std::process::id()));
        std::fs::write(&local, b"new").unwrap();
        let card = Card(BTreeMap::from([(
            "A:\\OLD.TXT".to_owned(),
            b"old".to_vec(),
        )]));
        let mut fs = CameraFs::new(card, vec![]).unwrap();
        fs.keep_listings();
        let (tx, rx) = mpsc::channel();
        for words in [
            vec!["put".to_owned(), format!("{}=NEW.TXT", local.display())],
            vec!["rm".to_owned(), "MISSING.TXT".to_owned()],
            vec!["rm".to_owned(), "OLD.TXT".to_owned()],
        ] {
            tx.send(Ok(serde_json::to_string(&words).unwrap())).unwrap();
        }
        drop(tx);
        // The failed rm is reported and the batch goes on.
        assert_eq!(batch(&mut fs, rx), Ok(()));
        let names: Vec<_> = files::list(&mut fs, "*")
            .unwrap()
            .into_iter()
            .map(|n| n.name)
            .collect();
        assert_eq!(names, ["NEW.TXT"]);

        let (tx, rx) = mpsc::channel();
        tx.send(Ok(r#"["rm","NEW.TXT"]"#.to_owned())).unwrap();
        STOP.store(true, Ordering::Relaxed);
        assert!(batch(&mut fs, rx).is_err());
        STOP.store(false, Ordering::Relaxed);
        assert_eq!(files::list(&mut fs, "NEW.TXT").unwrap().len(), 1);
        std::fs::remove_file(local).unwrap();
    }
}
