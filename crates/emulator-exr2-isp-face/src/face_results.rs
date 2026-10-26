//! Explicit external detector results; no face classifier or software records.
//!
//! The result bank at 0xfff14600 exposes results. START at
//! offset 4 admits an aligned detector input bus address and one IRQ47 owner.
//! The declared 400,000-tick latency is unmeasured. Completion retains that
//! endpoint even if the interrupt controller masks it after admission.
use fujimod_emulator_devices::exr2::irq_routes::Irq;
use fujimod_emulator_devices::exr2::irq_routes::IrqRoutes;
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};
use fujimod_emulator_imaging::exr2_isp::options::face_results::Profile;

pub const BASE: u64 = crate::face_detector::BASE;
pub const LATENCY: u64 = 400_000;
const HISTORY: usize = 64;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Request {
    tick: u64,
    input: u32,
    endpoint: Irq,
    completed: Option<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct State {
    profile: Profile,
    registers: BTreeMap<u16, u32>,
    due: Option<u64>,
    requested: u64,
    produced: u64,
    reads: u64,
    available: bool,
    recent: VecDeque<Request>,
}

pub struct FaceResults {
    profile: Profile,
    routes: IrqRoutes,
    state: State,
}
impl FaceResults {
    pub fn new(profile: Profile, routes: IrqRoutes) -> Self {
        Self {
            profile,
            routes,
            state: State {
                profile,
                registers: BTreeMap::new(),
                due: None,
                requested: 0,
                produced: 0,
                reads: 0,
                available: false,
                recent: VecDeque::new(),
            },
        }
    }
    fn offset(cx: &mut Cx<'_>, address: u64, size: u8) -> Option<u16> {
        if size == 4 && address & 3 == 0 && (BASE..BASE + 0x1000).contains(&address) {
            Some((address - BASE) as u16)
        } else {
            cx.unmodeled();
            None
        }
    }
    fn start(&mut self, cx: &Cx<'_>) -> Result<(), String> {
        if cx.cpu != 0 || self.state.due.is_some() {
            return Err("declared face result requires a nonoverlapping CPU0 START".into());
        }
        let input = self.state.registers.get(&0x48).copied().unwrap_or(0);
        if !(0x40000000..0x60000000).contains(&input) || input & 3 != 0 {
            return Err("declared face result requires an aligned bounded detector input".into());
        }
        let endpoint = self.routes.latch(47)?;
        let due = cx
            .now()
            .checked_add(LATENCY)
            .ok_or("face result deadline overflow")?;
        let requested = self
            .state
            .requested
            .checked_add(1)
            .ok_or("face request counter overflow")?;
        self.state.due = Some(due);
        self.state.requested = requested;
        self.state.available = false;
        if self.state.recent.len() == HISTORY {
            self.state.recent.pop_front();
        }
        self.state.recent.push_back(Request {
            tick: cx.now(),
            input,
            endpoint,
            completed: None,
        });
        Ok(())
    }
    fn valid(&self, state: &State) -> bool {
        state.profile == self.profile
            && state.produced <= state.requested
            && state.recent.len() == (state.requested.min(HISTORY as u64) as usize)
            && state
                .registers
                .keys()
                .all(|offset| *offset < 0x1000 && offset & 3 == 0)
            && state.recent.iter().all(|r| {
                (0x40000000..0x60000000).contains(&r.input)
                    && r.input & 3 == 0
                    && r.endpoint.core < 2
                    && r.endpoint.line == 47
                    && r.completed
                        .is_none_or(|tick| tick >= r.tick.saturating_add(LATENCY))
            })
            && state.due.is_none_or(|due| {
                !state.available
                    && state.recent.back().is_some_and(|r| {
                        r.tick.checked_add(LATENCY) == Some(due) && r.completed.is_none()
                    })
            })
    }
}
impl Device for FaceResults {
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        fujimod_emulator_devices::checkpoint::Checkpoint::encode(self.name(), &self.state)
    }
    fn validate_checkpoint(
        &self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        let state: State = saved.decode(self.name())?;
        if !self.valid(&state) {
            return Err("invalid declared face result checkpoint".into());
        }
        Ok(())
    }
    fn restore_checkpoint(
        &mut self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        self.state = saved.decode(self.name())?;
        Ok(())
    }
    fn name(&self) -> &'static str {
        "exr2-declared-face-results"
    }
    fn fingerprint(&self) -> Result<Value, String> {
        Ok(self.save())
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let Some(offset) = Self::offset(cx, address, size) else {
            return 0;
        };
        if let Some(output) = self.profile.output(offset) {
            self.state.reads = self.state.reads.saturating_add(1);
            return if self.state.available {
                u64::from(output)
            } else {
                0
            };
        }
        u64::from(self.state.registers.get(&offset).copied().unwrap_or(0))
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Some(offset) = Self::offset(cx, address, size) else {
            return;
        };
        let Ok(value) = u32::try_from(value) else {
            cx.fail("face result word exceeds 32 bits");
            return;
        };
        if offset == 4 && value == 1 {
            if let Err(error) = self.start(cx) {
                cx.fail(error);
                return;
            }
        } else if offset == 0 && value <= 1 && self.state.due.is_some() {
            self.state.due = None;
            self.state.available = false;
        }
        self.state.registers.insert(offset, value);
    }
    fn next_deadline(&self, _now: u64) -> Option<u64> {
        self.state.due
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if self.state.due.is_none_or(|due| cx.now() < due) {
            return;
        }
        let request = self.state.recent.back_mut().expect("pending face request");
        // Masking controls CPU delivery, not completion or the pending latch.
        if let Err(error) = request.endpoint.raise(cx) {
            cx.fail(error);
            return;
        }
        request.completed = Some(cx.now());
        self.state.due = None;
        self.state.available = true;
        self.state.produced += 1;
    }
    fn save(&self) -> Value {
        serde_json::to_value(&self.state).expect("face results serialize")
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value.clone())
            .map_err(|e| format!("invalid face result state: {e}"))?;
        if !self.valid(&state) {
            return Err("invalid declared face result state".into());
        }
        self.state = state;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
