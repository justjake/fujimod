//! Translated block accounting must land on a known guest and device state.
//!
//! The program exercises a coarse debit inside a block, an exact stop, a guest
//! timer read with its deadline, a self-modifying store and an ARM/Thumb call.
//! Both policies are pinned to the state the engine produced when the
//! translated path replaced the helper path.
use super::*;
use fujimod_emulator_cpu::Model;
use fujimod_emulator_cpu::arm;
use fujimod_emulator_cpu_unicorn::Library;
use fujimod_emulator_devices::{Cx, Device};
use std::sync::{Arc, Mutex};

const BUDGET: u64 = 260;

#[derive(Default)]
struct Probe {
    value: u64,
    due: Option<u64>,
    calls: Vec<(u64, u64)>,
}
struct Timer(Arc<Mutex<Probe>>);
impl Device for Timer {
    fn name(&self) -> &'static str {
        "accounting-timer"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, _: u8) -> u64 {
        let mut probe = self.0.lock().unwrap();
        probe.calls.push((cx.now(), address));
        probe.value
    }
    fn write(&mut self, cx: &mut Cx<'_>, _: u64, _: u8, value: u64) {
        let mut probe = self.0.lock().unwrap();
        probe.value = value;
        probe.due = Some(cx.now() + 7);
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.0.lock().unwrap().due
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        let mut probe = self.0.lock().unwrap();
        if probe.due.is_some_and(|at| at <= cx.now()) {
            probe.due = None;
            probe.value += 1;
            probe.calls.push((cx.now(), 0));
        }
    }
    fn save(&self) -> Value {
        let probe = self.0.lock().unwrap();
        json!({"due":probe.due,"calls":probe.calls,"value":probe.value})
    }
}

fn set(cpu: &dyn Cpu, register: Register, value: u32) {
    cpu.write_register(register, &value.to_le_bytes()).unwrap();
}

fn words(cpu: &dyn Cpu, address: u64, values: &[u32]) {
    cpu.write_memory(
        address,
        &values
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<_>>(),
    )
    .unwrap();
}

fn build() -> (Board, Arc<Mutex<Probe>>) {
    let library = Library::linked().unwrap();
    let raw = library.open(Model::CortexR5F, 4 << 20).unwrap();
    let cpu: Box<dyn Cpu> = Box::new(
        fujimod_emulator_cpu_unicorn::rprofile::core::Core::new(
            raw,
            fujimod_emulator_cpu_unicorn::rprofile::core::Options::default(),
            fujimod_emulator_cpu_unicorn::rprofile::core::Wiring {
                memory: Default::default(),
            },
        )
        .unwrap(),
    );
    cpu.map_zeroed(0x10000, 0x1000, 7).unwrap();
    cpu.map_zeroed(0x20000, 0x1000, 7).unwrap();
    cpu.map_zeroed(0x11000, 0x1000, 7).unwrap();
    words(
        &*cpu,
        0x10000,
        &[
            0xe3a0_0000, // mov r0, #0
            0xe585_0000, // str r0, [r5] (timer write)
            0xe595_2000, // ldr r2, [r5] (timer read)
            0xe080_0002, // add r0, r0, r2
            0xe594_1000, // ldr r1, [r4]
            0xe281_1001, // add r1, r1, #1
            0xe584_1000, // str r1, [r4]
            0xe594_1000, // ldr r1, [r4]
            0xe281_1001, // add r1, r1, #1
            0xe584_1000, // str r1, [r4]
            0xe597_3000, // ldr r3, [r7]
            0xe1a0_800f, // mov r8, pc
            0xe588_3000, // str r3, [r8] (patch code at 0x10034)
            0xe1a0_0000, // nop, patched to str r0, [r4, #16]
            0xe584_0008, // str r0, [r4, #8]
            0xe12f_ff39, // blx r9 (Thumb)
            0xe584_000c, // str r0, [r4, #12]
            0xeaff_fffe, // b .
        ],
    );
    // Thumb leaf: adds r0, #2; bx lr
    words(&*cpu, 0x11000, &[0x4770_3002]);
    // Patch word and initial data.
    words(&*cpu, 0x20000, &[0, 0xe584_0010, 0, 0, 0]);
    set(&*cpu, arm::CORE[4], 0x20000);
    set(&*cpu, arm::CORE[5], 0x8000000);
    set(&*cpu, arm::CORE[7], 0x20004);
    set(&*cpu, arm::CORE[9], 0x11001);
    set(&*cpu, arm::CPSR, 0xd3);
    set(&*cpu, Register::Pc, 0x10000);
    let probe = Arc::new(Mutex::new(Probe::default()));
    let mut bus = Bus::new(bus::Mode::Strict);
    bus.register(0x8000000, 4, Box::new(Timer(probe.clone())))
        .unwrap();
    let mut board = Board::new(
        vec![cpu],
        bus,
        Ratio::new(1, 1).unwrap(),
        None,
        vec![true],
        memory::Memory::default(),
    )
    .unwrap();
    board.map_mmio(0, 0x8000000, 0x1000, 0x8000000).unwrap();
    (board, probe)
}

