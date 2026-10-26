//! A round-robin phase survives host pauses and unconstrained lone-core runs.
#[derive(Clone, Copy)]
pub(super) struct Slice {
    pub core: usize,
    end: u64,
    quantum: u64,
}
impl Slice {
    pub fn new(core: usize, entered: u64, left: usize, quantum: usize) -> Result<Self, String> {
        if left == 0 || left > quantum {
            return Err("invalid processor slice phase".into());
        }
        Ok(Self {
            core,
            end: entered
                .checked_add(left as u64)
                .ok_or("processor slice boundary overflow")?,
            quantum: quantum as u64,
        })
    }
    /// Periodic boundaries need not stop a lone runnable core. The next
    /// boundary is still deterministic when a peer becomes runnable later.
    pub fn room(self, entered: u64) -> u64 {
        if entered <= self.end {
            return self.end - entered;
        }
        let phase = (entered - self.end) % self.quantum;
        if phase == 0 { 0 } else { self.quantum - phase }
    }
}
