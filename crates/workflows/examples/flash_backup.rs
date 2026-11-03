//! Save a whole-flash backup using a prepared reader artifact and debug access.
//! flash_backup ARTIFACT OUTPUT_DIRECTORY [--execute]
//! Without --execute, validate the artifact offline. Live device selection uses
//! FUJIMOD_USB_DEVICE (bus-port hex) or FUJIMOD_USB_PROXY_URL.
use fujimod_catalog::ReferenceCatalog;
use fujimod_usb::{
    connection::{self, Connection, DeviceFilter, TransferOptions},
    ff80::{SessionState, commands::memory::DebugMemoryLimits},
    trace::{Action, Trace},
};
use fujimod_workflows::{Limits, flash};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    num::NonZeroU32,
    path::PathBuf,
    time::{Duration, Instant, SystemTime},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Artifact {
    payload: flash::Payload,
    image: String,
}

struct Sink {
    file: File,
    size: u64,
    spans: BTreeMap<u64, u64>,
    delivered: u64,
}

#[derive(Default)]
struct Timings {
    pending: BTreeMap<u64, (u32, SystemTime)>,
    commands: BTreeMap<u32, (u64, f64)>,
    dropped: u64,
}
impl Timings {
    fn collect(&mut self, trace: &Trace) {
        let batch = trace.drain();
        self.dropped += batch.dropped_events;
        for event in batch.protocol_events {
            match event.action {
                Action::Submitted { .. } => {
                    self.pending
                        .insert(event.exchange, (event.command, event.timestamp));
                }
                Action::Completed { .. } | Action::Failed { .. } | Action::Canceled => {
                    if let Some((command, start)) = self.pending.remove(&event.exchange) {
                        let row = self.commands.entry(command).or_default();
                        row.0 += 1;
                        row.1 += event
                            .timestamp
                            .duration_since(start)
                            .unwrap_or_default()
                            .as_secs_f64();
                    }
                }
            }
        }
    }
    fn report(&self) -> serde_json::Value {
        serde_json::json!({"dropped_trace_events":self.dropped,
            "protocol":self.commands.iter().map(|(command,(count,seconds))|
                serde_json::json!({"command":format!("{command:#x}"),"count":count,"seconds":seconds})).collect::<Vec<_>>()})
    }
}
impl Sink {
    fn put(&mut self, offset: u64, data: &[u8]) -> Result<(), String> {
        let end = offset
            .checked_add(data.len() as u64)
            .ok_or("Chunk overflow")?;
        if data.is_empty()
            || end > self.size
            || self
                .spans
                .range(..end)
                .next_back()
                .is_some_and(|(&at, &n)| at + n > offset)
        {
            return Err("Empty, overlapping or out-of-range flash chunk".into());
        }
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(|e| e.to_string())?;
        self.file.write_all(data).map_err(|e| e.to_string())?;
        self.spans.insert(offset, data.len() as u64);
        self.delivered += data.len() as u64;
        Ok(())
    }
    fn digest(&mut self) -> Result<String, Box<dyn std::error::Error>> {
        if self.delivered != self.size || self.file.metadata()?.len() != self.size {
            return Err("Incomplete flash coverage".into());
        }
        self.file.sync_all()?;
        self.file.seek(SeekFrom::Start(0))?;
        let mut digest = Sha256::new();
        let mut buffer = vec![0; 1024 * 1024];
        loop {
            let size = self.file.read(&mut buffer)?;
            if size == 0 {
                break;
            }
            digest.update(&buffer[..size]);
        }
        Ok(format!("{:x}", digest.finalize()))
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    futures_lite::future::block_on(run())
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let diagnostic = match args.get(3).map(String::as_str) {
        None => None,
        Some("--diagnostic=corrected") => Some(flash::NandReadMode::Corrected),
        Some("--diagnostic=uncorrected") => Some(flash::NandReadMode::Uncorrected),
        _ => return Err("Unknown diagnostic mode".into()),
    };
    if !(args.len() == 2 || (args.len() == 3 || args.len() == 4) && args[2] == "--execute") {
        return Err("Usage: flash_backup ARTIFACT OUTPUT_DIRECTORY [--execute]".into());
    }
    let metadata = PathBuf::from(&args[0]);
    let artifact: Artifact = serde_json::from_slice(&fs::read(&metadata)?)?;
    if artifact.image.contains(['/', '\\']) {
        return Err("Image must be beside the artifact metadata".into());
    }
    let image = fs::read(
        metadata
            .parent()
            .ok_or("Artifact directory")?
            .join(&artifact.image),
    )?;
    let selected = ReferenceCatalog::bundled()?.resolve_firmware(artifact.payload.firmware_id())?;
    let bulk = selected.bulk.ok_or("Missing bulk facts")?;
    let flash = selected.flash.ok_or("Missing flash facts")?;
    if selected
        .firmware
        .update
        .as_ref()
        .is_none_or(|update| update.sha256 != artifact.payload.firmware_sha256())
    {
        return Err("Payload update identity differs".into());
    }
    artifact.payload.validate(&flash, &bulk, &image)?;
    println!(
        "{}: validated {} reader bytes; {} flash bytes",
        flash.firmware_id,
        image.len(),
        flash.window.size
    );
    if args.len() == 2 {
        return Ok(());
    }
    let device_key = std::env::var("FUJIMOD_USB_DEVICE").ok();
    if device_key.is_none() && std::env::var("FUJIMOD_USB_PROXY_URL").is_err() {
        return Err("Select FUJIMOD_USB_DEVICE or FUJIMOD_USB_PROXY_URL".into());
    }
    let devices: Vec<_> = connection::discover(DeviceFilter::Diagnostic)
        .await?
        .into_iter()
        .filter(|d| {
            device_key.as_ref().is_none_or(|key| {
                let ports = d
                    .port_chain()
                    .iter()
                    .map(|p| format!("{p:02x}"))
                    .collect::<String>();
                *key == format!("{}-{ports}", d.bus_id())
            })
        })
        .collect();
    if devices.len() != 1 {
        return Err("Expected exactly one selected diagnostic camera".into());
    }
    let interfaces = connection::interfaces(&devices[0]).await?;
    if interfaces.len() != 1 {
        return Err("Expected one diagnostic interface".into());
    }
    let output = PathBuf::from(&args[1]);
    fs::create_dir(&output)?;
    let file = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(output.join("flash.partial"))?;
    let size = if diagnostic.is_some() {
        8 << 20
    } else {
        flash.window.size
    };
    let mut sink = Sink {
        file,
        size,
        spans: BTreeMap::new(),
        delivered: 0,
    };
    let connection = Connection::open(
        &devices[0],
        interfaces[0],
        TransferOptions {
            timeout: Duration::from_secs(10),
            max_reply_bytes: NonZeroU32::new(8 * 1024 * 1024).unwrap(),
        },
    )
    .await?;
    connection.set_tool("Whole-flash backup".into());
    let trace = Trace::new(
        NonZeroU32::new(32768).unwrap(),
        NonZeroU32::new(8 << 20).unwrap(),
    );
    let mut timings = Timings::default();
    let mut session = connection.begin_session(Some(trace.clone())).await?;
    let limits = Limits {
        memory: DebugMemoryLimits {
            read: NonZeroU32::new(256).unwrap(),
            write: NonZeroU32::new(256).unwrap(),
        },
        configuration: NonZeroU32::new(256).unwrap(),
    };
    let mut last = 0;
    let started = Instant::now();
    let mut progress_seconds = Vec::new();
    let mut callbacks = flash::Callbacks {
        chunk: &mut |offset, bytes| {
            sink.put(
                offset - if diagnostic.is_some() { 64 << 20 } else { 0 },
                &bytes,
            )
        },
        progress: &mut |done, total| {
            if done == 0 || done == total || done - last >= 1024 * 1024 {
                let elapsed = started.elapsed().as_secs_f64();
                println!("{done}/{total} bytes at {elapsed:.3}s");
                progress_seconds.push(serde_json::json!({"bytes":done,"seconds":elapsed}));
                timings.collect(&trace);
                last = done;
            }
            Ok(())
        },
    };
    let result = if let Some(mode) = diagnostic {
        flash::diagnose_nand(
            &mut session,
            &bulk,
            &flash,
            &artifact.payload,
            &image,
            limits,
            64 << 20,
            size as u32,
            mode,
            &mut callbacks,
        )
        .await
    } else {
        flash::backup(
            &mut session,
            &bulk,
            &flash,
            &artifact.payload,
            &image,
            limits,
            &mut callbacks,
        )
        .await
    };
    let workflow_seconds = started.elapsed().as_secs_f64();
    let closed = if session.state() == SessionState::Ready {
        session
            .close()
            .await
            .map(|_| true)
            .map_err(|e| e.to_string())
    } else {
        Err("Session completion unknown; close withheld".into())
    };
    timings.collect(&trace);
    sink.file.sync_all()?;
    let mut report = serde_json::json!({"complete": false, "firmware_id": flash.firmware_id,
        "firmware_sha256": artifact.payload.firmware_sha256(),
        "size": size, "source_offset": if diagnostic.is_some() { 64u64 << 20 } else { 0 }, "diagnostic": args.get(3), "delivered": sink.delivered,
        "written_ranges": sink.spans.iter().map(|(&offset, &size)|
            serde_json::json!({"offset": offset, "size": size})).collect::<Vec<_>>(),
        "session_closed": closed.as_ref().is_ok_and(|v| *v),
        "session_close_error": closed.as_ref().err(),
        "operation_error": result.as_ref().err().map(ToString::to_string)});
    report["performance"] = timings.report();
    report["performance"]["workflow_seconds"] = workflow_seconds.into();
    report["performance"]["progress"] = progress_seconds.into();
    if let Ok(backup) = &result {
        report["nand_read_stats"] = serde_json::json!(backup.nand_read_stats);
        report["uncorrectable_page_offsets"] = serde_json::json!(backup.uncorrectable_page_offsets);
        report["uncorrectable_granularity_bytes"] =
            serde_json::json!(backup.uncorrectable_granularity_bytes);
        let digest = sink.digest()?;
        if digest != backup.sha256 {
            return Err("Saved flash hash differs from workflow".into());
        }
        report["sha256"] = digest.into();
        report["complete"] = closed.is_ok().into();
    }
    fs::write(
        output.join("result.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    result?;
    closed?;
    fs::rename(output.join("flash.partial"), output.join("flash.bin"))?;
    println!("Saved {}", output.join("result.json").display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn file_sink_accepts_reordered_chunks_and_rejects_overlap_and_holes() {
        let path = std::env::temp_dir().join(format!("flash-sink-{}", std::process::id()));
        let file = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let mut sink = Sink {
            file,
            size: 4,
            spans: BTreeMap::new(),
            delivered: 0,
        };
        sink.put(2, b"cd").unwrap();
        assert!(sink.put(1, b"xx").is_err());
        assert!(sink.put(4, b"x").is_err());
        assert!(sink.digest().is_err());
        sink.put(0, b"ab").unwrap();
        assert_eq!(
            sink.digest().unwrap(),
            format!("{:x}", Sha256::digest(b"abcd"))
        );
        drop(sink);
        fs::remove_file(path).unwrap();
    }
}
