//! Discovery callee substitutions: explicit execution experiments, never
//! selected by a device or counted as a boot.
use super::{Board, bus::Mode};
use fujimod_emulator_cpu::{Cpu, Hook, HookRange, Model, Register, arm};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    ffi::c_void,
    rc::{Rc, Weak},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

/// A caller-requested routine substitution. It is only valid on a discovery
/// board, and its address/value remain visible in every board-state report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalleeStub {
    pub name: String,
    pub core: usize,
    pub address: u64,
    pub value: u64,
}

pub(super) struct CalleeStubContext {
    pub(super) spec: CalleeStub,
    cpus: Weak<Vec<Rc<dyn Cpu>>>,
    hits: AtomicU64,
    pub(super) failure: Mutex<Option<String>>,
}

impl CalleeStubContext {
    fn record_failure(&self, error: impl Into<String>) {
        if let Ok(mut failure) = self.failure.lock() {
            failure.get_or_insert_with(|| error.into());
        }
    }

    pub(super) fn state(&self) -> Value {
        json!({
            "kind": "callee_stub",
            "name": self.spec.name,
            "core": self.spec.core,
            "address": format!("{:#x}", self.spec.address),
            "value": self.spec.value,
            "hits": self.hits.load(Ordering::Relaxed),
            "failure": self.failure.lock().ok().and_then(|failure| failure.clone()),
        })
    }
}

unsafe fn substitute_callee(address: u64, _: u32, data: *mut c_void) {
    let context = unsafe { &*data.cast::<CalleeStubContext>() };
    if address != context.spec.address {
        return;
    }
    let Some(cpus) = context.cpus.upgrade() else {
        context.record_failure("processor was destroyed before discovery substitution");
        return;
    };
    let Some(cpu) = cpus.get(context.spec.core) else {
        context.record_failure("discovery substitution names an absent processor");
        return;
    };
    let (result, return_address, width) = if cpu.model() == Model::ArmV8A {
        (Register::X(0), Register::X(30), 8)
    } else {
        (
            Register::Arm32(arm::Register::R(0)),
            Register::Arm32(arm::Register::R(14)),
            4,
        )
    };
    let mut link = [0; 8];
    let redirect = (|| {
        if cpu.read_register(return_address, &mut link[..width])? != width {
            return Err("callee return address width differs".into());
        }
        cpu.write_register(result, &context.spec.value.to_le_bytes()[..width])?;
        cpu.write_register(Register::Pc, &link[..width])
    })();
    if let Err(error) = redirect {
        context.record_failure(error);
        let _ = cpu.stop();
        return;
    }
    context.hits.fetch_add(1, Ordering::Relaxed);
}

impl Board {
    /// Install explicit callee substitutions on a discovery board.
    /// The instruction hook redirects to the caller's return address; it does
    /// not inspect instruction bytes or infer a routine from program state.
    pub fn install_discovery_callee_stubs(&mut self, stubs: &[CalleeStub]) -> Result<(), String> {
        if stubs.is_empty() {
            return Ok(());
        }
        if self.state.borrow().bus.mode() != Mode::Discovery {
            return Err("callee substitutions require explicit discovery execution".into());
        }
        if stubs.len() + self.discovery_stub_contexts.len() > 4096
            || stubs.iter().any(|stub| {
                let Some(cpu) = self.cpus.get(stub.core) else {
                    return true;
                };
                match cpu.model() {
                    Model::ArmV8A => stub.address & 3 != 0,
                    Model::CortexR5F | Model::CortexA7 => {
                        stub.address & 1 != 0
                            || u32::try_from(stub.address).is_err()
                            || u32::try_from(stub.value).is_err()
                    }
                    Model::CortexM4 => true,
                }
            })
        {
            return Err("discovery callee substitution address or value does not fit the selected processor".into());
        }
        let mut unique = BTreeSet::new();
        for context in &self.discovery_stub_contexts {
            unique.insert((context.spec.core, context.spec.address));
        }
        if stubs
            .iter()
            .any(|stub| !unique.insert((stub.core, stub.address)))
        {
            return Err("duplicate discovery callee substitution".into());
        }

        let original_hooks = self.hooks.len();
        let original_contexts = self.discovery_stub_contexts.len();
        for spec in stubs {
            self.discovery_stub_contexts
                .push(Box::new(CalleeStubContext {
                    spec: spec.clone(),
                    cpus: Rc::downgrade(&self.cpus),
                    hits: AtomicU64::new(0),
                    failure: Mutex::new(None),
                }));
            let context = &mut **self.discovery_stub_contexts.last_mut().unwrap();
            let data = (context as *mut CalleeStubContext).cast();
            match unsafe {
                self.cpus[spec.core].install_hook(Hook::Instruction {
                    callback: substitute_callee,
                    data,
                    range: HookRange::Inclusive {
                        begin: spec.address,
                        end: spec.address,
                    },
                })
            } {
                Ok(hook) => self.hooks.push((spec.core, hook)),
                Err(error) => {
                    for (core, hook) in self.hooks.drain(original_hooks..) {
                        let _ = self.cpus[core].remove_hook(hook);
                    }
                    self.discovery_stub_contexts.truncate(original_contexts);
                    return Err(format!("install discovery callee substitution: {error}"));
                }
            }
        }
        Ok(())
    }
}
