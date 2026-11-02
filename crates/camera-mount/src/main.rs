use fujimod_usb::ff80::commands::memory::DebugMemoryLimits;
use std::{num::NonZeroU32, path::PathBuf};

const USAGE: &str = "Usage: fujimod-mount --firmware ID [--device BUS-OR-SERIAL] [--read-limit BYTES] [--write-limit BYTES] MOUNTPOINT\nRequires an installed, enabled FSKitBridge extension on macOS 15.4+.";

struct Options {
    firmware: String,
    device: Option<String>,
    mount: PathBuf,
    limits: DebugMemoryLimits,
}
fn options(args: impl Iterator<Item = String>) -> std::result::Result<Options, String> {
    let mut args = args.peekable();
    let mut firmware = None;
    let mut device = std::env::var("FUJIMOD_USB_DEVICE").ok();
    let mut mount = None;
    let mut read = NonZeroU32::new(256).expect("constant");
    let mut write = read;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--firmware" => firmware = Some(args.next().ok_or(USAGE)?),
            "--device" => device = Some(args.next().ok_or(USAGE)?),
            "--read-limit" | "--write-limit" => {
                let n = args
                    .next()
                    .ok_or(USAGE)?
                    .parse::<NonZeroU32>()
                    .map_err(|_| "Expected a nonzero command limit")?;
                if arg == "--read-limit" {
                    read = n;
                } else {
                    write = n;
                }
            }
            "--help" | "-h" => return Err(USAGE.into()),
            value if !value.starts_with('-') && mount.is_none() => {
                mount = Some(PathBuf::from(value))
            }
            _ => return Err(USAGE.into()),
        }
    }
    Ok(Options {
        firmware: firmware.ok_or(USAGE)?,
        device,
        mount: mount.ok_or(USAGE)?,
        limits: DebugMemoryLimits { read, write },
    })
}

#[cfg(target_os = "macos")]
#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("fujimod-mount: {error}");
        std::process::exit(1);
    }
}

#[cfg(target_os = "macos")]
async fn run() -> std::result::Result<(), Box<dyn std::error::Error>> {
    use fujimod_camera_mount::{CameraFs, fskit::Adapter, usb::UsbDevice};
    use std::os::unix::fs::MetadataExt;
    if std::env::args().skip(1).any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return Ok(());
    }
    let options = options(std::env::args().skip(1))?;
    let metadata = std::fs::metadata(&options.mount)?;
    if !metadata.is_dir() || std::fs::read_dir(&options.mount)?.next().is_some() {
        return Err("Mountpoint must be an empty existing directory".into());
    }
    let (device, regions) = UsbDevice::connect(options.firmware, options.device, options.limits)?;
    let fs = CameraFs::new(device, regions).map_err(std::io::Error::from_raw_os_error)?;
    let adapter = Adapter::new(fs, metadata.uid(), metadata.gid());
    let session = fskit_rs::mount(
        adapter,
        fskit_rs::MountOptions {
            mount_point: options.mount.clone(),
            force: false,
            ..Default::default()
        },
    )
    .await?;
    println!(
        "Mounted {}. Press Ctrl+C to unmount.",
        options.mount.display()
    );
    tokio::signal::ctrl_c().await?;
    drop(session);
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("fujimod-mount currently requires macOS FSKit");
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_options_fail_before_device_access() {
        for args in [
            vec![],
            vec!["--firmware", "body"],
            vec!["--firmware", "body", "--read-limit", "0", "mount"],
        ] {
            assert!(options(args.into_iter().map(str::to_owned)).is_err());
        }
        let value = options(
            ["--firmware", "body", "mount"]
                .into_iter()
                .map(str::to_owned),
        )
        .unwrap();
        assert_eq!(value.firmware, "body");
        assert_eq!(value.limits.read.get(), 256);
    }
}
