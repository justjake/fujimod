use super::{Core, ffi};
use fujimod_emulator_cpu::code::{CodeTracker, Invalidations};
use std::cell::{Cell, RefCell};
use std::ffi::c_void;

pub(super) struct CodeTracking {
    tracker: CodeTracker,
    seen_watches: Cell<u64>,
    invalidations: RefCell<Invalidations>,
}

impl Core {
    /// Add this engine to the board's code tracker.
    pub fn with_code_tracker(mut self, tracker: CodeTracker) -> Result<Self, String> {
        let mut tracking = Box::new(CodeTracking {
            seen_watches: Cell::new(tracker.watch_revision()),
            invalidations: RefCell::new(tracker.subscribe()),
            tracker,
        });
        self.check(unsafe {
            ffi::uc_embedder_set_code_tracking(
                self.raw,
                Some(watched),
                Some(translated),
                Some(store),
                (&mut *tracking as *mut CodeTracking).cast(),
            )
        })?;
        self.code_tracking = Some(tracking);
        Ok(self)
    }
}

impl CodeTracking {
    pub(super) fn before_run(&self, cpu: &Core) -> Result<(), String> {
        let watches = self.tracker.watch_revision();
        if self.seen_watches.get() != watches {
            cpu.check(unsafe { ffi::uc_embedder_sync_code_tracking(cpu.raw) })?;
            self.seen_watches.set(watches);
        }
        let mut words = Vec::new();
        match self
            .invalidations
            .borrow_mut()
            .drain(|word| words.push(word))
        {
            Some(true) => {
                words.sort_unstable();
                words.dedup();
                for word in words {
                    cpu.invalidate_backing(word, 4)?;
                }
            }
            Some(false) => {
                use fujimod_emulator_cpu::Cpu;
                cpu.invalidate_code(None)?;
            }
            None => {}
        }
        Ok(())
    }
}

unsafe extern "C" fn watched(data: *mut c_void, backing: *const u8, size: usize) -> bool {
    let tracking = unsafe { &*data.cast::<CodeTracking>() };
    tracking.tracker.watches_pages(backing, size)
}

unsafe extern "C" fn translated(data: *mut c_void, backing: *const u8, size: usize) -> bool {
    let tracking = unsafe { &*data.cast::<CodeTracking>() };
    // The native engine provides the backing span used by the translation.
    let bytes = unsafe { std::slice::from_raw_parts(backing, size) };
    let Some(revision) = tracking.tracker.watch_bytes(backing, bytes) else {
        return false;
    };
    // Unicorn rearms the current translation's watches. Leave an unseen peer
    // registration pending for the next entry even if ours followed it.
    if tracking.seen_watches.get().wrapping_add(1) == revision {
        tracking.seen_watches.set(revision);
    }
    true
}

unsafe extern "C" fn store(data: *mut c_void, backing: *const u8, size: usize, completed: bool) {
    let tracking = unsafe { &*data.cast::<CodeTracking>() };
    if completed {
        unsafe { tracking.tracker.note_write(backing, size) };
    } else {
        tracking.tracker.note_possible_write(backing, size);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support;
    use fujimod_emulator_cpu::{Cpu, Model, Run, arm, memory::Backing};
    use std::ptr::NonNull;

    #[test]
    fn guest_store_reaches_code_translated_only_by_a_peer_at_an_alias() {
        let tracker = CodeTracker::default();
        let shared = Backing::aligned_zeros(0x1000, 0x1000).unwrap();
        let core = || {
            let cpu = test_support::library()
                .open(Model::CortexA7, 1 << 20)
                .unwrap()
                .with_code_tracker(tracker.clone())
                .unwrap();
            cpu.write_register(arm::CPSR, &0xd3u32.to_le_bytes())
                .unwrap();
            cpu
        };
        let reader = core();
        let writer = core();
        for (cpu, address) in [(&reader, 0x1000), (&writer, 0x4000)] {
            unsafe { cpu.map_backing(address, 0x1000, 7, NonNull::new(shared.as_ptr()).unwrap()) }
                .unwrap();
        }

        writer.map_zeroed(0x2000, 0x1000, 7).unwrap();
        writer
            .write_memory(0x2000, &0xe581_0000u32.to_le_bytes()) // str r0, [r1]
            .unwrap();
        let run = |cpu: &Core, begin| {
            cpu.run(Run {
                begin,
                until: None,
                instructions: Some(1),
                watchdog_us: 0,
            })
            .unwrap();
        };
        let write = |address: u32, value: u32| {
            writer
                .write_register(arm::CORE[0], &value.to_le_bytes())
                .unwrap();
            writer
                .write_register(arm::CORE[1], &address.to_le_bytes())
                .unwrap();
            run(&writer, 0x2000);
        };
        let execute = || {
            run(&reader, 0x1000);
            let mut out = [0; 4];
            reader.read_register(arm::CORE[0], &mut out).unwrap();
            u32::from_le_bytes(out)
        };

        let original = 0xe3a0_0001; // mov r0, #1
        reader
            .write_memory(0x1004, &0xeaff_fffeu32.to_le_bytes())
            .unwrap(); // b .
        write(0x4000, original); // Warm the writer's TLB before registration.
        assert!(!tracker.watched(shared.as_ptr()));
        assert_eq!(execute(), 1);
        assert!(tracker.watched(shared.as_ptr()));

        let mut invalidations = tracker.subscribe();
        write(0x4008, 0x1234);
        write(0x4000, original);
        assert_eq!(invalidations.drain(|_| panic!("unchanged code")), None);

        write(0x4000, 0xe3a0_0009); // mov r0, #9
        let mut changed = Vec::new();
        assert_eq!(invalidations.drain(|word| changed.push(word)), Some(true));
        assert_eq!(changed, [shared.as_ptr().addr()]);
        assert_eq!(execute(), 9);
    }
}