struct Case {
    policy: ExecutionPolicy,
    registers: [u32; 16],
    ram: [u8; 20],
    clock: u64,
    probe: Value,
}

fn cases() -> [Case; 2] {
    [
        Case {
            policy: ExecutionPolicy::Interactive,
            registers: [
                3,
                2,
                1,
                3_850_633_232,
                0x20000,
                0x8000000,
                0,
                0x20004,
                0x10034,
                0x11001,
                0,
                0,
                0,
                0,
                0x10040,
                0x10044,
            ],
            ram: [
                2, 0, 0, 0, 0x10, 0, 0x84, 0xe5, 1, 0, 0, 0, 3, 0, 0, 0, 1, 0, 0, 0,
            ],
            clock: 252,
            probe: json!({"calls":[[24,0],[24,0x8000000]],"due":null,"value":1}),
        },
        Case {
            policy: ExecutionPolicy::Exact,
            registers: [
                2,
                2,
                0,
                3_850_633_232,
                0x20000,
                0x8000000,
                0,
                0x20004,
                0x10034,
                0x11001,
                0,
                0,
                0,
                0,
                0x10040,
                0x10044,
            ],
            ram: [
                2, 0, 0, 0, 0x10, 0, 0x84, 0xe5, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0,
            ],
            clock: 260,
            probe: json!({"calls":[[3,0x8000000],[9,0]],"due":null,"value":1}),
        },
    ]
}

#[test]
fn translated_accounting_lands_on_the_expected_state() {
    for case in cases() {
        let (mut board, probe) = build();
        board.set_execution_policy(case.policy).unwrap();
        board.run_for_instructions(BUDGET, 1000).unwrap();

        assert_eq!(
            board.instruction_counts(),
            vec![BUDGET],
            "{:?}: instruction count",
            case.policy
        );
        let cpu = board.cpu(0).unwrap();
        let registers = arm::CORE
            .iter()
            .map(|&register| {
                let mut bytes = [0; 4];
                cpu.read_register(register, &mut bytes).unwrap();
                u32::from_le_bytes(bytes)
            })
            .collect::<Vec<_>>();
        assert_eq!(
            registers,
            case.registers.to_vec(),
            "{:?}: architectural registers",
            case.policy
        );
        assert_eq!(board.now(), case.clock, "{:?}: board clock", case.policy);

        let mut ram = vec![0; case.ram.len()];
        board.read_memory(0, 0x20000, &mut ram).unwrap();
        assert_eq!(ram, case.ram.to_vec(), "{:?}: guest RAM", case.policy);

        // Serializing the board reads every device, so the probe lock must not
        // be held across it.
        let device = {
            let probe = probe.lock().unwrap();
            json!({"due":probe.due,"calls":probe.calls,"value":probe.value})
        };
        assert_eq!(device, case.probe, "{:?}: timer device", case.policy);
    }
}

