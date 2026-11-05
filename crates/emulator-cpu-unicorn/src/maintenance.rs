//! Accepted guest translation maintenance for a serialized Cortex-A7 owner.
//!
//! This interface records local completion and provides a local peer flush.
//! The board owner is responsible for domain membership, recipient selection,
//! delivery, failure handling, and acknowledgment. Attachment alone does not
//! implement cross-core delivery.
use crate::{Core, ffi};
use fujimod_emulator_cpu::{Identity, Model, arm};
use std::sync::atomic::{AtomicU64, Ordering};

pub use crate::ffi::{TlbiEvent as Event, TlbiInfo as Status};
pub const ABI: u32 = 1;
pub const CAPACITY: u32 = 64;
pub const TLBIALLIS: u32 = 0x0f08_3000;

/// Unique capability for one attached engine. Not transferable to another CPU.
pub struct Owner {
    cpu: Identity,
    token: u64,
}

pub(crate) fn shared_selector(cp: arm::Coprocessor) -> bool {
    cp.cp == 15 && !cp.wide && cp.crn == 8 && match (cp.crm, cp.opc1, cp.opc2) {
        (3, 0, 0 | 1 | 2 | 3 | 5 | 7) => true,
        (3, 4, 0 | 1 | 4 | 5) => true,
        (0, 4, 1 | 5) => true,
        _ => false,
    }
}

impl Core {
    /// Attach before the engine's first execution. There is no detach/reset.
    ///
    /// # Safety
    /// The caller must serialize all participating engines, keep this Core
    /// private, reject reentrant public mutations, settle accepted events before
    /// executing peers, and acknowledge only after complete delivery. Private
    /// architectural callbacks may still write registers during execution.
    pub unsafe fn attach_translation_owner(&self) -> Result<Owner, String> {
        if self.model != Model::CortexA7 || self.tlbi_owner.get().is_some() {
            return Err("Translation owner requires a fresh, unattached Cortex-A7".into());
        }
        if self.api.extensions.features & ffi::FEATURE_TLBI_OWNER == 0
            || self.api.extensions.tlbi_status.is_none()
            || self.api.extensions.tlbi_ack.is_none()
        {
            return Err("CPU backend lacks the accepted-TLBI owner ABI".into());
        }
        let attach = self.api.extensions.tlbi_attach.ok_or("Missing TLBI attachment ABI")?;
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let token = NEXT.fetch_update(Ordering::Relaxed, Ordering::Relaxed,
            |value| value.checked_add(1)).map_err(|_| "TLBI owner identity exhausted")?;
        self.check(unsafe { attach(self.raw, ABI, token) })?;
        self.tlbi_owner.set(Some(token));
        Ok(Owner { cpu: self.identity, token })
    }

    pub fn translation_maintenance_status(&self) -> Result<Status, String> {
        let get = self.api.extensions.tlbi_status.ok_or("Missing TLBI status ABI")?;
        let mut status = Status::default();
        self.check(unsafe { get(self.raw, ABI, &mut status) })?;
        Ok(status)
    }

    fn check_translation_owner(&self, owner: &Owner) -> Result<(), String> {
        if owner.cpu != self.identity || self.tlbi_owner.get() != Some(owner.token) {
            return Err("Translation owner belongs to a different processor".into());
        }
        Ok(())
    }

    /// Acknowledge the exact front record after all selected peers were handled.
    pub fn acknowledge_translation_maintenance(&self, owner: &Owner, event: &Event) -> Result<(), String> {
        self.check_translation_owner(owner)?;
        let status = self.translation_maintenance_status()?;
        if status.pending == 0 || status.poison != 0 || status.front != *event {
            return Err("Translation acknowledgment is stale, incomplete, or poisoned".into());
        }
        let ack = self.api.extensions.tlbi_ack.ok_or("Missing TLBI acknowledgment ABI")?;
        self.check(unsafe { ack(self.raw, ABI, owner.token, event.sequence) })
    }

    /// Flush this stopped engine's modeled translations, including all MMU
    /// indexes. This does not perform a translation-block-only code flush.
    pub fn flush_local_translations(&self, owner: &Owner) -> Result<(), String> {
        self.check_translation_owner(owner)?;
        self.check_tlbi_ready()?;
        self.check(unsafe { (self.api.uc_ctl)(self.raw, ffi::ctl(11, 0, 1)) })
    }

    pub(crate) fn check_tlbi_ready(&self) -> Result<(), String> {
        if self.tlbi_owner.get().is_some() {
            let status = self.translation_maintenance_status()?;
            if status.pending != 0 || status.poison != 0 {
                return Err(format!("Translation owner has {} pending events, poison {}",
                    status.pending, status.poison));
            }
        }
        Ok(())
    }

    pub(crate) fn check_tlbi_mutation(&self) -> Result<(), String> {
        // Execution-owned architectural callbacks retain their authority. The
        // unsafe attachment contract requires the owner to keep public host
        // mutation out of this interval.
        if !self.tlbi_executing.get() {
            self.check_tlbi_ready()?;
        }
        Ok(())
    }
}

pub(crate) struct RunGuard<'a>(&'a Core);
impl<'a> RunGuard<'a> {
    pub fn enter(core: &'a Core) -> Result<Self, String> {
        if core.tlbi_owner.get().is_some() && core.tlbi_executing.replace(true) {
            return Err("Reentrant owned Cortex-A7 execution".into());
        }
        Ok(Self(core))
    }
}
impl Drop for RunGuard<'_> {
    fn drop(&mut self) {
        self.0.tlbi_executing.set(false);
    }
}
