//! Exact-policy instruction boundaries for callbacks during guest accesses.
use super::tests::{get, processor_with_options, run, set, words};
use super::*;

struct WatchedStore<'a> {
    cpu: &'a Core,
    calls: Cell<u32>,
}

unsafe fn stop_on_watched_store(_: Access, _: u64, size: i32, _: i64, data: *mut c_void) {
    let watched = unsafe { &*data.cast::<WatchedStore>() };
    assert_eq!(size, 4);
    watched.calls.set(watched.calls.get() + 1);
    watched.cpu.stop().unwrap();
}

fn setup() -> Core {
    let cpu = processor_with_options(&Memory::default(), Options::default());
    cpu.map_zeroed(0x1000, 0x1000, 7).unwrap();
    cpu.set_execution_policy(ExecutionPolicy::Exact).unwrap();
    set(&cpu, arm::CPSR, 0xd3);
    cpu
}

#[test]
fn a_watched_ram_store_stops_after_the_store_instruction() {
    let cpu = setup();
    cpu.map_zeroed(0x2000, 0x1000, 7).unwrap();
    words(&cpu, 0x1000, &[0xe5810000, 0xe2822001, 0xe320f003]);
    set(&cpu, arm::CORE[0], 0x1234_5678);
    set(&cpu, arm::CORE[1], 0x2000);
    let watched = WatchedStore {
        cpu: &cpu,
        calls: Cell::new(0),
    };
    let hook = unsafe {
        cpu.install_hook(Hook::Memory {
            callback: stop_on_watched_store,
            data: (&watched as *const WatchedStore).cast_mut().cast(),
            read: false,
            write: true,
            range: HookRange::Inclusive {
                begin: 0x2000,
                end: 0x2003,
            },
        })
    }
    .unwrap();

    assert!(matches!(run(&cpu, 0x1000, 100).unwrap(), Exit::Stopped));
    let mut bytes = [0; 4];
    cpu.read_memory(0x2000, &mut bytes).unwrap();
    assert_eq!(watched.calls.get(), 1);
    assert_eq!(u32::from_le_bytes(bytes), 0x1234_5678);
    assert_eq!(
        (
            get(&cpu, arm::PC),
            get(&cpu, arm::CORE[2]),
            cpu.instruction_progress()
        ),
        (0x1000, 0, Some(1))
    );
    cpu.remove_hook(hook).unwrap();
}

#[test]
fn plain_ram_access_loops_keep_exact_instruction_counts() {
    let cpu = setup();
    cpu.map_zeroed(0x2000, 0x1000, 7).unwrap();
    words(
        &cpu,
        0x1000,
        &[0xe5910000, 0xe2822001, 0xe5812000, 0xeafffffb],
    );
    set(&cpu, arm::CORE[1], 0x2000);
    cpu.write_memory(0x2000, &0u32.to_le_bytes()).unwrap();

    run(&cpu, 0x1000, 39).unwrap();

    let mut bytes = [0; 4];
    cpu.read_memory(0x2000, &mut bytes).unwrap();
    assert_eq!(cpu.instruction_progress(), Some(39));
    assert_eq!((get(&cpu, arm::PC), get(&cpu, arm::CORE[2])), (0x100c, 10));
    assert_eq!(u32::from_le_bytes(bytes), 10);
}
