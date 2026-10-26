//! Replay a captured vector GPU input stream without the CPU emulator.

use crate::recording::{Event, MemoryRead, RecordingReader, RegisterWrite, TargetSample};
use crate::{Action, CommandProcessor, Inputs};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, VecDeque};
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

struct ReplayInputs {
    reads: VecDeque<MemoryRead>,
    targets: VecDeque<TargetSample>,
}

impl ReplayInputs {
    fn from_write(write: &mut RegisterWrite) -> Self {
        Self {
            reads: std::mem::take(&mut write.memory_reads).into(),
            targets: std::mem::take(&mut write.target_samples).into(),
        }
    }
}

impl Inputs for ReplayInputs {
    fn has_reader(&self) -> bool {
        true
    }

    fn has_irq(&self) -> bool {
        true
    }

    fn routed(&self) -> bool {
        true
    }

    fn read(
        &mut self,
        cpu: i64,
        address: u64,
        length: u64,
    ) -> std::result::Result<Vec<u8>, String> {
        let input = self
            .reads
            .pop_front()
            .ok_or_else(|| "GPU replay used an unrecorded RAM range".to_owned())?;
        if input.cpu != cpu || input.address != address || input.length != length {
            return Err(format!(
                "GPU replay RAM read differs: expected CPU {} {:#x}+{:#x}, got CPU {cpu} {address:#x}+{length:#x}",
                input.cpu, input.address, input.length
            ));
        }
        if let Ok(bytes) = &input.result
            && bytes.len() as u64 != length
        {
            return Err(format!(
                "GPU replay RAM length differs: expected {}, got {length}",
                bytes.len()
            ));
        }
        input.result
    }

    fn targets(&mut self) -> fujimod_emulator_devices::record::Result<(usize, i64)> {
        match self
            .targets
            .pop_front()
            .ok_or_else(|| "GPU replay used an unrecorded IRQ routing sample".to_owned())?
        {
            TargetSample::Routed { count, selected } => Ok((count, selected)),
            TargetSample::Failed(error) => Err(fujimod_emulator_devices::Unsupported(error)),
        }
    }
}

#[derive(Default)]
struct Summary {
    records: u64,
    accesses: u64,
    submissions: u64,
    outputs: u64,
    matched_outputs: u64,
    operation_time: Duration,
    access_time: Duration,
    digest_time: Duration,
    output_lines: Vec<String>,
}

