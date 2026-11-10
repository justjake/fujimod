//! `fujimod-raw-convert`: develop RAFs on a camera in USB RAW CONV. mode
//! (crates/raw-conversion/README.md).
use fujimod_raw_conversion::{card, ptp::Session, run};
use fujimod_usb::connection::{self, BulkPipe, Connection, DeviceFilter, TransferOptions};
use std::{collections::BTreeMap, num::NonZeroU32, path::PathBuf, time::Duration};

const USAGE: &str = "usage:
  fujimod-raw-convert --jobs JOBS.json --out DIR [--raf NAME=PATH]... [--dry-run] [--upload-each] [--conversion-timeout SECONDS]
  fujimod-raw-convert card list
  fujimod-raw-convert card fetch --dir DIR NAME=FILE...";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(message) = futures_lite::future::block_on(main_async(args)) {
        eprintln!("fujimod-raw-convert: {message}");
        std::process::exit(1);
    }
}

async fn main_async(args: Vec<String>) -> Result<(), String> {
    if args.first().map(String::as_str) == Some("card") {
        return card(&args[1..]).await;
    }
    let (mut jobs, mut out, mut dry_run, mut upload_each) = (None, None, false, false);
    let mut rafs = BTreeMap::new();
    let mut timeout = Duration::from_secs(120);
    let mut items = args.iter();
    while let Some(item) = items.next() {
        let mut value = || items.next().cloned().ok_or(USAGE);
        match item.as_str() {
            "--jobs" => jobs = Some(PathBuf::from(value()?)),
            "--out" => out = Some(PathBuf::from(value()?)),
            "--raf" => {
                let v = value()?;
                let (name, path) = v.split_once('=').ok_or(USAGE)?;
                rafs.insert(name.to_owned(), PathBuf::from(path));
            }
            "--dry-run" => dry_run = true,
            "--upload-each" => upload_each = true,
            "--conversion-timeout" => {
                timeout = Duration::from_secs(value()?.parse().map_err(|_| USAGE)?)
            }
            _ => return Err(USAGE.into()),
        }
    }
    let file = run::JobFile::load(&jobs.ok_or(USAGE)?)?;
    let options = run::Options {
        out: out.ok_or(USAGE)?,
        rafs,
        upload_each,
        conversion_timeout: timeout,
    };
    let done = run::finished(&options.out);
    if dry_run {
        for line in run::describe(&file, &options, &done)? {
            println!("{line}");
        }
        let pending = file.jobs.iter().filter(|j| !done.contains(&j.id)).count();
        println!(
            "{} jobs, {} done, {pending} pending",
            file.jobs.len(),
            done.len()
        );
        return Ok(());
    }

    let mut session = open().await?;
    let result = run::execute(&mut session, &file, &options, |line| eprintln!("{line}")).await;
    // Close only after a clean run; after a failure the link state is unknown.
    if result.is_ok() {
        let pipe = session.close().await.map_err(|e| e.to_string())?;
        pipe.close().await.map_err(|e| e.to_string())?;
    }
    result
}

async fn open() -> Result<Session<BulkPipe>, String> {
    // FUJIMOD_USB_PROXY_URL selects a shared camera; otherwise local USB.
    let devices = connection::discover(DeviceFilter::Fujifilm)
        .await
        .map_err(|e| e.to_string())?;
    let [device] = devices.as_slice() else {
        return Err(format!(
            "expected one Fujifilm camera, found {}",
            devices.len()
        ));
    };
    let selections = connection::interfaces(device)
        .await
        .map_err(|e| e.to_string())?;
    let [selection] = selections.as_slice() else {
        return Err(format!(
            "expected one bulk interface, found {}",
            selections.len()
        ));
    };
    let connection = Connection::open(
        device,
        *selection,
        TransferOptions {
            timeout: Duration::from_secs(30),
            max_reply_bytes: NonZeroU32::new(1 << 20).unwrap(),
        },
    )
    .await
    .map_err(|e| e.to_string())?;
    Session::open(connection.into_bulk().map_err(|e| e.to_string())?)
        .await
        .map_err(|e| e.to_string())
}

/// USB CARD READER mode: list the SD card's RAFs, or download chosen ones.
async fn card(args: &[String]) -> Result<(), String> {
    let (command, rest) = args.split_first().ok_or(USAGE)?;
    let mut dir = None;
    let mut picks = Vec::new();
    let mut items = rest.iter();
    while let Some(item) = items.next() {
        match item.as_str() {
            "--dir" => dir = Some(PathBuf::from(items.next().ok_or(USAGE)?)),
            pick => picks.push(pick.split_once('=').ok_or(USAGE)?),
        }
    }
    let mut session = open().await?;
    let rafs = card::list(&mut session).await?;
    if rafs.is_empty() {
        return Err("no RAFs; set CONNECTION SETTING > CONNECTION MODE to USB CARD READER".into());
    }
    match command.as_str() {
        "list" if dir.is_none() && picks.is_empty() => {
            for r in &rafs {
                let iso = r.iso.map_or("?".into(), |i| i.to_string());
                println!(
                    "{:16} ISO {iso:>6}  {}  {} bytes",
                    r.name, r.captured, r.size
                );
            }
        }
        "fetch" if !picks.is_empty() => {
            let dir = dir.ok_or(USAGE)?;
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let mut flags = Vec::new();
            for (name, file) in picks {
                let raf = rafs
                    .iter()
                    .find(|r| r.name == file)
                    .ok_or(format!("{file} is not on the SD card"))?;
                let path = dir.join(&raf.name);
                // A complete earlier download is kept.
                if std::fs::metadata(&path).map(|m| m.len()).ok() != Some(u64::from(raf.size)) {
                    eprintln!("downloading {} ({} bytes)", raf.name, raf.size);
                    let bytes = card::fetch(&mut session, raf.handle).await?;
                    let partial = path.with_extension("RAF.partial");
                    std::fs::write(&partial, bytes).map_err(|e| e.to_string())?;
                    std::fs::rename(&partial, &path).map_err(|e| e.to_string())?;
                }
                flags.push(format!("--raf {name}={}", path.display()));
            }
            println!("{}", flags.join(" "));
        }
        _ => return Err(USAGE.into()),
    }
    let pipe = session.close().await.map_err(|e| e.to_string())?;
    pipe.close().await.map_err(|e| e.to_string())
}
