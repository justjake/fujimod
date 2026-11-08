use fujimod_usb::ff80::commands::memory::DebugMemoryLimits;
use std::{num::NonZeroU32, path::PathBuf};

const USAGE: &str = "Usage: fujimod-mount --firmware ID [--device BUS-OR-SERIAL] [--read-limit BYTES] [--write-limit BYTES] [--flash-reader ARTIFACT] [--benchmark | --benchmark-usb] MOUNTPOINT\nRequires macOS 15.4+. Installs the signed filesystem extension on first use; macOS may require enabling it in System Settings.";
#[cfg(target_os = "macos")]
mod benchmark;
/// Exit status when the camera has no installed file service, as on stock firmware.
#[cfg(target_os = "macos")]
const NO_FILE_SERVICE: i32 = 3;

struct Options {
    firmware: String,
    device: Option<String>,
    mount: PathBuf,
    limits: DebugMemoryLimits,
    flash_reader: Option<PathBuf>,
    benchmark: bool,
    benchmark_usb: bool,
}
fn options(args: impl Iterator<Item = String>) -> std::result::Result<Options, String> {
    let mut args = args.peekable();
    let mut firmware = None;
    let mut device = std::env::var("FUJIMOD_USB_DEVICE").ok();
    let mut mount = None;
    let mut flash_reader = std::env::var_os("FUJIMOD_FLASH_READER").map(PathBuf::from);
    let mut read = NonZeroU32::new(256).expect("constant");
    let mut benchmark = false;
    let mut benchmark_usb = false;
    let mut write = read;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--firmware" => firmware = Some(args.next().ok_or(USAGE)?),
            "--device" => device = Some(args.next().ok_or(USAGE)?),
            "--flash-reader" => flash_reader = Some(PathBuf::from(args.next().ok_or(USAGE)?)),
            "--benchmark" => benchmark = true,
            "--benchmark-usb" => benchmark_usb = true,
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
        flash_reader,
        benchmark,
        benchmark_usb,
    })
}

#[cfg(target_os = "macos")]
#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("fujimod-mount: {error}");
        let no_service = matches!(
            error.downcast_ref(),
            Some(fujimod_usb::Error::NoFileService)
        );
        std::process::exit(if no_service { NO_FILE_SERVICE } else { 1 });
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
    if !options.benchmark_usb {
        fujimod_camera_mount::bridge::prepare()?;
        fujimod_camera_mount::fskit::prepare_bridge_resource()?;
    }
    let (mut device, regions) = UsbDevice::connect(
        options.firmware,
        options.device,
        options.limits,
        options.flash_reader,
    )
    .map_err(|error| error as Box<dyn std::error::Error>)?;
    if options.benchmark_usb {
        use fujimod_camera_mount::Device;
        let result = benchmark::usb(&mut device, &regions);
        device
            .shutdown()
            .map_err(std::io::Error::from_raw_os_error)?;
        println!("{}", serde_json::to_string_pretty(&result?)?);
        return Ok(());
    }
    let fatal = device.fatal();
    let fs = CameraFs::new(device, regions.clone()).map_err(std::io::Error::from_raw_os_error)?;
    let adapter = Adapter::new(fs, metadata.uid(), metadata.gid());
    let cleanup = adapter.clone();
    let mounted = fskit_rs::mount(
        adapter,
        fskit_rs::MountOptions {
            mount_point: options.mount.clone(),
            force: false,
            ..Default::default()
        },
    )
    .await;
    let session = match mounted {
        Ok(session) => session,
        Err(error) => {
            cleanup.shutdown().await?;
            return Err(error.into());
        }
    };
    println!(
        "Mounted {}. Press Ctrl+C to unmount.",
        options.mount.display()
    );
    if options.benchmark {
        let mount = options.mount.clone();
        let result = tokio::task::spawn_blocking(move || benchmark::run(&mount, &regions)).await?;
        // Unmount even when the benchmark reports an I/O error.
        drop(session);
        cleanup.shutdown().await?;
        println!("{}", serde_json::to_string_pretty(&result?)?);
        return Ok(());
    }
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let lost = tokio::select! {
        _ = tokio::signal::ctrl_c() => false,
        _ = terminate.recv() => true,
        _ = async {
            while !fatal.load(std::sync::atomic::Ordering::Relaxed) {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
        } => {
            eprintln!("fujimod-mount: camera session lost; unmounting");
            true
        }
    };
    drop(session);
    let shutdown = cleanup.shutdown().await;
    if lost {
        if let Err(error) = shutdown {
            eprintln!("fujimod-mount: shutdown after session loss: {error}");
        }
        std::process::exit(1);
    }
    shutdown?;
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