/// Run the model and rasterizer against one binary recording and print timing
/// and the digest of every DMA output.
pub fn run(path: impl AsRef<Path>) -> Result<(), String> {
    let mut recording = RecordingReader::open(path.as_ref()).map_err(format_io)?;
    let mut state = CommandProcessor {
        ram_bytes: recording.header.ram_bytes,
        max_fill_bytes: recording.header.max_fill_bytes,
        max_copy_bytes: recording.header.max_copy_bytes,
        max_command_capture_bytes: recording.header.max_command_capture_bytes,
        max_clear_bytes: recording.header.max_clear_bytes,
        bytes_per_tick: recording.header.bytes_per_tick,
        copy_profile: recording.header.copy_profile,
        clear_profile: recording.header.clear_profile,
        ..CommandProcessor::default()
    };
    let ram_length = usize::try_from(recording.header.ram_bytes)
        .map_err(|_| "recorded RAM exceeds the replay host address space")?;
    let mut ram = vec![0; ram_length];
    let mut scheduled = BTreeMap::<u64, u64>::new();
    let mut summary = Summary::default();
    let mut last_tick = 0;
    let mut first_event = true;
    let replay_start = Instant::now();

    while let Some(event) = recording.next_event().map_err(format_io)? {
        let tick = match &event {
            Event::RegisterRead(read) => read.now,
            Event::RegisterWrite(write) => write.now,
            Event::Completion(completion) => completion.now,
        };
        if !first_event && tick < last_tick {
            return Err(format!(
                "GPU replay virtual time moved backward from {last_tick} to {tick}"
            ));
        }
        first_event = false;
        last_tick = tick;

        match event {
            Event::RegisterRead(read) => {
                summary.accesses += 1;
                let start = Instant::now();
                let actual = state
                    .read(read.address, read.size.into())
                    .map_err(|error| error.0);
                let elapsed = start.elapsed();
                summary.operation_time += elapsed;
                summary.access_time += elapsed;
                compare_result("register read", &actual, &read.result)?;
            }
            Event::RegisterWrite(mut write) => {
                summary.accesses += 1;
                if is_submission(write.address, write.value) {
                    summary.submissions += 1;
                }
                let mut inputs = ReplayInputs::from_write(&mut write);
                let mut actions = Vec::new();
                let start = Instant::now();
                let actual = if write.value > u64::from(u32::MAX) {
                    Err("vector GPU word exceeds 32 bits".to_owned())
                } else if write.size != 4 {
                    Err("vector GPU requires aligned word accesses".to_owned())
                } else {
                    state
                        .write_native_for_replay(
                            write.cpu,
                            write.address,
                            write.value as u32,
                            &mut inputs,
                            &mut actions,
                        )
                        .map_err(|error| error.0)
                };
                for action in actions {
                    if let Action::Schedule { delay, token } = action {
                        let due = tick
                            .checked_add(delay)
                            .ok_or("GPU replay completion time overflow")?;
                        if delay == 0 || scheduled.insert(token, due).is_some() {
                            return Err("GPU replay received an invalid completion schedule".into());
                        }
                    }
                }
                let elapsed = start.elapsed();
                summary.operation_time += elapsed;
                summary.access_time += elapsed;
                compare_result("register write", &actual, &write.result)?;
                if !inputs.reads.is_empty() || !inputs.targets.is_empty() {
                    return Err("GPU replay did not consume all recorded write inputs".into());
                }
            }
            Event::Completion(completion) => {
                let due = scheduled
                    .get(&completion.token)
                    .copied()
                    .ok_or_else(|| "GPU replay completed an unscheduled token".to_owned())?;
                if completion.now < due {
                    return Err(format!(
                        "GPU replay completed token {} before its deadline",
                        completion.token
                    ));
                }

                let start = Instant::now();
                let (_, output) = state.output(completion.token).map_err(|error| error.0)?;
                summary.operation_time += start.elapsed();
                if output.len() != completion.outputs.len() {
                    return Err(format!(
                        "GPU replay output count differs for token {}: expected {}, got {}",
                        completion.token,
                        completion.outputs.len(),
                        output.len()
                    ));
                }

                for ((address, bytes), expected) in output.iter().zip(&completion.outputs) {
                    if *address != expected.address || bytes.len() as u64 != expected.length {
                        return Err(format!(
                            "GPU replay output location differs for token {}",
                            completion.token
                        ));
                    }
                    let digest_start = Instant::now();
                    let digest: [u8; 32] = Sha256::digest(bytes.as_slice()).into();
                    summary.digest_time += digest_start.elapsed();
                    if digest != expected.sha256 {
                        return Err(format!(
                            "GPU replay output digest differs at {address:#x} on tick {}: expected {}, got {}",
                            completion.now,
                            hex_digest(&expected.sha256),
                            hex_digest(&digest)
                        ));
                    }
                    let write_start = Instant::now();
                    write_output(&mut ram, *address, bytes)?;
                    summary.operation_time += write_start.elapsed();
                    summary.outputs += 1;
                    summary.matched_outputs += 1;
                    summary.output_lines.push(format!(
                        "output tick={} address={address:#x} bytes={} sha256={}",
                        completion.now,
                        bytes.len(),
                        hex_digest(&digest)
                    ));
                }

                let start = Instant::now();
                state
                    .complete_for_replay(completion.token, completion.now)
                    .map_err(|error| error.0)?;
                summary.operation_time += start.elapsed();
                scheduled.remove(&completion.token);
            }
        }
        summary.records += 1;
    }

    let replay_time = replay_start.elapsed();
    for line in summary.output_lines {
        println!("{line}");
    }
    println!("records replayed: {}", summary.records);
    println!("register accesses: {}", summary.accesses);
    println!("GPU submissions: {}", summary.submissions);
    println!("DMA output writes: {}", summary.outputs);
    println!(
        "GPU operation time: {:.6} s",
        summary.operation_time.as_secs_f64()
    );
    println!(
        "time per submission: {:.3} us",
        nanos_per(summary.operation_time, summary.submissions) / 1000.0
    );
    println!(
        "time per register access: {:.3} ns",
        nanos_per(summary.access_time, summary.accesses)
    );
    println!(
        "output digest time: {:.6} s",
        summary.digest_time.as_secs_f64()
    );
    println!("replay wall time: {:.6} s", replay_time.as_secs_f64());
    println!(
        "digest check: matched {} of {} output writes",
        summary.matched_outputs, summary.outputs
    );
    Ok(())
}

fn is_submission(address: u64, value: u64) -> bool {
    if let Ok(offset) = crate::offset(address, 4) {
        value != 0 && (crate::TRIGGERS.contains(&offset) || offset == 0x1c || offset == 0xc18)
    } else {
        false
    }
}

fn write_output(ram: &mut [u8], address: u64, bytes: &[u8]) -> Result<(), String> {
    let start = usize::try_from(address).map_err(|_| "GPU output address exceeds host memory")?;
    let end = start
        .checked_add(bytes.len())
        .ok_or("GPU output range overflow")?;
    let target = ram.get_mut(start..end).ok_or_else(|| {
        format!(
            "GPU output range {address:#x}+{:#x} exceeds replay RAM",
            bytes.len()
        )
    })?;
    target.copy_from_slice(bytes);
    Ok(())
}

fn compare_result<T: PartialEq>(
    operation: &str,
    actual: &std::result::Result<T, String>,
    expected: &std::result::Result<T, String>,
) -> Result<(), String> {
    match (actual, expected) {
        (Ok(actual), Ok(expected)) if actual == expected => Ok(()),
        (Err(actual), Err(expected)) if actual == expected => Ok(()),
        (Ok(_), Err(expected)) => Err(format!(
            "GPU replay {operation} unexpectedly succeeded; recording refused it: {expected}"
        )),
        (Err(actual), Ok(_)) => Err(format!("GPU replay {operation} was refused: {actual}")),
        (Err(actual), Err(expected)) => Err(format!(
            "GPU replay {operation} refusal differs: expected {expected}, got {actual}"
        )),
        (Ok(_), Ok(_)) => Err(format!("GPU replay {operation} value differs")),
    }
}

fn nanos_per(duration: Duration, count: u64) -> f64 {
    if count == 0 {
        0.0
    } else {
        duration.as_nanos() as f64 / count as f64
    }
}

fn hex_digest(digest: &[u8; 32]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn format_io(error: io::Error) -> String {
    format!("vector GPU recording error: {error}")
}
