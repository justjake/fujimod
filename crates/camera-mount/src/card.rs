//! `fujimod-card`: SD card files over the debug-USB file service, without a mount.
use fujimod_camera_mount::{CameraFs, Device, Node, files, read_only::ReadOnly, usb::UsbDevice};
use fujimod_usb::ff80::commands::memory::DebugMemoryLimits;
use sha2::{Digest, Sha256};
use std::{num::NonZeroU32, path::PathBuf};

const USAGE: &str = "Usage: fujimod-card --firmware ID [--device BUS-OR-SERIAL] COMMAND
  list PATTERN...                    print each matching file's name and size
  get [--limit BYTES] --out DIR PATTERN...
                                     save each matching file's first BYTES (default: all)
  put LOCAL=PATH...                  write each local file as the whole card file,
                                     creating or resizing it, and verify it by readback
Paths are card paths such as DCIM/100_FUJI/DSCF*.JPG; only the last component
may hold * or ?. Each file prints one JSON line with its SHA-256.";

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

fn run<D: Device>(
    device: D,
    regions: Vec<fujimod_camera_mount::Region>,
    command: &str,
    args: Vec<String>,
) -> Result<(), String> {
    let mut fs = CameraFs::new(device, regions).map_err(|e| errno("card", e))?;
    let result = (|| match command {
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
            for pattern in &patterns {
                let nodes: Vec<Node> =
                    files::list(&mut fs, pattern).map_err(|e| errno(pattern, e))?;
                for node in nodes {
                    let mut line = serde_json::json!({"path": node.path, "bytes": node.size});
                    if let Some(out) = &out {
                        let data =
                            files::get(&mut fs, &node, limit).map_err(|e| errno(&node.name, e))?;
                        let file = out.join(&node.name);
                        std::fs::write(&file, &data)
                            .map_err(|e| format!("{}: {e}", file.display()))?;
                        line = serde_json::json!({"path": node.path, "bytes": node.size,
                            "saved": data.len(), "file": file, "sha256": hash(&data)});
                    }
                    println!("{line}");
                }
            }
            Ok(())
        }
        "put" if !args.is_empty() => {
            for pair in &args {
                let (local, path) = pair.split_once('=').ok_or(USAGE)?;
                let data = std::fs::read(local).map_err(|e| format!("{local}: {e}"))?;
                let sha256 = files::put(&mut fs, path, &data).map_err(|e| errno(path, e))?;
                println!(
                    "{}",
                    serde_json::json!({"path": path, "local": local, "bytes": data.len(), "sha256": sha256, "verified": true})
                );
            }
            Ok(())
        }
        _ => Err(USAGE.to_owned()),
    })();
    let shutdown = fs.shutdown().map_err(|e| errno("shutdown", e));
    result.and(shutdown)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let (mut firmware, mut device) = (None, std::env::var("FUJIMOD_USB_DEVICE").ok());
    let command = loop {
        match args.next().as_deref() {
            Some("--firmware") => firmware = args.next(),
            Some("--device") => device = args.next(),
            Some(command @ ("list" | "get" | "put")) => break command.to_owned(),
            Some("--help" | "-h") => {
                println!("{USAGE}");
                return;
            }
            _ => fail(USAGE),
        }
    };
    let firmware = firmware.unwrap_or_else(|| fail(USAGE));
    let rest: Vec<String> = args.collect();
    let page = NonZeroU32::new(256).expect("constant");
    let (usb, regions) = UsbDevice::connect(
        firmware,
        device,
        DebugMemoryLimits {
            read: page,
            write: page,
        },
        None,
    )
    .unwrap_or_else(|e| fail(e));
    // Only put may change the card.
    let result = if command == "put" {
        run(usb, regions, &command, rest)
    } else {
        run(ReadOnly(usb), regions, &command, rest)
    };
    if let Err(error) = result {
        fail(error);
    }
}
