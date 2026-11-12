//! `fujimod-raw-convert`: develop RAFs on a camera in USB RAW CONV. mode
//! (crates/raw-conversion/README.md).
use fujimod_raw_conversion::{card, chart_slot, ptp::Session, run};
use fujimod_usb::connection::{self, BulkPipe, Connection, DeviceFilter, TransferOptions};
use std::{collections::BTreeMap, num::NonZeroU32, path::PathBuf, time::Duration};

const USAGE: &str = "usage:
  fujimod-raw-convert --jobs JOBS.json --out DIR [--raf NAME=PATH]... [--dry-run | --checklist] [--upload-each] [--conversion-timeout SECONDS]
  fujimod-raw-convert card list
  fujimod-raw-convert card fetch --dir DIR NAME=FILE...
  fujimod-raw-convert chart-slot save --slot N --saved FILE.json
  fujimod-raw-convert chart-slot write --saved FILE.json
  fujimod-raw-convert chart-slot write --slot N --dry-run
  fujimod-raw-convert chart-slot restore --saved FILE.json";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(message) = futures_lite::future::block_on(main_async(args)) {
        eprintln!("fujimod-raw-convert: {message}");
        std::process::exit(1);
    }
}

async fn main_async(args: Vec<String>) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("card") => return card(&args[1..]).await,
        Some("chart-slot") => return chart(&args[1..]).await,
        Some("--help" | "-h") => {
            println!("{USAGE}\n\nchart-slot: {}", chart_slot::UNVERIFIED);
            return Ok(());
        }
        _ => {}
    }
    let (mut jobs, mut out, mut dry_run, mut upload_each) = (None, None, false, false);
    let mut checklist = false;
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
            "--checklist" => checklist = true,
            "--upload-each" => upload_each = true,
            "--conversion-timeout" => {
                timeout = Duration::from_secs(value()?.parse().map_err(|_| USAGE)?)
            }
            _ => return Err(USAGE.into()),
        }
    }
    let file = run::JobFile::load(&jobs.ok_or(USAGE)?)?;
    if checklist || dry_run {
        println!("Capture checklist:");
        for line in run::checklist(&file) {
            println!("{line}");
        }
        if checklist {
            return Ok(());
        }
        println!();
    }
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

/// An X100VI 1.31 custom-setting slot for shooting a test chart
/// (crates/raw-conversion/README.md).
async fn chart(args: &[String]) -> Result<(), String> {
    let (command, rest) = args.split_first().ok_or(USAGE)?;
    let (mut slot, mut saved, mut dry_run) = (None, None, false);
    let mut items = rest.iter();
    while let Some(item) = items.next() {
        match item.as_str() {
            "--slot" => {
                slot = Some(
                    items
                        .next()
                        .ok_or(USAGE)?
                        .parse::<u8>()
                        .map_err(|_| USAGE)?,
                )
            }
            "--saved" => saved = Some(PathBuf::from(items.next().ok_or(USAGE)?)),
            "--dry-run" => dry_run = true,
            _ => return Err(USAGE.into()),
        }
    }
    let load = |path: &PathBuf| -> Result<chart_slot::Saved, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
    };
    let log = |line: &str| eprintln!("{line}");
    match (command.as_str(), slot, saved, dry_run) {
        ("write", Some(slot), None, true) => {
            for line in chart_slot::plan(slot) {
                println!("{line}");
            }
            return Ok(());
        }
        ("save", Some(slot), Some(path), false) => {
            if path.exists() {
                return Err(format!(
                    "{} exists; a saved slot is never overwritten",
                    path.display()
                ));
            }
            let mut session = open().await?;
            let saved = chart_slot::save(&mut session, slot).await?;
            let text = serde_json::to_string_pretty(&saved).map_err(|e| e.to_string())?;
            std::fs::write(&path, text + "\n").map_err(|e| e.to_string())?;
            eprintln!("saved C{slot} ({:?}) to {}", saved.name, path.display());
            close(session).await?;
        }
        ("write", None, Some(path), false) => {
            let saved = load(&path)?;
            eprintln!("{}", chart_slot::UNVERIFIED);
            let mut session = open().await?;
            chart_slot::write(&mut session, &saved, log).await?;
            close(session).await?;
        }
        ("restore", None, Some(path), false) => {
            let saved = load(&path)?;
            let mut session = open().await?;
            chart_slot::restore(&mut session, &saved, log).await?;
            close(session).await?;
        }
        _ => return Err(USAGE.into()),
    }
    Ok(())
}

async fn close(session: Session<BulkPipe>) -> Result<(), String> {
    let pipe = session.close().await.map_err(|e| e.to_string())?;
    pipe.close().await.map_err(|e| e.to_string())
}
