//! Logical exclusive monitors: native debug exits clear the host reservation.
mod checkpoint;
use std::collections::BTreeMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

pub type Shared = Arc<Mutex<Monitors>>;
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Owner(u64);
impl Owner {
    fn allocate() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reservation {
    pub address: u64,
    pub size: usize,
}
#[derive(Clone, Copy)]
pub struct Saved {
    epoch: u128,
    reservation: Option<Reservation>,
}
#[derive(Clone, Default)]
pub struct Monitors {
    epoch: u128,
    /// Bumped whenever any owner's reservation changes. A reader that asks the
    /// same overlap question every block can compare it against the last
    /// answer instead of walking every slot again.
    reservations: u64,
    cores: BTreeMap<Owner, Slot>,
}
#[derive(Clone)]
struct Slot {
    granule: u64,
    reservation: Option<Reservation>,
    // Older board checkpoints stored these processors' exclusives in the CPU.
    legacy_optional: bool,
    invalidates_on_write: bool,
}
impl Monitors {
    pub fn validate_checkpoint(&self, saved: &Self) -> Result<(), String> {
        if self
            .cores
            .iter()
            .map(|(owner, slot)| (*owner, slot.granule))
            .ne(saved
                .cores
                .iter()
                .map(|(owner, slot)| (*owner, slot.granule)))
        {
            return Err("exclusive checkpoint owner set differs".into());
        }
        Ok(())
    }
    /// Whole-board restore also rolls memory back, unlike CPU-only restore.
    pub fn restore_checkpoint(&mut self, saved: &Self) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        // The saved archive carries its own generation, which may collide with
        // one a live reader already cached an answer for. Move past both so a
        // restore always reads as a new reservation state.
        let next = self.reservations.max(saved.reservations).wrapping_add(1);
        self.clone_from(saved);
        self.reservations = next;
        Ok(())
    }
    /// Register once at processor construction, so guest reservations never
    /// insert or remove map entries on the execution path.
    pub fn register(&mut self, granule: u64) -> Result<Owner, String> {
        self.register_slot(granule, false, true)
    }
    /// Register a CPU that previously kept its monitor entirely in the engine.
    /// Serialized engines can retain their native value comparison while the
    /// domain checks the physical reservation and its width.
    pub fn register_native(
        &mut self,
        granule: u64,
        invalidates_on_write: bool,
    ) -> Result<Owner, String> {
        self.register_slot(granule, true, invalidates_on_write)
    }
    fn register_slot(
        &mut self,
        granule: u64,
        legacy_optional: bool,
        invalidates_on_write: bool,
    ) -> Result<Owner, String> {
        if !(4..=2048).contains(&granule) || !granule.is_power_of_two() {
            return Err("invalid exclusive reservation granule".into());
        }
        let owner = Owner::allocate();
        self.cores.insert(
            owner,
            Slot {
                granule,
                reservation: None,
                legacy_optional,
                invalidates_on_write,
            },
        );
        Ok(owner)
    }
    pub fn unregister(&mut self, core: Owner) {
        if self
            .cores
            .remove(&core)
            .is_some_and(|slot| slot.reservation.is_some())
        {
            self.bump_reservations();
        }
    }
    pub fn reservation(&self, core: Owner) -> Option<Reservation> {
        self.cores.get(&core).and_then(|slot| slot.reservation)
    }
    pub fn active(&self) -> bool {
        self.cores.values().any(|slot| slot.reservation.is_some())
    }
    /// Whether a write observer is needed to invalidate a reservation.
    pub fn observes_writes(&self) -> bool {
        self.cores
            .values()
            .any(|slot| slot.invalidates_on_write && slot.reservation.is_some())
    }
    /// Whether an owner other than `core` holds a reservation.
    pub fn active_except(&self, core: Owner) -> bool {
        self.cores
            .iter()
            .any(|(owner, slot)| *owner != core && slot.reservation.is_some())
    }
    /// Whether an owner other than `core` holds a reservation whose granule
    /// overlaps any `[begin, end)` physical span.
    pub fn active_except_overlapping(&self, core: Owner, spans: &[(u64, u64)]) -> bool {
        self.cores.iter().any(|(owner, slot)| {
            if *owner == core || !slot.invalidates_on_write {
                return false;
            }
            let Some(r) = slot.reservation else {
                return false;
            };
            let mask = slot.granule - 1;
            let first = r.address & !mask;
            let last = r.address.saturating_add(r.size.saturating_sub(1) as u64) | mask;
            spans
                .iter()
                .any(|&(begin, end)| begin < end && begin <= last && end > first)
        })
    }
    pub fn load(&mut self, core: Owner, address: u64, size: usize) {
        self.cores
            .get_mut(&core)
            .expect("exclusive owner was not registered")
            .reservation = Some(Reservation { address, size });
        self.bump_reservations();
    }
    fn bump_reservations(&mut self) {
        self.reservations = self.reservations.wrapping_add(1);
    }
    /// Changes whenever any owner's reservation changed.
    pub fn reservations(&self) -> u64 {
        self.reservations
    }
    pub fn store(&mut self, core: Owner, address: u64, size: usize) -> bool {
        let previous = self
            .cores
            .get_mut(&core)
            .and_then(|slot| slot.reservation.take());
        if previous.is_some() {
            self.bump_reservations();
        }
        previous == Some(Reservation { address, size })
    }
    pub fn clear(&mut self, core: Owner) {
        if let Some(slot) = self.cores.get_mut(&core)
            && slot.reservation.take().is_some()
        {
            self.bump_reservations();
        }
    }
    /// Each owner declares its reservation granule. Writes need not change bytes:
    /// ABA stores invalidate just as other conflicting stores do.
    pub fn written(&mut self, range: Option<(u64, usize)>) -> Result<(), String> {
        if range.is_some_and(|(_, bytes)| bytes == 0) {
            return Ok(());
        }
        let range = range
            .map(|(address, size)| {
                address
                    .checked_add(size as u64)
                    .map(|end| (address, end))
                    .ok_or("exclusive write range overflow")
            })
            .transpose()?;
        self.epoch = self
            .epoch
            .checked_add(1)
            .ok_or("exclusive write epoch exhausted")?;
        if let Some((address, end)) = range {
            let mut cleared = false;
            for slot in self.cores.values_mut() {
                if slot.invalidates_on_write
                    && let Some(r) = slot.reservation
                {
                    let mask = slot.granule - 1;
                    let first = r.address & !mask;
                    let last = (r.address + r.size as u64 - 1) | mask;
                    if address <= last && end > first {
                        slot.reservation = None;
                        cleared = true;
                    }
                }
            }
            if cleared {
                self.bump_reservations();
            }
        } else {
            for slot in self.cores.values_mut() {
                if slot.invalidates_on_write {
                    slot.reservation = None;
                }
            }
            self.bump_reservations();
        }
        Ok(())
    }
    pub fn save(&self, core: Owner) -> Saved {
        Saved {
            epoch: self.epoch,
            reservation: self.reservation(core),
        }
    }
    /// A CPU-only restore cannot roll RAM back. Preserve a saved reservation
    /// only if no memory write happened since capture; otherwise fail the SC.
    pub fn restore(&mut self, core: Owner, saved: Saved) {
        self.clear(core);
        if saved.epoch == self.epoch
            && let Some(r) = saved.reservation
        {
            self.cores
                .get_mut(&core)
                .expect("exclusive owner was not registered")
                .reservation = Some(r);
            self.bump_reservations();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn conflicting_and_aba_writes_snapshot_and_granule() {
        let mut m = Monitors::default();
        let a = m.register(64).unwrap();
        let b = m.register(64).unwrap();
        m.load(a, 0x1000, 4);
        m.load(b, 0x2000, 8);
        let saved = m.save(a);
        assert!(m.store(a, 0x1000, 4));
        m.restore(a, saved);
        m.written(Some((0x103f, 1))).unwrap();
        assert!(!m.store(a, 0x1000, 4));
        m.restore(a, saved);
        assert!(!m.store(a, 0x1000, 4));
        assert!(m.store(b, 0x2000, 8));
        m.load(a, 0x1000, 8);
        assert!(!m.store(a, 0x1000, 4));
        assert!(!m.store(a, 0x1000, 8));
    }

    #[test]
    fn active_except_overlapping_checks_granule_and_span_boundaries() {
        let mut m = Monitors::default();
        let a = m.register(64).unwrap();
        let b = m.register(64).unwrap();

        // Owner a reserves granule [0x1000, 0x103f].
        m.load(a, 0x1010, 4);
        // Own reservation is excluded.
        assert!(!m.active_except_overlapping(a, &[(0x1000, 0x2000)]));
        // Disjoint spans before, after, and empty spans inside the granule.
        assert!(!m.active_except_overlapping(b, &[]));
        assert!(!m.active_except_overlapping(b, &[(0x0fc0, 0x1000), (0x1040, 0x2000)]));
        assert!(!m.active_except_overlapping(b, &[(0x1010, 0x1010)]));
        // Overlapping at the first and last byte of the granule.
        assert!(m.active_except_overlapping(b, &[(0x0fc0, 0x1001)]));
        assert!(m.active_except_overlapping(b, &[(0x103f, 0x1080)]));
        assert!(m.active_except_overlapping(b, &[(0x2000, 0x3000), (0x1000, 0x1040)]));

        m.clear(a);
        assert!(!m.active_except_overlapping(b, &[(0x1000, 0x2000)]));
    }

    /// A cached overlap answer is only valid while the generation is unchanged,
    /// so every mutation of a reservation, including a conflicting write and a
    /// restore, has to move it.
    #[test]
    fn every_reservation_change_moves_the_generation() {
        let mut m = Monitors::default();
        let a = m.register(64).unwrap();
        let b = m.register(64).unwrap();
        let start = m.reservations();

        m.load(a, 0x1000, 4);
        let loaded = m.reservations();
        assert_ne!(loaded, start);

        // A store with no reservation to clear changes nothing observable.
        assert!(!m.store(b, 0x2000, 8));
        assert_eq!(m.reservations(), loaded);

        // A conflicting write clears a's reservation, which must be visible
        // to a reader that cached "no overlap" a moment earlier.
        m.written(Some((0x103f, 1))).unwrap();
        let written = m.reservations();
        assert_ne!(written, loaded);

        // A write that clears nothing observable leaves the answer alone.
        m.written(Some((0x8000, 4))).unwrap();
        assert_eq!(m.reservations(), written);

        m.load(a, 0x1000, 4);
        let reloaded = m.reservations();
        assert_ne!(reloaded, written);
        m.clear(a);
        let cleared = m.reservations();
        assert_ne!(cleared, reloaded);
        // Clearing an idle owner is not a change.
        m.clear(a);
        assert_eq!(m.reservations(), cleared);

        // A restore may install any archive, so it always lands past the
        // generation a reader could already have cached.
        m.load(b, 0x2000, 8);
        let saved = m.clone();
        let cacheable = m.reservations();
        m.restore_checkpoint(&saved).unwrap();
        assert_ne!(m.reservations(), cacheable);

        // The CPU-only path restores a single reservation, too.
        let reservation = m.save(b);
        m.clear(b);
        let before_cpu_restore = m.reservations();
        m.restore(b, reservation);
        assert_ne!(m.reservations(), before_cpu_restore);
    }
}
