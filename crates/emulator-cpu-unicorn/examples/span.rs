//! Measure a caller-supplied CPU span, checking bounded/unbounded end states.
use fujimod_emulator_cpu::{
    Cpu, Hook, HookRange, InstructionDisposition, Model, Register, Run, Translation,
    TranslationHooks,
};
use fujimod_emulator_cpu_unicorn::Library;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{ffi::c_void, time::Instant};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    model: Model,
    memory: Vec<(u64, usize)>,
    images: Vec<(u64, String, String)>,
    registers: Vec<(Register, u64)>,
    begin: u64,
    until: u64,
    repeats: usize,
}
unsafe fn count(_: u64, _: u32, data: *mut c_void) {
    unsafe { *data.cast::<usize>() += 1 };
}
// Installing an observing filter selects the original per-instruction count
// hook. It observes every instruction and changes no guest state, providing a
// contemporaneous reference while retaining the same native executable.
unsafe fn legacy(_: *mut c_void, _: u64, _: u32) -> Translation {
    Translation {
        instruction: InstructionDisposition::Observe,
        observe_block: false,
    }
}
unsafe fn restart(_: *mut c_void, _: u64) {}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("expected input JSON path")?;
    let input: Input = serde_json::from_slice(&std::fs::read(path)?)?;
    if input.model != Model::ArmV8A || input.repeats == 0 {
        return Err("expected armv8a and at least one repeat".into());
    }
    let library = Library::linked()?;
    let images = input
        .images
        .iter()
        .map(|(base, path, hash)| {
            let data = std::fs::read(path).unwrap();
            assert_eq!(format!("{:x}", Sha256::digest(&data)), *hash);
            (*base, data)
        })
        .collect::<Vec<_>>();
    let mut instructions = 0usize;
    let mut expected = None;
    let modes = ["calibrate", "unbounded", "legacy-budgeted", "budgeted"];
    let mut samples: [Vec<f64>; 4] = std::array::from_fn(|_| Vec::new());
    for round in 0..=input.repeats {
        let order = if round == 0 {
            vec![0]
        } else {
            (0..3).map(|n| 1 + (round + n) % 3).collect()
        };
        for index in order {
            let mode = modes[index];
            let cpu = library.open(input.model, 16 << 20)?;
            for &(base, size) in &input.memory {
                cpu.map_zeroed(base, size, 7)?;
            }
            for (base, data) in &images {
                cpu.write_memory(*base, data)?;
            }
            for &(register, value) in &input.registers {
                let bytes = value.to_le_bytes();
                cpu.write_register(register, &bytes)?;
            }
            if mode == "calibrate" {
                unsafe {
                    cpu.install_hook(Hook::Instruction {
                        range: HookRange::All,
                        callback: count,
                        data: (&mut instructions as *mut usize).cast(),
                    })?;
                }
            }
            if mode == "legacy-budgeted" {
                unsafe {
                    cpu.translation_hooks(Some(TranslationHooks {
                        classify: legacy,
                        restart,
                        data: std::ptr::null_mut(),
                    }))?;
                }
            }
            let start = Instant::now();
            cpu.run(Run {
                begin: input.begin,
                until: Some(input.until),
                instructions: mode.ends_with("budgeted").then_some(instructions),
                watchdog_us: 1_000_000,
            })
            .map_err(|e| {
                let mut pc = [0; 8];
                let _ = cpu.read_register(Register::Pc, &mut pc);
                format!("{e:?} at {:#x}", u64::from_le_bytes(pc))
            })?;
            samples[index].push(start.elapsed().as_secs_f64());
            let mut digest = Sha256::new();
            for n in 0..31 {
                let mut bytes = [0; 8];
                cpu.read_register(Register::X(n), &mut bytes)?;
                digest.update(bytes);
            }
            for register in [Register::Pc, Register::Sp, Register::Pstate] {
                let mut bytes = [0; 8];
                cpu.read_register(register, &mut bytes)?;
                if register == Register::Pc {
                    assert_eq!(u64::from_le_bytes(bytes), input.until);
                }
                digest.update(bytes);
            }
            for &(base, size) in &input.memory {
                let mut data = vec![0; size];
                cpu.read_memory(base, &mut data)?;
                digest.update(data);
            }
            let digest = format!("{:x}", digest.finalize());
            if let Some(expected) = &expected {
                assert_eq!(&digest, expected);
            } else {
                expected = Some(digest);
            }
        }
    }
    for (mode, mut samples) in modes.into_iter().zip(samples) {
        samples.sort_by(f64::total_cmp);
        let seconds = samples[samples.len() / 2];
        println!(
            "{}",
            serde_json::json!({"mode":mode,"instructions":instructions,
            "median_seconds":seconds,"instructions_per_second":instructions as f64/seconds,
            "samples":samples,"state_sha256":expected})
        );
    }
    Ok(())
}