#[cfg(all(
    feature = "dynarmic",
    any(target_arch = "aarch64", target_arch = "x86_64"),
    any(target_os = "linux", target_os = "macos")
))]
#[test]
fn dynarmic_scheduler_window_matches_instruction_observation() {
    let run = |observe: bool| {
        let cpu = Box::new(fujimod_emulator_cpu_dynarmic::Core::new(Model::CortexA7).unwrap());
        cpu.map_zeroed(0x10000, 0x1000, 7).unwrap();
        words(
            &*cpu,
            0x10000,
            &[
                0xe3a0_0029, // mov r0, #41
                0xe280_0001, // add r0, r0, #1
                0xe585_0000, // str r0, [r5] (arms the deadline)
                0xe595_1000, // ldr r1, [r5] (before deadline)
                0xe284_4001, // add r4, r4, #1
                0xe284_4001,
                0xe284_4001,
                0xe284_4001,
                0xe284_4001,
                0xe284_4001,
                0xe595_2000, // ldr r2, [r5] (after deadline)
                0xee10_3fb0, // mrc p15, 0, r3, c0, c0, 5 (board MPIDR)
                0xe284_4001, // add r4, r4, #1
                0xeaff_fffe, // b .
            ],
        );
        set(&*cpu, Register::Pc, 0x10000);
        set(&*cpu, arm::CORE[5], 0x8000000);
        set(&*cpu, arm::CPSR, 0xd3);
        let probe = Arc::new(Mutex::new(Probe::default()));
        let mut bus = Bus::new(bus::Mode::Strict);
        bus.register(0x8000000, 4, Box::new(Timer(probe.clone())))
            .unwrap();
        let mut board = Board::new(
            vec![cpu],
            bus,
            Ratio::new(1, 1).unwrap(),
            None,
            vec![true],
            memory::Memory::default(),
        )
        .unwrap();
        board.set_execution_policy(ExecutionPolicy::Exact).unwrap();
        board.map_mmio(0, 0x8000000, 0x1000, 0x8000000).unwrap();
        board
            .configure_armv7a(
                0,
                armv7a::Config {
                    mpidr: 0x8000_0003,
                    configuration_base: 0xffff_0000,
                    cycles_per_tick: 1,
                    cache_sizes: None,
                },
            )
            .unwrap();
        let points = observe.then(|| board.instruction_points_begin(0, vec![0x10100]).unwrap());
        board.run_for_instructions(48, 32).unwrap();
        assert!(points.is_none_or(|points| points.event().is_none()));
        let results = [
            arm::CORE[1],
            arm::CORE[2],
            arm::CORE[3],
            arm::CORE[4],
            Register::Pc,
        ]
        .map(|register| board.read_register(0, register).unwrap());
        let accesses = probe.lock().unwrap().calls.clone();
        (board.instruction_counts(), board.now(), results, accesses)
    };
    let window = run(false);
    assert_eq!(window, run(true));
    assert_eq!(window.0, [48]);
    assert_eq!(window.1, 48);
    assert_eq!(window.2, [42, 43, 0x8000_0003, 7, 0x10034]);
    assert_eq!(window.3, [(3, 0x8000000), (9, 0), (10, 0x8000000)]);
}

