//! Wire Unicorn's native AArch64 exclusives into the board's physical domain.

use super::*;
use crate::ffi;

impl Core {
    /// Join the exclusive monitor of this board's shared memory.
    pub fn with_board_exclusives(self, invalidates_on_write: bool) -> Result<Self, String> {
        let e = &self.execution;
        if e.a32 || e.a64_monitor.get().is_some() {
            return Err("board AArch64 exclusives must be configured once".into());
        }
        let owner = e
            .memory
            .monitors
            .lock()
            .map_err(|_| "exclusive monitors poisoned")?
            .register_native(64, invalidates_on_write)?;
        e.watch_a64_exclusives.set(invalidates_on_write);
        e.a64_monitor.set(Some(owner));
        e.cpu.check(unsafe {
            ffi::uc_embedder_set_a64_exclusive(
                e.cpu.raw,
                Some(exclusive),
                Rc::as_ptr(e).cast_mut().cast(),
            )
        })?;
        Ok(self)
    }
}

// Keep in sync with enum uc_embedder_exclusive_op in Unicorn.
const CLEAR: u32 = 0;
const LOAD: u32 = 1;
const CHECK: u32 = 2;
const WRITTEN: u32 = 3;

unsafe extern "C" fn exclusive(data: *mut c_void, address: u64, size: usize, op: u32) -> bool {
    let e = unsafe { &*data.cast::<Execution>() };
    let mut allowed = true;
    e.guarded(|| {
        let owner = e.a64_monitor.get().ok_or("missing AArch64 monitor owner")?;
        let physical = if op == CLEAR {
            None
        } else {
            // The native engine owns address faults and the host CAS.
            e.memory.canonical(e.id.get(), address, size).ok()
        };
        if op == LOAD && physical.is_some() && e.watch_a64_exclusives.get() {
            e.memory.watch_exclusive(e.id.get(), address, size)?;
        }
        let mut monitor = e
            .memory
            .monitors
            .lock()
            .map_err(|_| "exclusive monitors poisoned")?;
        match (op, physical) {
            (LOAD, Some(physical)) => monitor.load(owner, physical, size),
            (CHECK, Some(physical)) => {
                allowed = monitor.reservation(owner)
                    == Some(exclusive::Reservation {
                        address: physical,
                        size,
                    });
            }
            (WRITTEN, Some(physical)) => monitor.written(Some((physical, size)))?,
            _ => monitor.clear(owner),
        }
        Ok(())
    });
    allowed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support;
    use fujimod_emulator_cpu::memory::Backing;

    #[test]
    fn aliased_peer_store_of_the_same_value_clears_an_aarch64_reservation() {
        let memory = Memory::default();
        let backing = Backing::aligned_zeros(4096, 4096).unwrap();
        let cpu = |code, data| {
            let raw = test_support::library()
                .open(Model::ArmV8A, 1 << 20)
                .unwrap();
            let cpu = Core::new(raw, memory.clone())
                .unwrap()
                .with_board_exclusives(true)
                .unwrap();
            cpu.map_zeroed(code, 4096, 7).unwrap();
            unsafe {
                cpu.map_physical(
                    data,
                    0x4000,
                    4096,
                    3,
                    NonNull::new(backing.as_ptr()).unwrap(),
                )
                .unwrap();
            }
            cpu.write_register(Register::X(1), &data.to_le_bytes())
                .unwrap();
            cpu
        };
        let reader = cpu(0x1000, 0x4000);
        let writer = cpu(0x2000, 0x8000);
        reader
            .write_memory(0x1000, &0x885f_7c20u32.to_le_bytes())
            .unwrap(); // ldxr w0, [x1]
        reader
            .write_memory(0x1004, &0x8802_7c23u32.to_le_bytes())
            .unwrap(); // stxr w2, w3, [x1]
        writer
            .write_memory(0x2000, &0xb900_0020u32.to_le_bytes())
            .unwrap(); // str w0, [x1]
        writer
            .write_memory(0x2004, &0x885f_7c20u32.to_le_bytes())
            .unwrap(); // ldxr w0, [x1]
        writer
            .write_memory(0x2008, &0x8802_7c3fu32.to_le_bytes())
            .unwrap(); // stxr w2, wzr, [x1]
        reader
            .write_register(Register::X(3), &7u64.to_le_bytes())
            .unwrap();
        let instruction = |cpu: &Core, begin| {
            cpu.run(Run {
                begin,
                until: None,
                instructions: Some(1),
                watchdog_us: 0,
            })
            .unwrap();
            assert_eq!(cpu.instruction_progress(), Some(1));
        };
        instruction(&reader, 0x1000);
        instruction(&writer, 0x2000);
        instruction(&reader, 0x1004);
        assert_eq!(reader.execution.scalar(Register::X(2)).unwrap(), 1);
        instruction(&reader, 0x1000);
        instruction(&writer, 0x2004);
        instruction(&writer, 0x2008);
        assert_eq!(writer.execution.scalar(Register::X(2)).unwrap(), 0);
        instruction(&reader, 0x1004);
        assert_eq!(reader.execution.scalar(Register::X(2)).unwrap(), 1);
        instruction(&reader, 0x1000);
        instruction(&reader, 0x1004);
        assert_eq!(reader.execution.scalar(Register::X(2)).unwrap(), 0);
        let mut value = [0; 4];
        writer.read_memory(0x8000, &mut value).unwrap();
        assert_eq!(u32::from_le_bytes(value), 7);
    }
}
