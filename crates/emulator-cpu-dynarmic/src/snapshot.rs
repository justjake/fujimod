//! Architectural state of a stopped Dynarmic processor on an exact board.
use super::{Core, ffi, rprofile, status};
use fujimod_emulator_cpu::{Identity, Snapshot, armv7a};
use serde::{Deserialize, Serialize};
use std::any::Any;

const FORMAT: &[u8] = b"dynarmic-a32-v1\0";
const CP15_WORDS: usize = 1 << 14;

pub(super) struct Saved {
    owner: Identity,
    state: State,
}

#[derive(Serialize, Deserialize)]
struct State {
    regs: [u32; 16],
    vectors: [[u32; 4]; 16],
    cpsr: u32,
    fpscr: u32,
    banks: status::Banks,
    fpexc: u32,
    cp15: Vec<(u16, u32)>,
    cache_ids: [u32; 3],
    irq: bool,
    exception_entries: [u64; 4],
    rprofile: Option<rprofile::Saved>,
}

impl Saved {
    pub(super) fn capture(core: &Core) -> Result<Box<dyn Snapshot>, String> {
        core.check_snapshot_boundary()?;
        let mut vectors = [[0; 4]; 16];
        let mut cp15 = vec![0; CP15_WORDS];
        let mut cache_ids = [0; 3];
        unsafe {
            ffi::dynarmic_read_snapshot(
                core.inst,
                vectors.as_mut_ptr().cast(),
                cp15.as_mut_ptr(),
                cache_ids.as_mut_ptr(),
            );
        }
        let entries = core.exception_entries.get();
        Ok(Box::new(Self {
            owner: core.identity,
            state: State {
                regs: std::array::from_fn(|i| unsafe {
                    ffi::dynarmic_get_reg(core.inst, i as u32)
                }),
                vectors,
                cpsr: unsafe { ffi::dynarmic_get_cpsr(core.inst) },
                fpscr: unsafe { ffi::dynarmic_get_fpscr(core.inst) },
                banks: core.banks.borrow().clone(),
                fpexc: core.fpexc.get(),
                cp15: cp15
                    .iter()
                    .enumerate()
                    .filter_map(|(slot, &value)| (value != 0).then_some((slot as u16, value)))
                    .collect(),
                cache_ids,
                irq: core.irq.get(),
                rprofile: core
                    .shared
                    .rprofile
                    .as_ref()
                    .map(rprofile::Profile::capture),
                exception_entries: [
                    entries.irq,
                    entries.undefined,
                    entries.data_abort,
                    entries.prefetch_abort,
                ],
            },
        }))
    }

    pub(super) fn import(core: &Core, bytes: &[u8]) -> Result<Box<dyn Snapshot>, String> {
        core.check_snapshot_boundary()?;
        let bytes = bytes
            .strip_prefix(FORMAT)
            .ok_or("expected a Dynarmic A32 snapshot")?;
        let (state, rest): (State, _) =
            postcard::take_from_bytes(bytes).map_err(|error| error.to_string())?;
        status::bank(state.cpsr)?;
        if !rest.is_empty()
            || state.fpexc & !0x4000_0000 != 0
            || !state
                .cp15
                .iter()
                .all(|&(slot, value)| usize::from(slot) < CP15_WORDS && value != 0)
            || !state.cp15.windows(2).all(|pair| pair[0].0 < pair[1].0)
        {
            return Err("invalid Dynarmic A32 snapshot state".into());
        }
        match (&core.shared.rprofile, &state.rprofile) {
            (Some(profile), Some(saved)) => {
                profile.validate(saved, |address, bytes| core.shared.has_mmio(address, bytes))?
            }
            (None, None) => (),
            _ => return Err("Dynarmic snapshot processor profile differs".into()),
        }
        Ok(Box::new(Self {
            owner: core.identity,
            state,
        }))
    }