#[cfg(all(
    feature = "dynarmic",
    target_arch = "aarch64",
    any(target_os = "linux", target_os = "macos")
))]
#[test]
fn projected_device_reads_preserve_fractional_clock_and_deadline() {
    struct ProjectedCounter {
        due: Option<u64>,
        expired: bool,
    }
    impl ProjectedCounter {
        fn at(&self, now: u64) -> u64 {
            now + u64::from(self.expired) * 100
        }
    }
    impl Device for ProjectedCounter {
        fn name(&self) -> &'static str {
            "projected-counter"
        }
        fn read(&mut self, cx: &mut Cx<'_>, _: u64, _: u8) -> u64 {
            self.at(cx.now())
        }
        fn read_without_advance(&self, _: u64, size: u8, now: u64) -> Option<u64> {
            (size == 4 && self.due.is_none_or(|due| now < due)).then(|| self.at(now))
        }
        fn write(&mut self, cx: &mut Cx<'_>, _: u64, _: u8, _: u64) {
            cx.fail("counter is read only");
        }
        fn next_deadline(&self, _: u64) -> Option<u64> {
            self.due
        }
        fn advance(&mut self, cx: &mut Cx<'_>) {
            if self.due.is_some_and(|due| cx.now() >= due) {
                self.expired = true;
                self.due = None;
            }
        }
        fn save(&self) -> Value {
            json!({"due": self.due, "expired": self.expired})
        }
    }
    let run = |observe: bool| {
        let cpu = Box::new(fujimod_emulator_cpu_dynarmic::Core::new(Model::CortexA7).unwrap());
        cpu.map_zeroed(0x10000, 0x1000, 7).unwrap();
        cpu.map_zeroed(0x20000, 0x1000, 7).unwrap();
        words(
            &*cpu,
            0x10000,
            &[
                0xe595_0000, // ldr r0, [r5]
                0xe486_0004, // str r0, [r6], #4
                0xeaff_fffc, // b back to the load
            ],
        );
        set(&*cpu, Register::Pc, 0x10000);
        set(&*cpu, arm::CORE[5], 0x8000000);
        set(&*cpu, arm::CORE[6], 0x20000);
        set(&*cpu, arm::CPSR, 0xd3);
        let mut bus = Bus::new(bus::Mode::Strict);
        bus.register(
            0x8000000,
            4,
            Box::new(ProjectedCounter {
                due: Some(7),
                expired: false,
            }),
        )
        .unwrap();
        let mut board = Board::new(
            vec![cpu],
            bus,
            Ratio::new(3, 2).unwrap(),
            None,
            vec![true],
            memory::Memory::default(),
        )
        .unwrap();
        board.set_execution_policy(ExecutionPolicy::Exact).unwrap();
        board.map_mmio(0, 0x8000000, 0x1000, 0x8000000).unwrap();
        board
            .configure_armv7a(
                0,
                armv7a::Config {
                    mpidr: 0x8000_0000,
                    configuration_base: 0xffff_0000,
                    cycles_per_tick: 1,
                    cache_sizes: None,
                },
            )
            .unwrap();
        let points = observe.then(|| board.instruction_points_begin(0, vec![0x10100]).unwrap());
        board.run_for_instructions(30, 16).unwrap();
        assert!(points.is_none_or(|points| points.event().is_none()));
        let mut result = [0; 40];
        board.read_memory(0, 0x20000, &mut result).unwrap();
        let values = result
            .as_chunks::<4>()
            .0
            .iter()
            .copied()
            .map(u32::from_le_bytes)
            .collect::<Vec<_>>();
        let projected = board.cpu(0).unwrap().exit_counters().unwrap()["projected_mmio_reads"];
        ((board.instruction_counts(), board.now(), values), projected)
    };
    let (batch, projected) = run(false);
    let (observed, observed_projected) = run(true);
    assert_eq!(batch, observed);
    assert_eq!(batch.0, [30]);
    assert_eq!(batch.2, [0, 2, 4, 6, 108, 110, 112, 114, 116, 118]);
    assert!(
        projected >= 5,
        "expected native projected reads, got {projected}"
    );
    assert_eq!(observed_projected, 0);
}

