//! `fujimod-raw-convert`: develop RAFs on a camera in USB RAW CONV. mode
//! (crates/raw-conversion/README.md).
use fujimod_raw_conversion::{ptp::Session, run};
use fujimod_usb::connection::{self, Connection, DeviceFilter, TransferOptions};
use std::{collections::BTreeMap, num::NonZeroU32, path::PathBuf, time::Duration};

const USAGE: &str = "usage: fujimod-raw-convert --jobs JOBS.json --out DIR [--raf NAME=PATH]... \
[--dry-run] [--upload-each] [--conversion-timeout SECONDS]";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(message) = futures_lite::future::block_on(main_async(args)) {
        eprintln!("fujimod-raw-convert: {message}");
        std::process::exit(1);
    }
}

async fn main_async(args: Vec<String>) -> Result<(), String> {
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
    let done = run::finished(&options.out)?;
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
    let mut session = Session::open(connection.into_bulk().map_err(|e| e.to_string())?)
        .await
        .map_err(|e| e.to_string())?;
    let result = run::execute(&mut session, &file, &options, |line| eprintln!("{line}")).await;
    // Close only after a clean run; after a failure the link state is unknown.
    if result.is_ok() {
        let pipe = session.close().await.map_err(|e| e.to_string())?;
        pipe.close().await.map_err(|e| e.to_string())?;
    }
    result
}
