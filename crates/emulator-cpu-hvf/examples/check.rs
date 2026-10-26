//! Signed checks using source-authored CPU programs and declared peripherals.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "check/durable.rs"]
mod durable;
#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
fn main() {
    panic!("hvf-check requires macOS ARM64");
}
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn main() -> Result<(), String> {
    use fujimod_emulator_cpu::*;
    use fujimod_emulator_cpu_hvf::Vm;
    use std::{
        ffi::c_void,
        thread::{self, ThreadId},
    };
    fn set(cpu: &dyn Cpu, r: Register, v: u64) {
        cpu.write_register(r, &v.to_le_bytes()).unwrap();
    }
    fn get(cpu: &dyn Cpu, r: Register) -> u64 {
        let mut b = [0; 8];
        cpu.read_register(r, &mut b).unwrap();
        u64::from_le_bytes(b)
    }
    fn code(cpu: &dyn Cpu, at: u64, words: &[u32]) {
        cpu.write_memory(
            at,
            &words
                .iter()
                .flat_map(|w| w.to_le_bytes())
                .collect::<Vec<_>>(),
        )
        .unwrap();
    }
    fn run(cpu: &dyn Cpu, pc: u64, n: usize) -> Result<Exit, ExecutionError> {
        cpu.run(Run {
            begin: pc,
            until: None,
            watchdog_us: 2_000_000,
            instructions: Some(n),
        })
    }
    fn architectural_board_checks() -> Result<(), String> {
        use fujimod_emulator::board::{
            InterruptMap, aprofile,
            bus::{Bus, Mode},
            factory::*,
        };
        use fujimod_emulator::board::aprofile::generic_timer::Clock;
        use fujimod_emulator_ip_arm::gicv3::Gic;
        let graph = |processors: Vec<Processor>| Graph {
            decoders: vec![],
            mappings: (0..processors.len())
                .map(|core| MemoryMap {
                    core,
                    address: 0x10000,
                    physical: 0x10000,
                    size: 0x14000,
                    permissions: 7,
                })
                .collect(),
            processors,
            dma_mappings: vec![fujimod_emulator::board::memory::BusMapping::ram(0x10000, 0x14000)],
            ram: vec![Ram {
                base: 0x10000,
                size: 0x14000,
            }],
            peripherals: vec![],
            instructions_per_tick: fujimod_emulator_cpu::timeline::Ratio::new(1, 1).unwrap(),
        };
        let options = || {
            Options::new(Engine::Hypervisor).with_translation_cache_size_bytes(1024 * 1024)
        };
        // Both architectural compare channels must assert their declared PPI at
        // guest tick100, including when the host pauses while WFI is waiting.
        for virtual_timer in [false, true] {
            for quantum in [1, 100] {
                for split in [false, true] {
                    let line = if virtual_timer { 27 } else { 30 };
                    let mut gic = Gic::new(vec![0], 512)?;
                    gic.distributor(0, 4, Some(7))?;
                    gic.redistributor(0, 0x14, 4, Some(0))?;
                    gic.redistributor(0, 0x10080, 4, Some(1 << line))?;
                    gic.redistributor(0, 0x10100, 4, Some(1 << line))?;
                    gic.redistributor(0, 0x10400 + line, 1, Some(0x40))?;
                    gic.system(0, 0xc230, Some(255))?;
                    gic.system(0, 0xc667, Some(1))?;
                    let mut board = graph(vec![Processor {
                        model: Model::ArmV8A,
                        released: true,
                        rprofile: None,
                        mprofile: None,
                        registers: vec![
                            (Register::Pstate, 0x345),
                            (Register::Pc, 0x10000),
                            (Register::X(0), 100),
                            (Register::X(1), 1),
                            (sys(0xc600), 0x20000),
                        ],
                    }])
                    .build(
                        options(),
                        Bus::new(Mode::Strict),
                        Some(InterruptMap {
                            gic: Box::new(gic),
                            registers: fujimod_emulator::board::InterruptRegisters::GicV3 {
                                distributor: 0xfa000000,
                                redistributor: 0xfa080000,
                                stride: 0x20000,
                            },
                            cores: 1,
                        }),
                    )?;
                    board.configure_aprofile(
                        0,
                        aprofile::Config {
                            clock: Clock {
                                counter_hz: 100,
                                ticks_per_second: 100,
                            },
                            physical_irq: 30,
                            virtual_irq: 27,
                            pmu_hz: 100,
                            implementation: None,
                        },
                    )?;
                    code(
                        board.cpu(0)?,
                        0x10000,
                        if virtual_timer {
                            &[0xd51be340, 0xd51be321, 0xd503207f, 0xd53be042, 0xd503207f]
                        } else {
                            &[0xd51be240, 0xd51be221, 0xd503207f, 0xd53be022, 0xd503207f]
                        },
                    );
                    code(
                        board.cpu(0)?,
                        0x20280,
                        if virtual_timer {
                            &[0xd538cc0d, 0xd51be33f, 0x91000463, 0xd518cc2d, 0xd69f03e0]
                        } else {
                            &[0xd538cc0d, 0xd51be23f, 0x91000463, 0xd518cc2d, 0xd69f03e0]
                        },
                    );
                    if split {
                        board.run_until(50, quantum)?;
                        assert_eq!(get(board.cpu(0)?, Register::X(3)), 0);
                        assert_eq!(board.state()["waiting"], serde_json::json!([true]));
                    }
                    board.run_until(150, quantum)?;
                    assert_eq!(get(board.cpu(0)?, Register::X(3)), 1);
                    assert_eq!(get(board.cpu(0)?, Register::X(13)), line);
                    assert_eq!(get(board.cpu(0)?, Register::X(2)), 106);
                    assert_eq!(board.state()["instructions"], serde_json::json!([10]));
                    assert_eq!(board.state()["timeline"]["now"], 150);
                    assert_eq!(board.state()["waiting"], serde_json::json!([true]));
                    assert_eq!(board.state()["unmodeled_system"], serde_json::json!([]));
                    let saved = board.state();
                    assert_eq!(board.state(), saved);
                }
            }
        }
        // SEVL is consumed by the first WFE; SEV completes the second WFE.
        // The same peer event must leave a WFI-only processor waiting.
        for quantum in [1, 64] {
            for wfe in [false, true] {
                let mut board = graph(
                    (0..2)
                        .map(|core| Processor {
                            model: Model::ArmV8A,
                            released: true,
                            rprofile: None,
                            mprofile: None,
                            registers: vec![
                                (Register::Pstate, 0x3c5),
                                (Register::Pc, 0x10000 + core * 0x1000),
                            ],
                        })
                        .collect(),
                )
                .build(options(), Bus::new(Mode::Strict), None)?;
                code(
                    board.cpu(0)?,
                    0x10000,
                    if wfe {
                        &[0xd50320bf, 0xd503205f, 0xd503205f, 0xd28009a0, 0xd503207f]
                    } else {
                        &[0xd503207f, 0xd28009a0, 0xd503207f]
                    },
                );
                let mut peer = vec![0xd503201f; 128];
                peer.extend([0xd503209f, 0xd503207f]);
                code(board.cpu(1)?, 0x11000, &peer);
                board.run_until(500, quantum)?;
                assert_eq!(get(board.cpu(0)?, Register::X(0)), if wfe { 77 } else { 0 });
                assert_eq!(board.state()["waiting"], serde_json::json!([true, true]));
                assert_eq!(board.state()["instructions"][0], if wfe { 5 } else { 1 });
                assert_eq!(board.state()["instructions"][1], 130);
            }
        }
        println!(
            "PASS: architectural physical/virtual timers, exact guest PPI deadline, WFI pause/resume, SEVL/WFE/SEV and WFI event isolation"
        );
        Ok(())
    }

    let vm = Vm::new()?;
    assert!(Vm::new().is_err());
    let cpu = vm.cpu(Model::ArmV8A)?;
    let peer = vm.cpu(Model::ArmV8A)?;
    cpu.map_zeroed(0x10000, 0x4000, 7)?;
    cpu.map_zeroed(0x20000, 0x4000, 7)?;
    assert_eq!(cpu.memory_regions()?, peer.memory_regions()?);
    code(&cpu, 0x10000, &[0x91000400, 0x91000421, 0x17fffffe]);
    assert!(matches!(run(&cpu, 0x10000, 10_000).unwrap(), Exit::Stopped));
    assert_eq!(
        (get(&cpu, Register::X(0)), get(&cpu, Register::X(1))),
        (3334, 3333)
    );
    assert_eq!(cpu.instruction_progress(), Some(10_000));
    let pc = get(&cpu, Register::Pc);
    run(&cpu, 0xdead0000, 0).unwrap();
    assert_eq!(get(&cpu, Register::Pc), pc);
    assert_eq!(cpu.instruction_progress(), Some(0));
    assert!(matches!(
        cpu.run(Run {
            begin: 0x10000,
            until: Some(0x10004),
            watchdog_us: 2_000_000,
            instructions: Some(100)
        })
        .unwrap(),
        Exit::Stopped
    ));
    assert_eq!(cpu.instruction_progress(), Some(1));
    // Snapshot banked stack, FP/SIMD and system state, plus owner refusal.
    set(&cpu, Register::Sp, 0x135790);
    set(&cpu, Register::Fpcr, 0x400000);
    cpu.write_register(Register::V(31), &[0x5a; 16])?;
    let snapshot = cpu.snapshot(None)?;
    assert!(peer.restore(snapshot.as_ref()).is_err());
    set(&cpu, Register::Sp, 0x246800);
    set(&cpu, Register::X(0), 0);
    cpu.write_register(Register::V(31), &[0; 16])?;
    cpu.restore(snapshot.as_ref())?;
    assert_eq!(get(&cpu, Register::Sp), 0x135790);
    assert_eq!(get(&cpu, Register::Fpcr), 0x400000);
    let mut v = [0; 16];
    cpu.read_register(Register::V(31), &mut v)?;
    assert_eq!(v, [0x5a; 16]);
    // Reentrant observations run on the owner thread and can redirect/stop.
    struct Observe<'a> {
        cpu: &'a dyn Cpu,
        owner: ThreadId,
        pcs: Vec<u64>,
    }
    unsafe fn before(pc: u64, _: u32, data: *mut c_void) {
        let o = unsafe { &mut *data.cast::<Observe<'_>>() };
        assert_eq!(thread::current().id(), o.owner);
        assert_eq!(get(o.cpu, Register::Pc), pc);
        o.pcs.push(pc);
        if pc == 0x10000 {
            set(o.cpu, Register::Pc, 0x10004);
        } else {
            assert_eq!(o.cpu.instruction_progress(), Some(0));
            o.cpu.stop().unwrap();
        }
    }
    let mut o = Observe {
        cpu: &cpu,
        owner: thread::current().id(),
        pcs: vec![],
    };
    let hook = unsafe {
        cpu.install_hook(Hook::Instruction {
            callback: before,
            data: (&mut o as *mut Observe<'_>).cast(),
            range: HookRange::All,
        })?
    };
    assert!(peer.remove_hook(hook).is_err());
    run(&cpu, 0x10000, 100).unwrap();
    cpu.remove_hook(hook)?;
    assert_eq!(o.pcs, vec![0x10000, 0x10004]);
    assert_eq!(cpu.instruction_progress(), Some(0));
    // SVC -> EL1 vector -> ERET, with no injected SS bit saved in SPSR.
    fn sys(r: u16) -> Register {
        Register::Arm64System(arm::SystemRegister::from_encoding(r))
    }
    code(&cpu, 0x10100, &[0xd4000001, 0x91000400, 0x17fffffe]);
    code(&cpu, 0x20400, &[0x91000442, 0xd69f03e0]);
    set(&cpu, sys(0xc600), 0x20000);
    set(&cpu, Register::Pstate, 0);
    set(&cpu, Register::X(2), 0);
    run(&cpu, 0x10100, 1).unwrap();
    assert_eq!(get(&cpu, Register::Pc), 0x20400);
    assert_eq!(get(&cpu, sys(0xc200)), 0);
    run(&cpu, 0x20400, 2).unwrap();
    assert_eq!(get(&cpu, Register::Pc), 0x10104);
    assert_eq!(get(&cpu, Register::X(2)), 1);
    assert_eq!(get(&cpu, Register::Pstate), 0);
    // IRQ vectors before the first observed instruction. The saved return PC
    // is the interrupted PC; no instruction budget is consumed by delivery.
    code(&cpu, 0x20480, &[0x91000463, 0xd69f03e0]);
    set(&cpu, Register::X(3), 0);
    set(&cpu, Register::Pc, 0x10104);
    cpu.set_interrupt(Interrupt::Irq, true)?;
    run(&cpu, 0x10104, 1).unwrap();
    assert_eq!(get(&cpu, Register::Pc), 0x20484);
    assert_eq!(get(&cpu, sys(0xc201)), 0x10104);
    assert_eq!(get(&cpu, Register::X(3)), 1);
    assert_eq!(cpu.instruction_progress(), Some(1));
    cpu.set_interrupt(Interrupt::Irq, false)?;
    run(&cpu, 0x20484, 1).unwrap();
    assert_eq!(get(&cpu, Register::Pc), 0x10104);
    set(&cpu, Register::Pstate, 0x3c5);
    // A framework-owned GIC injects the architectural pin before observation.
    // ICC accesses trap to that same model; no in-kernel GIC runs asynchronously.
    for group in [1, 0] {
        use fujimod_emulator_ip_arm::gicv3::Gic;
        let mut gic = Gic::new(vec![0], 512)?;
        gic.distributor(0, 4, Some(7))?;
        gic.redistributor(0, 0x14, 4, Some(0))?;
        gic.distributor(0x84, 4, Some(group))?;
        gic.distributor(0x104, 4, Some(1))?;
        gic.distributor(0x420, 1, Some(0x40))?;
        gic.system(0, 0xc230, Some(255))?;
        gic.system(0, 0xc666 + group as u16, Some(1))?;
        gic.set_spi(32, true)?;
        assert_eq!(gic.pins(0)?, (group == 1, group == 0));
        let vector = if group == 1 { 0x20280 } else { 0x20300 };
        let pstate = 0x800 | if group == 1 { 0x345 } else { 0x385 };
        let pin = if group == 1 {
            Interrupt::Irq
        } else {
            Interrupt::Fiq
        };
        let iar = if group == 1 { 0xc660 } else { 0xc640 };
        let eoir = iar + 1;
        code(
            &cpu,
            vector,
            &[
                0xd5200000 | ((iar as u32) << 5) | 13,
                0xd5000000 | ((eoir as u32) << 5) | 13,
                0xd69f03e0,
            ],
        );
        let mut entries = Vec::<u64>::new();
        unsafe fn record(pc: u64, _: u32, data: *mut c_void) {
            unsafe { &mut *data.cast::<Vec<u64>>() }.push(pc);
        }
        let hook = unsafe {
            cpu.install_hook(Hook::Instruction {
                callback: record,
                data: (&mut entries as *mut Vec<u64>).cast(),
                range: HookRange::All,
            })?
        };
        set(&cpu, Register::Pstate, pstate);
        let x0 = get(&cpu, Register::X(0));
        cpu.set_interrupt(pin, true)?;
        match run(&cpu, 0x10200, 1).unwrap() {
            Exit::Trap(t) => assert_eq!(t.syndrome >> 26, 0x18),
            other => panic!("GIC acknowledge did not trap: {other:?}"),
        }
        assert_eq!(entries, vec![vector]);
        assert_eq!(cpu.instruction_progress(), Some(1));
        assert_eq!(get(&cpu, sys(0xc201)), 0x10200);
        assert_eq!(get(&cpu, sys(0xc200)), pstate);
        assert_eq!(get(&cpu, Register::X(0)), x0);
        assert!(cpu.read_register(sys(iar), &mut [0; 8]).is_err());
        let id = gic.system(0, iar, None)?;
        assert_eq!(id, 32);
        set(&cpu, Register::X(13), id);
        cpu.set_interrupt(pin, false)?;
        match run(&cpu, vector + 4, 1).unwrap() {
            Exit::Trap(t) => assert_eq!((t.syndrome >> 26, t.syndrome & 1), (0x18, 0)),
            other => panic!("GIC EOI did not trap: {other:?}"),
        }
        gic.set_spi(32, false)?;
        gic.system(0, eoir, Some(get(&cpu, Register::X(13))))?;
        run(&cpu, vector + 8, 1).unwrap();
        cpu.remove_hook(hook)?;
        assert_eq!(entries, vec![vector, vector + 4, vector + 8]);
        assert_eq!(get(&cpu, Register::Pc), 0x10200);
        assert_eq!(get(&cpu, Register::Pstate), pstate);
        assert_eq!(gic.pins(0)?, (false, false));
        set(&cpu, Register::Pstate, 0x3c5);
    }
    // Counter, PMU, AMU and RNG reads must exit instead of observing host
    // clocks, host performance counters or host randomness.
    for word in [0xd53be040, 0xd53b9d00, 0xd53bd400, 0xd53b2400] {
        code(&cpu, 0x10200, &[word]);
        let previous = get(&cpu, Register::X(0));
        match run(&cpu, 0x10200, 1).unwrap() {
            Exit::Trap(t) => {
                assert_eq!(t.syndrome >> 26, 0x18);
                assert_eq!(t.syndrome & 1, 1);
            }
            e => panic!("host input did not trap: {e:?}"),
        }
        assert_eq!(get(&cpu, Register::Pc), 0x10200);
        assert_eq!(get(&cpu, Register::X(0)), previous);
        assert_eq!(cpu.instruction_progress(), Some(1));
    }
    // MMIO callbacks are guest-only, typed, on the owner thread and reentrant.
    struct Io<'a> {
        cpu: &'a dyn Cpu,
        owner: ThreadId,
        reads: Vec<(u64, u32)>,
        writes: Vec<(u64, u32, u64)>,
        stop_reads: bool,
    }
    unsafe fn read(offset: u64, size: u32, data: *mut c_void) -> u64 {
        let io = unsafe { &mut *data.cast::<Io<'_>>() };
        assert_eq!(thread::current().id(), io.owner);
        assert!([0x10300, 0x10320, 0x10340, 0x10344].contains(&get(io.cpu, Register::Pc)));
        io.reads.push((offset, size));
        if io.stop_reads {
            io.cpu.stop().unwrap();
        }
        0x1234567800000080 | offset
    }
    unsafe fn write(offset: u64, size: u32, value: u64, data: *mut c_void) {
        let io = unsafe { &mut *data.cast::<Io<'_>>() };
        io.writes.push((offset, size, value));
    }
    let mut io = Io {
        cpu: &cpu,
        owner: thread::current().id(),
        reads: vec![],
        writes: vec![],
        stop_reads: false,
    };
    unsafe {
        cpu.map_mmio(
            0x8000000,
            0xc000,
            Mmio {
                read,
                write,
                data: (&mut io as *mut Io<'_>).cast(),
            },
        )?;
    }
    assert!(peer.map_zeroed(0x8000000, 0x4000, 3).is_err());
    code(&cpu, 0x10300, &[0xf9400020, 0xf9000420]);
    set(&cpu, Register::X(1), 0x8000000);
    run(&cpu, 0x10300, 2).unwrap();
    assert_eq!(io.reads, vec![(0, 8)]);
    assert_eq!(io.writes, vec![(8, 8, 0x1234567800000080)]);
    // Signed loads, post-index writeback and pairs use the shared decoder.
    code(&cpu, 0x10320, &[0x39800020]);
    run(&cpu, 0x10320, 1).unwrap();
    assert_eq!(get(&cpu, Register::X(0)), 0xffffffffffffff80);
    code(&cpu, 0x10340, &[0xb8404420, 0xa9401023]);
    run(&cpu, 0x10340, 1).unwrap();
    assert_eq!(get(&cpu, Register::X(1)), 0x8000004);
    set(&cpu, Register::X(1), 0x8000000);
    run(&cpu, 0x10344, 1).unwrap();
    assert_eq!(get(&cpu, Register::X(3)), 0x1234567800000080);
    assert_eq!(get(&cpu, Register::X(4)), 0x1234567800000088);
    assert_eq!(&io.reads[1..], &[(0, 1), (0, 4), (0, 8), (8, 8)]);
    // A strict device refusal stops the access before register/PC commitment.
    io.stop_reads = true;
    set(&cpu, Register::X(0), 0xdeadbeef);
    set(&cpu, Register::X(1), 0x8000000);
    run(&cpu, 0x10300, 2).unwrap();
    assert_eq!(get(&cpu, Register::X(0)), 0xdeadbeef);
    assert_eq!(get(&cpu, Register::Pc), 0x10300);
    assert_eq!(cpu.instruction_progress(), Some(1));
    io.stop_reads = false;
    let before_reads = io.reads.len();
    let mut bytes = [0xa5; 16];
    assert!(cpu.read_memory(0x8000000, &mut bytes).is_err());
    assert_eq!(bytes, [0xa5; 16]);
    assert!(
        cpu.read_virtual(0x8000000, Access::Read, &mut bytes)
            .is_err()
    );
    assert_eq!(io.reads.len(), before_reads);
    cpu.protect(0x8004000, 0x4000, 1)?;
    cpu.unmap(0x8000000, 0x4000)?;
    set(&cpu, Register::X(1), 0x8004040);
    run(&cpu, 0x10300, 1).unwrap();
    assert_eq!(*io.reads.last().unwrap(), (0x4040, 8));
    assert_eq!(run(&cpu, 0x10304, 1).unwrap_err().kind(), "write-protected");
    assert_eq!(io.writes.len(), 1);
    cpu.unmap(0x8004000, 0x8000)?;
    // Device windows need no native host-page backing or alignment.
    unsafe {
        cpu.map_mmio(
            0x8800004,
            0x1000,
            Mmio {
                read,
                write,
                data: (&mut io as *mut Io<'_>).cast(),
            },
        )?;
    }
    cpu.protect(0x8800004, 4, 1)?;
    cpu.unmap(0x8800004, 0x1000)?;
    // Host virtual inspection follows the same EL1 page tables as native fetch.
    cpu.map_zeroed(0x30000, 0x4000, 3)?;
    for (at, value) in [
        (0x30000u64, 0x31003u64),
        (0x31000, 0x32003),
        (0x32080, 0x10403),
        (0x32090, 0x12403),
        (0x32100, 0x20403),
        (0x32200, 0x10403),
        (0x32208, 0x20483 | (1 << 53)),
    ] {
        cpu.write_memory(at, &value.to_le_bytes())?;
    }
    cpu.write_memory(0x10ffc, &[1, 2, 3, 4])?;
    cpu.write_memory(0x20000, &[5, 6, 7, 8])?;
    set(&cpu, sys(0xc100), 0x30000);
    set(&cpu, sys(0xc102), 32);
    set(&cpu, sys(0xc510), 0xff);
    set(&cpu, sys(0xc080), 0x30d00801);
    assert_eq!(cpu.translate(0x40010, Access::Read)?, 0x10010);
    assert_eq!(cpu.translate(0x41010, Access::Read)?, 0x20010);
    assert!(cpu.translate(0x41010, Access::Write).is_err());
    assert!(cpu.translate(0x41010, Access::Fetch).is_err());
    let mut crossing = [0; 8];
    cpu.read_virtual(0x40ffc, Access::Read, &mut crossing)?;
    assert_eq!(crossing, [1, 2, 3, 4, 5, 6, 7, 8]);
    let checkpoint = cpu.snapshot(None)?;
    let x0 = get(&cpu, Register::X(0));
    run(&cpu, 0x10000, 13).unwrap();
    assert_eq!(get(&cpu, Register::X(0)), x0 + 5);
    cpu.restore(checkpoint.as_ref())?;
    assert_eq!(cpu.translate(0x40010, Access::Read)?, 0x10010);
    // Normal memory: exclusive reservations must survive the debug exits used for counted
    // execution, and a peer store must make the subsequent STXR fail.
    for (load, store) in [(0x885f7c20, 0x88027c20), (0x885ffc20, 0x8802fc20)] {
        code(&cpu, 0x10800, &[load, 0x11000400, store]);
        cpu.write_memory(0x12000, &41u32.to_le_bytes())?;
        set(&cpu, Register::X(1), 0x12000);
        run(&cpu, 0x10800, 1).unwrap();
        assert_eq!(
            get(&cpu, Register::Pc),
            0x10804,
            "LDXR exception: ESR={:#x} FAR={:#x}",
            get(
                &cpu,
                Register::Arm64System(arm::SystemRegister::from_encoding(0xc290))
            ),
            get(
                &cpu,
                Register::Arm64System(arm::SystemRegister::from_encoding(0xc300))
            )
        );
        run(&cpu, 0x10804, 2).unwrap();
        assert_eq!(
            get(&cpu, Register::X(2)),
            0,
            "exclusive store failed after counted exits"
        );
        let mut word = [0; 4];
        cpu.read_memory(0x12000, &mut word)?;
        assert_eq!(u32::from_le_bytes(word), 42);
        run(&cpu, 0x10800, 1).unwrap();
        code(&peer, 0x10820, &[0xb9000020]);
        set(&peer, Register::X(1), 0x12000);
        set(&peer, Register::X(0), 99);
        run(&peer, 0x10820, 1).unwrap();
        run(&cpu, 0x10808, 1).unwrap();
        assert_eq!(
            get(&cpu, Register::X(2)),
            1,
            "peer store did not invalidate reservation"
        );
        cpu.read_memory(0x12000, &mut word)?;
        assert_eq!(u32::from_le_bytes(word), 99);
    }
    // All scalar widths and both pair widths, with acquire/release variants.
    // Check CPU-only restore before and after a conflicting host/peer write.
    for (load, store, bytes, pair) in [
        (0x085f7c20u32, 0x08027c20u32, 1usize, false),
        (0x485ffc20, 0x4802fc20, 2, false),
        (0xc85ffc20, 0xc802fc20, 8, false),
        (0x887f0c20, 0x88220c20, 4, true),
        (0xc87f8c20, 0xc8228c20, 8, true),
    ] {
        code(&cpu, 0x10840, &[load, store, 0xd5033f5f, store]);
        let n = bytes * if pair { 2 } else { 1 };
        let initial: Vec<u8> = (1..=n as u8).collect();
        cpu.write_memory(0x12000, &initial)?;
        set(&cpu, Register::X(1), 0x12000);
        run(&cpu, 0x10840, 1).unwrap();
        let snapshot = cpu.snapshot(None)?;
        let mut first = [0; 8];
        first[..bytes].copy_from_slice(&initial[..bytes]);
        assert_eq!(get(&cpu, Register::X(0)), u64::from_le_bytes(first));
        if pair {
            let mut second = [0; 8];
            second[..bytes].copy_from_slice(&initial[bytes..]);
            assert_eq!(get(&cpu, Register::X(3)), u64::from_le_bytes(second));
        }
        run(&cpu, 0x10848, 1).unwrap(); // CLREX
        cpu.restore(snapshot.as_ref())?;
        let mut expected = initial.clone();
        expected[0] ^= 0x80;
        first[..bytes].copy_from_slice(&expected[..bytes]);
        set(&cpu, Register::X(0), u64::from_le_bytes(first));
        if pair {
            expected[bytes] ^= 0x40;
            let mut second = [0; 8];
            second[..bytes].copy_from_slice(&expected[bytes..]);
            set(&cpu, Register::X(3), u64::from_le_bytes(second));
        }
        run(&cpu, 0x10844, 1).unwrap();
        assert_eq!(get(&cpu, Register::X(2)), 0);
        run(&cpu, 0x10844, 1).unwrap(); // SC consumes the reservation
        assert_eq!(get(&cpu, Register::X(2)), 1);
        let mut actual = vec![0; n];
        cpu.read_memory(0x12000, &mut actual)?;
        assert_eq!(actual, expected);
        run(&cpu, 0x10840, 1).unwrap();
        let snapshot = cpu.snapshot(None)?;
        cpu.write_memory(0x12000, &expected)?; // ABA, same bytes
        cpu.restore(snapshot.as_ref())?;
        run(&cpu, 0x10844, 1).unwrap();
        assert_eq!(get(&cpu, Register::X(2)), 1);
        run(&cpu, 0x10840, 1).unwrap();
        run(&cpu, 0x10848, 2).unwrap(); // CLREX then SC
        assert_eq!(get(&cpu, Register::X(2)), 1);
        run(&cpu, 0x10840, 1).unwrap();
        peer.memory_written(0x12000, n)?; // peer backend/DMA notification
        run(&cpu, 0x10844, 1).unwrap();
        assert_eq!(get(&cpu, Register::X(2)), 1);
    }
    // An architectural exception return clears a pending local reservation.
    code(&cpu, 0x10880, &[0x885f7c20, 0xd69f03e0, 0x88027c20]);
    set(&cpu, Register::X(1), 0x12000);
    set(&cpu, sys(0xc201), 0x10888);
    set(&cpu, sys(0xc200), 0x3c5);
    run(&cpu, 0x10880, 3).unwrap();
    assert_eq!(get(&cpu, Register::X(2)), 1);
    set(&peer, Register::X(0), 0);
    set(&cpu, sys(0xc080), 0x30d00800);
    // Host code replacement becomes visible on the next native fetch.
    code(&cpu, 0x10500, &[0x91000400]);
    let x0 = get(&cpu, Register::X(0));
    run(&cpu, 0x10500, 1).unwrap();
    assert_eq!(get(&cpu, Register::X(0)), x0 + 1);
    code(&cpu, 0x10500, &[0x91000800]);
    cpu.invalidate_code(Some((0x10500, 0x10504)))?;
    run(&cpu, 0x10500, 1).unwrap();
    assert_eq!(get(&cpu, Register::X(0)), x0 + 3);
    code(&cpu, 0x10600, &[0xd503207f]); // WFI reports a completed architectural hint.
    match run(&cpu, 0x10600, 1).unwrap() {
        Exit::Hint(fujimod_emulator_cpu::Hint::WaitForInterrupt) => {
            assert_eq!(get(&cpu, Register::Pc), 0x10604);
            assert_eq!(cpu.instruction_progress(), Some(1));
        }
        other => panic!("WFI failed to exit: {other:?}"),
    }
    // Host cancellation changes no guest clock. A subsequent bounded run
    // must not inherit a canceled-entry latch from the earlier watchdog.
    code(&cpu, 0x10400, &[0x14000000]);
    match cpu.run(Run {
        begin: 0x10400,
        until: None,
        watchdog_us: 1000,
        instructions: None,
    }) {
        Ok(Exit::Stopped) => assert!(cpu.instruction_progress().unwrap() > 0),
        Err(ExecutionError::Backend(e)) if e.contains("entry count is unavailable") => {
            assert_eq!(cpu.instruction_progress(), None)
        }
        other => panic!("unexpected cancellation: {other:?}"),
    }
    run(&cpu, 0x10000, 7).unwrap();
    assert_eq!(cpu.instruction_progress(), Some(7));
    assert_eq!(
        run(&cpu, 0xdead0000, 1).unwrap_err().kind(),
        "fetch-unmapped"
    );
    cpu.protect(0x10000, 0x4000, 3)?;
    assert_eq!(run(&cpu, 0x10000, 1).unwrap_err().kind(), "fetch-protected");
    assert_eq!(cpu.instruction_progress(), Some(0));
    cpu.protect(0x10000, 0x4000, 7)?;
    assert_eq!(cpu.instruction_progress(), Some(0));
    // Shared physical maps survive dropping the processor that created them.
    drop(cpu);
    assert_eq!(get(&peer, Register::X(0)), 0);
    let mut codeword = [0; 4];
    peer.read_memory(0x10000, &mut codeword)?;
    assert_eq!(u32::from_le_bytes(codeword), 0x91000400);
    peer.unmap(0x20000, 0x4000)?;
    assert!(peer.read_memory(0x20000, &mut codeword).is_err());
    drop(peer);
    drop(vm);
    // Run the same device deadline/GIC path through the common factory. A
    // virtual timer asserts SPI32 at tick7, with no host clock involved.
    for quantum in [1, 100] {
        use fujimod_emulator::board::{
            InterruptMap,
            bus::{Bus, Mode},
            factory::*,
        };
        use fujimod_emulator_ip_arm::gicv3::Gic;
        use fujimod_emulator_devices::{Cx, Device};
        struct Deadline {
            due: Option<u64>,
            fired: Vec<u64>,
        }
        impl Device for Deadline {
            fn name(&self) -> &'static str {
                "deadline-check"
            }
            fn read(&mut self, _: &mut Cx<'_>, _: u64, _: u8) -> u64 {
                0
            }
            fn write(&mut self, cx: &mut Cx<'_>, address: u64, _: u8, _: u64) {
                if address == 0x8000000 {
                    self.due = Some(cx.now() + 5);
                } else {
                    cx.set_irq(32, false);
                }
            }
            fn next_deadline(&self, _: u64) -> Option<u64> {
                self.due
            }
            fn advance(&mut self, cx: &mut Cx<'_>) {
                if self.due.is_some_and(|at| at <= cx.now()) {
                    self.due = None;
                    self.fired.push(cx.now());
                    cx.set_irq(32, true);
                }
            }
            fn save(&self) -> serde_json::Value {
                serde_json::json!({"fired":self.fired})
            }
        }
        let mut bus = Bus::new(Mode::Strict);
        bus.register(
            0x8000000,
            8,
            Box::new(Deadline {
                due: None,
                fired: vec![],
            }),
        )?;
        let mut gic = Gic::new(vec![0], 512)?;
        gic.distributor(0, 4, Some(7))?;
        gic.redistributor(0, 0x14, 4, Some(0))?;
        gic.distributor(0x84, 4, Some(1))?;
        gic.distributor(0x104, 4, Some(1))?;
        gic.distributor(0x420, 1, Some(0x40))?;
        gic.system(0, 0xc230, Some(255))?;
        gic.system(0, 0xc667, Some(1))?;
        let graph = Graph {
            decoders: vec![],
            processors: vec![Processor {
                rprofile: None,
                mprofile: None,
                model: Model::ArmV8A,
                released: true,
                registers: vec![
                    (Register::Pstate, 0x345),
                    (Register::Pc, 0x10000),
                    (Register::X(1), 0x8000000),
                    (sys(0xc600), 0x20000),
                ],
            }],
            dma_mappings: vec![
                fujimod_emulator::board::memory::BusMapping::ram(0x10000, 0x4000),
                fujimod_emulator::board::memory::BusMapping::ram(0x20000, 0x4000),
            ],
            ram: vec![
                Ram {
                    base: 0x10000,
                    size: 0x4000,
                },
                Ram {
                    base: 0x20000,
                    size: 0x4000,
                },
            ],
            mappings: vec![
                MemoryMap {
                    core: 0,
                    address: 0x10000,
                    physical: 0x10000,
                    size: 0x4000,
                    permissions: 7,
                },
                MemoryMap {
                    core: 0,
                    address: 0x20000,
                    physical: 0x20000,
                    size: 0x4000,
                    permissions: 7,
                },
            ],
            peripherals: vec![Peripheral {
                core: 0,
                address: 0x8000000,
                physical: 0x8000000,
                size: 0x1000,
            }],
            instructions_per_tick: fujimod_emulator_cpu::timeline::Ratio::new(1, 1)?,
        };
        let mut built = graph.build(
            Options::new(Engine::Hypervisor).with_translation_cache_size_bytes(1024 * 1024),
            bus,
            Some(InterruptMap {
                gic: Box::new(gic),
                registers: fujimod_emulator::board::InterruptRegisters::GicV3 {
                    distributor: 0xfa000000,
                    redistributor: 0xfa080000,
                    stride: 0x20000,
                },
                cores: 1,
            }),
        )?;
        code(
            built.cpu(0)?,
            0x10000,
            &[0x52800520, 0xb9000020, 0x14000000],
        );
        code(
            built.cpu(0)?,
            0x20280,
            &[0xd538cc0d, 0xb900043f, 0xd518cc2d, 0x91000463, 0xd69f03e0],
        );
        built.run_until(30, quantum)?;
        let saved = built.state();
        assert_eq!(saved["timeline"]["now"], 30);
        assert_eq!(saved["instructions"], serde_json::json!([30]));
        assert_eq!(
            saved["bus"]["devices"][0]["state"]["fired"],
            serde_json::json!([7])
        );
        assert_eq!(get(built.cpu(0)?, Register::X(3)), 1);
        assert_eq!(get(built.cpu(0)?, Register::Pc), 0x10008);
        assert_eq!(saved, built.state());
    }
    // A hardware reset pin releases a real M-profile peer. Its IRQ handler
    // reads an opaque request, adds one in guest instructions and writes the
    // reply through the same mailbox model. No reply substitute participates.
    for quantum in [1, 64] {
        use fujimod_emulator::board::{
            InterruptMap,
            bus::{Bus, Mode},
            factory::*,
            mprofile,
        };
        use fujimod_emulator_ip_arm::gicv3::Gic;
        use fujimod_emulator_xp5_io::mailbox::{Channel, Mailbox, Pins};
        use fujimod_emulator_devices::{Cx, Device};
        struct Reset;
        impl Device for Reset {
            fn name(&self) -> &'static str {
                "reset-pin"
            }
            fn read(&mut self, _: &mut Cx<'_>, _: u64, _: u8) -> u64 {
                0
            }
            fn write(&mut self, cx: &mut Cx<'_>, _: u64, _: u8, value: u64) {
                if let Err(e) = cx.reset_core(1, value & 1 == 0) {
                    cx.fail(e);
                }
            }
        }
        let mut bus = Bus::new(Mode::Strict);
        bus.register(0x8000000, 4, Box::new(Reset))?;
        bus.register(
            0xff90e100,
            0x24,
            Box::new(Mailbox::new(
                0xff90e100,
                Pins {
                    companion: 1,
                    request: 55,
                    control: 58,
                    reply: 141,
                    status: 142,
                    fault: 143,
                },
                Channel::default(),
            )),
        )?;
        let mut gic = Gic::new(vec![0], 512)?;
        gic.distributor(0, 4, Some(7))?;
        gic.redistributor(0, 0x14, 4, Some(0))?;
        gic.distributor(0x90, 4, Some(1 << 13))?;
        gic.distributor(0x110, 4, Some(1 << 13))?;
        gic.distributor(0x48d, 1, Some(0x40))?;
        gic.system(0, 0xc230, Some(255))?;
        gic.system(0, 0xc667, Some(1))?;
        let graph = Graph {
            decoders: vec![],
            processors: vec![
                Processor {
                    model: Model::ArmV8A,
                    rprofile: None,
                    mprofile: None,
                    released: true,
                    registers: vec![
                        (Register::Pstate, 0x345),
                        (Register::Pc, 0x10000),
                        (Register::X(1), 0xff90e100),
                        (Register::X(2), 0x8000000),
                        (sys(0xc600), 0x20000),
                    ],
                },
                Processor {
                    model: Model::CortexM4,
                    rprofile: None,
                    mprofile: Some(mprofile::Config {
                        clock_hz: 600_000_000,
                        board_ticks_per_second: 1_000_000_000,
                        vector_base: 0,
                    }),
                    released: false,
                    registers: vec![],
                },
            ],
            dma_mappings: vec![
                fujimod_emulator::board::memory::BusMapping::ram(0x10000, 0x4000),
                fujimod_emulator::board::memory::BusMapping::ram(0x20000, 0x4000),
                fujimod_emulator::board::memory::BusMapping::ram(0x30000, 0x4000),
            ],
            ram: vec![
                Ram {
                    base: 0x10000,
                    size: 0x4000,
                },
                Ram {
                    base: 0x20000,
                    size: 0x4000,
                },
                Ram {
                    base: 0x30000,
                    size: 0x4000,
                },
            ],
            mappings: vec![
                MemoryMap {
                    core: 0,
                    address: 0x10000,
                    physical: 0x10000,
                    size: 0x4000,
                    permissions: 7,
                },
                MemoryMap {
                    core: 0,
                    address: 0x20000,
                    physical: 0x20000,
                    size: 0x4000,
                    permissions: 7,
                },
                MemoryMap {
                    core: 0,
                    address: 0x30000,
                    physical: 0x30000,
                    size: 0x4000,
                    permissions: 7,
                },
                MemoryMap {
                    core: 1,
                    address: 0,
                    physical: 0x30000,
                    size: 0x4000,
                    permissions: 7,
                },
            ],
            peripherals: vec![
                Peripheral {
                    core: 0,
                    address: 0x8000000,
                    physical: 0x8000000,
                    size: 0x1000,
                },
                Peripheral {
                    core: 0,
                    address: 0xff90e000,
                    physical: 0xff90e000,
                    size: 0x1000,
                },
                Peripheral {
                    core: 1,
                    address: 0x5f90e000,
                    physical: 0xff90e000,
                    size: 0x1000,
                },
            ],
            instructions_per_tick: fujimod_emulator_cpu::timeline::Ratio::new(1, 1)?,
        };
        let mut board = graph.build(
            Options::new(Engine::Hypervisor).with_translation_cache_size_bytes(1024 * 1024),
            bus,
            Some(InterruptMap {
                gic: Box::new(gic),
                registers: fujimod_emulator::board::InterruptRegisters::GicV3 {
                    distributor: 0xfa000000,
                    redistributor: 0xfa080000,
                    stride: 0x20000,
                },
                cores: 1,
            }),
        )?;
        code(
            board.cpu(0)?,
            0x10000,
            &[
                0x52800020, 0xb9000040, 0x52824680, 0xb9000020, 0xd503207f, 0xd503207f,
            ],
        );
        code(
            board.cpu(0)?,
            0x20280,
            &[
                0xd538cc0d, 0xb9401023, 0x52800204, 0xb9002024, 0xd518cc2d, 0xd69f03e0,
            ],
        );
        board.cpu(0)?.write_memory(
            0x30000,
            &[0x3000u32, 0x1001]
                .into_iter()
                .flat_map(u32::to_le_bytes)
                .collect::<Vec<_>>(),
        )?;
        board
            .cpu(0)?
            .write_memory(0x30000 + 71 * 4, &0x1101u32.to_le_bytes())?;
        board.cpu(0)?.write_memory(
            0x31000,
            &[
                0x4e, 0xf2, 0x00, 0x10, 0xce, 0xf2, 0x00, 0x00, 0x4f, 0xf4, 0x00, 0x01, 0x41, 0x60,
                0x30, 0xbf, 0xfd, 0xe7,
            ],
        )?;
        board.cpu(0)?.write_memory(
            0x31100,
            &[
                0x4e, 0xf2, 0x00, 0x10, 0xc5, 0xf6, 0x90, 0x70, 0x01, 0x68, 0x01, 0x31, 0x01, 0x61,
                0x01, 0x22, 0x02, 0x62, 0x70, 0x47,
            ],
        )?;
        board.run_until(100, quantum)?;
        assert_eq!(get(board.cpu(0)?, Register::X(3)), 0x1235);
        let saved = board.state();
        assert_eq!(saved["timeline"]["now"], 100);
        assert_eq!(saved["bus"]["devices"][1]["state"]["causes"], 0);
        assert_eq!(
            saved["mprofile"][1]["nvic"]["active"],
            serde_json::json!([])
        );
        assert_eq!(saved["waiting"], serde_json::json!([true, true]));
    }
    // Both directions share one physical reservation domain. The reverse
    // case writes 7 -> 99 -> 7 from the A53 after the M4's exclusive load.
    // Also replace an already translated M4 instruction through its A53 alias.
    for case in 0..3 {
        let reverse = case == 1;
        let replace_code = case == 2;
        use fujimod_emulator::board::{
            bus::{Bus, Mode},
            factory::*,
            mprofile,
        };
        let graph = Graph {
            decoders: vec![],
            processors: vec![
                Processor {
                    model: Model::ArmV8A,
                    rprofile: None,
                    mprofile: None,
                    released: true,
                    registers: vec![
                        (Register::Pc, 0x10000),
                        (Register::X(1), if replace_code { 0x31004 } else { 0x30200 }),
                        (Register::X(0), if replace_code { 0x2609 } else { 99 }),
                        (sys(0xc100), 0x40000),
                        (sys(0xc102), 32),
                        (sys(0xc510), 0xff),
                        (sys(0xc080), 0x30d00801),
                    ],
                },
                Processor {
                    model: Model::CortexM4,
                    rprofile: None,
                    mprofile: Some(mprofile::Config {
                        clock_hz: 600_000_000,
                        board_ticks_per_second: 1_000_000_000,
                        vector_base: 0,
                    }),
                    released: true,
                    registers: vec![(arm::CORE[0], 99), (arm::CORE[1], 0x200)],
                },
            ],
            dma_mappings: vec![
                fujimod_emulator::board::memory::BusMapping::ram(0x10000, 0x4000),
                fujimod_emulator::board::memory::BusMapping::ram(0x30000, 0x4000),
                fujimod_emulator::board::memory::BusMapping::ram(0x40000, 0x4000),
            ],
            ram: vec![
                Ram {
                    base: 0x10000,
                    size: 0x4000,
                },
                Ram {
                    base: 0x30000,
                    size: 0x4000,
                },
                Ram {
                    base: 0x40000,
                    size: 0x4000,
                },
            ],
            mappings: vec![
                MemoryMap {
                    core: 0,
                    address: 0x10000,
                    physical: 0x10000,
                    size: 0x4000,
                    permissions: 7,
                },
                MemoryMap {
                    core: 0,
                    address: 0x30000,
                    physical: 0x30000,
                    size: 0x4000,
                    permissions: 7,
                },
                MemoryMap {
                    core: 0,
                    address: 0x40000,
                    physical: 0x40000,
                    size: 0x4000,
                    permissions: 7,
                },
                MemoryMap {
                    core: 1,
                    address: 0,
                    physical: 0x30000,
                    size: 0x4000,
                    permissions: 7,
                },
            ],
            peripherals: vec![],
            instructions_per_tick: fujimod_emulator_cpu::timeline::Ratio::new(1, 1)?,
        };
        let mut board = graph.build(
            Options::new(Engine::Hypervisor).with_translation_cache_size_bytes(1024 * 1024),
            Bus::new(Mode::Strict),
            None,
        )?;
        board
            .cpu(0)?
            .write_memory(0x40000, &0x401u64.to_le_bytes())?;
        board.cpu(0)?.write_memory(0x30200, &7u32.to_le_bytes())?;
        code(
            board.cpu(0)?,
            0x10000,
            if replace_code {
                &[0xd503201f, 0x79000020, 0xd503207f]
            } else if reverse {
                &[0xd503201f, 0xb9000020, 0xd28000e0, 0xb9000020, 0xd503207f]
            } else {
                &[0x885f7c20, 0x88027c20, 0xd503207f]
            },
        );
        board.cpu(0)?.write_memory(
            0x30000,
            &[0x3000u32, 0x1001]
                .into_iter()
                .flat_map(u32::to_le_bytes)
                .collect::<Vec<_>>(),
        )?;
        let m4: &[u16] = if replace_code {
            &[0xbf00, 0xbf00, 0x2601, 0xbf30]
        } else if reverse {
            &[0xe851, 0x3f00, 0xbf00, 0xbf00, 0xe841, 0x0200, 0xbf30]
        } else {
            &[0x6008, 0xbf30]
        };
        board.cpu(0)?.write_memory(
            0x31000,
            &m4.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>(),
        )?;
        board.run_until(20, 1)?;
        if replace_code {
            assert_eq!(get(board.cpu(1)?, arm::CORE[6]), 9);
            continue;
        }
        assert_eq!(
            get(
                board.cpu(if reverse { 1 } else { 0 })?,
                if reverse {
                    arm::CORE[2]
                } else {
                    Register::X(2)
                }
            ),
            1
        );
        let mut word = [0; 4];
        board.cpu(0)?.read_memory(0x30200, &mut word)?;
        assert_eq!(u32::from_le_bytes(word), if reverse { 7 } else { 99 });
    }
    architectural_board_checks()?;
    durable::run()?;
    println!(
        "PASS: bounded execution, two workers, callback reentrancy, snapshots, SVC/ERET, IRQ, shared GIC/ICC, WFI, counter trap, MMIO, native MMU, exclusives/peer stores/CLREX, code replacement, cancellation, map lifetime and shared factory deadline/IRQ scheduling and M-profile firmware mailbox execution and peer-store reservation invalidation"
    );
    Ok(())
}