#[cfg(all(
    feature = "dynarmic",
    target_arch = "aarch64",
    any(target_os = "linux", target_os = "macos")
))]
#[test]
fn repeated_read_window_expires_at_a_deadline_and_a_guest_write() {
    struct Status {
        value: u64,
        deadline: Option<u64>,
        countdown: bool,
        sampled_reads: Arc<Mutex<u64>>,
    }
    impl Status {
        fn value_at(&self, at: u64) -> u64 {
            self.deadline
                .filter(|&deadline| self.countdown && at < deadline)
                .map_or(self.value, |deadline| deadline - at - 1)
        }
    }
    impl Device for Status {
        fn name(&self) -> &'static str {
            "stable-status"
        }
        fn read_without_advance(&self, _: u64, size: u8, at: u64) -> Option<u64> {
            (size == 4 && self.deadline.is_none_or(|deadline| at < deadline)).then(|| {
                *self.sampled_reads.lock().unwrap() += 1;
                self.value_at(at)
            })
        }
        fn projected_value_until(
            &self,
            _: u64,
            size: u8,
        ) -> Option<fujimod_emulator_devices::bus::ReadValidity> {
            (size == 4).then_some(fujimod_emulator_devices::bus::ReadValidity {
                before: self.deadline.unwrap_or(u64::MAX),
                countdown_expiry: self.deadline.filter(|_| self.countdown),
            })
        }
        fn read(&mut self, cx: &mut Cx<'_>, _: u64, _: u8) -> u64 {
            self.value_at(cx.now())
        }
        fn write(&mut self, _: &mut Cx<'_>, _: u64, _: u8, value: u64) {
            self.value = value;
        }
        fn next_deadline(&self, _: u64) -> Option<u64> {
            self.deadline
        }
        fn advance(&mut self, cx: &mut Cx<'_>) {
            if self.deadline.is_some_and(|deadline| deadline <= cx.now()) {
                self.deadline = None;
                self.value = 9;
            }
        }
    }

    for countdown in [false, true] {
        let cpu = Box::new(fujimod_emulator_cpu_dynarmic::Core::new(Model::CortexA7).unwrap());
        cpu.map_zeroed(0x10000, 4096, 7).unwrap();
        words(
            &*cpu,
            0x10000,
            &[
                0xe595_0000,
                0xe595_1000,
                0xe595_2000,
                0xeaff_ffff, // end the native block before the deadline
                0xe595_3000, // read at the deadline
                0xe3a0_400b,
                0xe585_4000,
                0xe595_4000,
                0xe595_7000,
                0xeaff_fffe,
            ],
        );
        set(&*cpu, Register::Pc, 0x10000);
        set(&*cpu, arm::CORE[5], 0x8000000);
        set(&*cpu, arm::CPSR, 0xd3);
        let mut bus = Bus::new(bus::Mode::Strict);
        let sampled_reads = Arc::new(Mutex::new(0));
        bus.register(
            0x8000000,
            4,
            Box::new(Status {
                value: 7,
                deadline: Some(4),
                countdown,
                sampled_reads: Arc::clone(&sampled_reads),
            }),
        )
        .unwrap();
        let mut board = Board::new(
            vec![cpu],
            bus,
            Ratio::new(1, 1).unwrap(),
            None,
            vec![true],
            memory::Memory::default(),
        )
        .unwrap();
        board.set_execution_policy(ExecutionPolicy::Exact).unwrap();
        board.map_mmio(0, 0x8000000, 4096, 0x8000000).unwrap();
        board
            .configure_armv7a(
                0,
                armv7a::Config {
                    mpidr: 0x8000_0000,
                    configuration_base: 0xffff_0000,
                    cycles_per_tick: 1,
                    cache_sizes: None,
                },
            )
            .unwrap();
        board.run_for_instructions(12, 32).unwrap();
        let cpu = board.cpu(0).unwrap();
        let values = [0, 1, 2, 3, 4, 7].map(|r| super::scalar(cpu, arm::CORE[r]).unwrap());
        let expected = if countdown {
            [3, 2, 1, 9, 11, 11]
        } else {
            [7, 7, 7, 9, 11, 11]
        };
        assert_eq!(values, expected);
        let projected = cpu.exit_counters().unwrap()["projected_mmio_reads"];
        let sampled = *sampled_reads.lock().unwrap();
        assert!(
            projected > sampled,
            "expected reuse across projected reads, got {projected} reads from {sampled} samples"
        );
    }
}
