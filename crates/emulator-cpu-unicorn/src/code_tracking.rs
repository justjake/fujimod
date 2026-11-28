use super::{Core, ffi};
use fujimod_emulator_cpu::code::CodeTracker;
use std::cell::Cell;
use std::ffi::c_void;

pub(super) struct CodeTracking {
    tracker: CodeTracker,
    seen_watches: Cell<u64>,
    seen_writes: Cell<u64>,
}

impl Core {
    /// Add this engine to the board's code tracker. All engines sharing this
    /// tracker must run on its owner thread.
    pub fn with_code_tracker(mut self, tracker: CodeTracker) -> Result<Self, String> {
        let mut tracking = Box::new(CodeTracking {
            seen_watches: Cell::new(tracker.watch_revision()),
            seen_writes: Cell::new(tracker.revision()),
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
        let writes = self.tracker.revision();
        if self.seen_writes.get() != writes {
            let mut words = Vec::new();
            if self
                .tracker
                .changed_since(self.seen_writes.get(), |word| words.push(word))
            {
                words.sort_unstable();
                words.dedup();
                for word in words {
                    cpu.invalidate_backing(word, 4)?;
                }
            } else {
                // The bounded journal no longer covers this engine.
                use fujimod_emulator_cpu::Cpu;
                cpu.invalidate_code(None)?;
            }
            self.seen_writes.set(writes);
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
    if let Some(revision) = tracking.tracker.watch_bytes(backing, bytes) {
        // Unicorn rearms its own TLB when this callback adds a watch.
        tracking.seen_watches.set(revision);
        return true;
    }
    false
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

        let original_revision = tracker.revision();
        write(0x4008, 0x1234);
        write(0x4000, original);
        assert_eq!(tracker.revision(), original_revision);

        write(0x4000, 0xe3a0_0009); // mov r0, #9
        let mut changed = Vec::new();
        assert!(tracker.changed_since(original_revision, |word| changed.push(word)));
        assert_eq!(changed, [shared.as_ptr().addr()]);
        assert_eq!(execute(), 9);
    }
}