    pub(super) fn restore(core: &Core, snapshot: &dyn Snapshot) -> Result<(), String> {
        core.check_snapshot_boundary()?;
        if snapshot.owner() != core.identity {
            return Err("Dynarmic CPU snapshot belongs to another processor".into());
        }
        let state = &snapshot
            .as_any()
            .downcast_ref::<Self>()
            .ok_or("invalid Dynarmic snapshot type")?
            .state;
        if let (Some(current), Some(saved)) = (&core.shared.rprofile, &state.rprofile) {
            current.validate(saved, |address, bytes| core.shared.has_mmio(address, bytes))?;
            current.restore(saved);
        }
        *core.banks.borrow_mut() = state.banks.clone();
        core.fpexc.set(state.fpexc);
        core.irq.set(state.irq);
        let [irq, undefined, data_abort, prefetch_abort] = state.exception_entries;
        core.exception_entries.set(armv7a::Entries {
            irq,
            undefined,
            data_abort,
            prefetch_abort,
        });
        let mut cp15 = vec![0; CP15_WORDS];
        for &(slot, value) in &state.cp15 {
            cp15[usize::from(slot)] = value;
        }
        unsafe {
            for (i, &value) in state.regs.iter().enumerate() {
                ffi::dynarmic_set_reg(core.inst, i as u32, value);
            }
            ffi::dynarmic_set_cpsr(core.inst, state.cpsr);
            ffi::dynarmic_set_fpscr(core.inst, state.fpscr);
            ffi::dynarmic_write_snapshot(
                core.inst,
                state.vectors.as_ptr().cast(),
                cp15.as_ptr(),
                state.cache_ids.as_ptr(),
            );
        }
        core.shared.mmu_context.set(None);
        core.shared.mapping_changed();
        Ok(())
    }
}

impl Snapshot for Saved {
    fn owner(&self) -> Identity {
        self.owner
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn export(&self) -> Result<Vec<u8>, String> {
        postcard::to_extend(&self.state, FORMAT.to_vec()).map_err(|error| error.to_string())
    }
}

impl Core {
    fn check_snapshot_boundary(&self) -> Result<(), String> {
        if self.run_limit.get().is_some() || !self.board_owns_instructions.get() {
            return Err(
                "Dynarmic snapshots require a stopped board with board-owned exclusives".into(),
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_cpu::{Cpu, Model, arm};

    #[test]
    fn portable_state_restores_system_registers_vectors_and_banks() {
        let source = Core::new(Model::CortexA7).unwrap();
        let target = Core::new(Model::CortexA7).unwrap();
        for cpu in [&source, &target] {
            cpu.set_armv7a_owner_exclusives(true).unwrap();
        }
        let saved = source.snapshot(None).unwrap();
        let mut state =
            postcard::from_bytes::<State>(saved.export().unwrap().strip_prefix(FORMAT).unwrap())
                .unwrap();
        let ttbr0 = arm::Coprocessor {
            cp: 15,
            opc1: 0,
            crn: 2,
            crm: 0,
            opc2: 0,
            secure: 0,
            wide: false,
        };
        state.vectors[15] = [3, 4, 5, 6];
        let fixture = Saved {
            owner: source.identity,
            state,
        };
        source.restore(&fixture).unwrap();
        source.coprocessor(ttbr0, Some(0x4000)).unwrap();
        source.set_cpsr(0xd2).unwrap(); // IRQ has its own stack and saved status.
        source
            .write_register(arm::CORE[13], &0x2000u32.to_le_bytes())
            .unwrap();
        source.set_spsr(0x1f).unwrap();
        source.set_cpsr(0xd3).unwrap();

        let bytes = source.snapshot(None).unwrap().export().unwrap();
        let encoded = postcard::from_bytes::<State>(bytes.strip_prefix(FORMAT).unwrap()).unwrap();
        assert_eq!(encoded.vectors[15], [3, 4, 5, 6]);
        let imported = target.import_snapshot(&bytes).unwrap();
        assert_eq!(target.coprocessor(ttbr0, None).unwrap(), 0);
        target.restore(imported.as_ref()).unwrap();
        assert_eq!(target.coprocessor(ttbr0, None).unwrap(), 0x4000);
        assert_eq!(target.snapshot(None).unwrap().export().unwrap(), bytes);
        target.set_cpsr(0xd2).unwrap();
        let mut stack = [0; 4];
        target.read_register(arm::CORE[13], &mut stack).unwrap();
        assert_eq!(u32::from_le_bytes(stack), 0x2000);
        assert_eq!(target.spsr().unwrap(), 0x1f);
        assert!(target.import_snapshot(&bytes[..bytes.len() - 1]).is_err());
    }
}
