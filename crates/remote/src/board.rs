//! In-process board transport. Execution yields between bounded, watched windows.
use crate::{backend::Backend, connection::Connection, *};
use fujimod_emulator_session::{
    Session,
    checkpoint::Snapshot as Checkpoint,
    contact_driver::{Actuator, Pose},
};

/// Access is scoped to stopped-board calls, so language owners can retain a board.
pub trait SessionOwner {
    fn with_session<T>(&mut self, call: impl FnOnce(&mut Session) -> Result<T>) -> Result<T>;
}
impl SessionOwner for &mut Session {
    fn with_session<T>(&mut self, call: impl FnOnce(&mut Session) -> Result<T>) -> Result<T> {
        call(self)
    }
}
impl SessionOwner for Session {
    fn with_session<T>(&mut self, call: impl FnOnce(&mut Session) -> Result<T>) -> Result<T> {
        call(self)
    }
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Options {
    pub quantum: usize,
    pub window_instructions: u64,
    pub scanout_node: String,
    pub output: String,
    /// Device observation and JSON pointer naming the completed frame counter.
    pub frame_counter_kind: String,
    pub frame_counter_pointer: String,
}
pub struct BoardBackend<S> {
    owner: S,
    options: Options,
    now: u64,
    /// Last position of each finger down, which its release reports.
    touches: std::collections::BTreeMap<String, (u16, u16)>,
}
pub type BoardRemote<S> = Connection<BoardBackend<S>>;
impl<S: SessionOwner> Connection<BoardBackend<S>> {
    pub fn new(mut owner: S, facts: Facts, options: Options) -> Result<Self> {
        if options.quantum == 0 || options.window_instructions == 0 || options.output.is_empty() {
            return Err("Invalid board continuation options".into());
        }
        let now = owner.with_session(|s| Ok(s.board.now()))?;
        Self::connect(
            BoardBackend {
                owner,
                options,
                now,
                touches: Default::default(),
            },
            facts,
        )
    }
    pub fn into_session(self) -> S {
        self.backend.owner
    }
    pub fn checkpoint(&mut self, previous: Option<&Checkpoint>) -> Result<Checkpoint> {
        self.backend.owner.with_session(|s| s.checkpoint(previous))
    }
    pub fn restore(&mut self, saved: &Checkpoint) -> Result<()> {
        self.backend.owner.with_session(|s| s.restore(saved))?;
        self.backend.now = saved.now();
        // A restored panel holds no remote finger.
        self.backend.touches.clear();
        self.touching.clear();
        Ok(())
    }
    pub fn set_scene(&mut self, scene: &serde_json::Value) -> Result<()> {
        self.backend
            .owner
            .with_session(|s| s.board.set_scene(scene))
    }
    pub async fn advance_until(&mut self, deadline: u64) -> Result<()> {
        while self.now() < deadline {
            self.wait(&ReadSet::default(), deadline).await?;
        }
        Ok(())
    }
}
/// The driver actuator a control's press drives: its own contact, or the
/// press member of a dial or ring whose motion has separate contacts.
fn actuator<'a>(actuators: &'a [Actuator], id: &str) -> Result<&'a Actuator> {
    actuators
        .iter()
        .find(|a| a.id == id)
        .or_else(|| {
            let member = format!("{id}.press");
            actuators.iter().find(|a| a.id == member)
        })
        .ok_or_else(|| format!("No physical actuator for {id}"))
}
/// The pose an input selects on its actuator.
fn pose<'a>(actuator: &'a Actuator, state: &InputState) -> Option<&'a Pose> {
    match state {
        InputState::Position(position) => actuator.states.iter().find(|p| &p.position == position),
        InputState::Stage(stage) => actuator
            .states
            .iter()
            .find(|p| p.position == stage.trim_end_matches("_press")),
        _ => actuator
            .states
            .get(usize::from(*state == InputState::Pressed)),
    }
}
/// Drive an actuator to a pose once the contact queue is idle.
fn drive(s: &mut Session, id: &str, state: &InputState) -> Result<()> {
    let driver = s
        .contact_driver
        .as_ref()
        .ok_or("BoardRemote requires checked contact acknowledgments")?;
    if driver.busy() {
        return Err("Board contact queue is busy".into());
    }
    let actuator = actuator(&driver.spec.actuators, id)?;
    let pose = pose(actuator, state).ok_or("Physical input pose is absent")?;
    let (actuator, position) = (actuator.id.clone(), pose.position.clone());
    s.set_position(&actuator, &position)
}
/// Whether the stock scanner shows the actuator's pose for an input.
fn pose_accepted(s: &Session, id: &str, state: &InputState) -> Result<bool> {
    let driver = s
        .contact_driver
        .as_ref()
        .ok_or("Physical acknowledgment driver is absent")?;
    // input() resolved aliases; the live panel position identifies the pose.
    let actuator = actuator(&driver.spec.actuators, id)?;
    let pose = pose(actuator, state).ok_or("Physical acknowledgment pose is absent")?;
    for check in pose.ready.iter().chain(&pose.expected) {
        let address = if let Some(pointer) = check.pointer {
            let mut raw = [0; 4];
            s.board.read_physical(pointer, &mut raw)?;
            u64::from(u32::from_le_bytes(raw))
                .checked_add(check.address)
                .ok_or("Acknowledgment address overflow")?
        } else {
            check.address
        };
        let mut raw = vec![0; check.bytes.len()];
        s.board.read_physical(address, &mut raw)?;
        if raw != check.bytes {
            return Ok(false);
        }
    }
    Ok(true)
}
fn edge(pressed: bool) -> InputState {
    if pressed {
        InputState::Pressed
    } else {
        InputState::Released
    }
}
fn tracked<T>(
    session: &Session,
    reads: &mut ReadSet,
    call: impl FnOnce(&Session) -> Result<T>,
) -> Result<T> {
    session.board.read_set_begin()?;
    let result = call(session);
    let tracked = session.board.read_set_end()?;
    reads.extend(ReadSet {
        stores: tracked.stores,
        devices: tracked.devices,
    });
    result
}
impl<S: SessionOwner> Backend for BoardBackend<S> {
    fn now(&self) -> u64 {
        self.now
    }
    fn memory<'a>(
        &'a mut self,
        address: u64,
        size: usize,
        reads: &'a mut ReadSet,
    ) -> LocalFuture<'a, Vec<u8>> {
        Box::pin(async move {
            self.now = self.owner.with_session(|s| Ok(s.board.now()))?;
            self.owner.with_session(|s| {
                tracked(s, reads, |s| {
                    let mut bytes = vec![0; size];
                    s.board.read_physical(address, &mut bytes)?;
                    Ok(bytes)
                })
            })
        })
    }
    fn input<'a>(&'a mut self, input: &'a Input, facts: &'a Facts) -> LocalFuture<'a, ()> {
        Box::pin(async move {
            let id = input.control.id();
            facts
                .physical_controls
                .action(&facts.control_ids, id, input.state.operation())?;
            if input.state.is_touch() {
                let (phase, x, y) = match input.state {
                    InputState::TouchStart { x, y } => ("press", x, y),
                    InputState::TouchMove { x, y } => ("drag", x, y),
                    _ => {
                        let (x, y) = self
                            .touches
                            .get(id)
                            .ok_or_else(|| format!("No finger is down on {id}"))?;
                        ("release", u32::from(*x), u32::from(*y))
                    }
                };
                let point = (
                    u16::try_from(x).map_err(|_| "Touch x exceeds 16 bits")?,
                    u16::try_from(y).map_err(|_| "Touch y exceeds 16 bits")?,
                );
                self.owner.with_session(|s| {
                    let (node, script) = s.touch_input(id, phase, point.0, point.1)?;
                    s.touch(&node, &script)
                })?;
                if input.state == InputState::TouchEnd {
                    self.touches.remove(id);
                } else {
                    self.touches.insert(id.into(), point);
                }
                return Ok(());
            }
            self.owner.with_session(|s| match &input.state {
                InputState::Turn(steps) => {
                    let (node, cycles) = s.rotation(id, *steps)?;
                    s.rotate(&node, cycles)
                }
                _ => drive(s, id, &input.state),
            })
        })
    }
    fn accepted(&mut self, input: &Input, facts: &Facts, reads: &mut ReadSet) -> Result<bool> {
        if matches!(input.state, InputState::Turn(_)) {
            return Ok(true);
        }
        if input.state.is_touch() {
            // The firmware's touch task has decoded a report showing this input.
            let layout = facts
                .physical_controls
                .touch(&facts.control_ids, input.control.id())?
                .decoded;
            return self.owner.with_session(|s| {
                tracked(s, reads, |s| {
                    let mut report = vec![0; crate::memory::touch_report_size(layout)];
                    s.board.read_physical(layout.address, &mut report)?;
                    crate::memory::touch_decoded(layout, &report, &input.state)
                })
            });
        }
        self.owner.with_session(|s| {
            tracked(s, reads, |s| {
                pose_accepted(s, input.control.id(), &input.state)
            })
        })
    }
    fn frame<'a>(
        &'a mut self,
        facts: &'a Facts,
        reads: &'a mut ReadSet,
    ) -> LocalFuture<'a, Capture> {
        Box::pin(async move {
            let options = &self.options;
            self.owner
                .with_session(|s| {
                    let mut completion_reads = ReadSet::default();
                    let sequence = tracked(s, &mut completion_reads, |s| {
                        let counters = s.board.device_states(&options.frame_counter_kind)?;
                        counters
                            .pointer(&options.frame_counter_pointer)
                            .and_then(serde_json::Value::as_u64)
                            .ok_or_else(|| "Completed frame counter is unavailable".to_string())
                    })?;
                    reads.extend(completion_reads.clone());
                    tracked(s, reads, |s| {
                        let frame = s
                            .recording_frame(
                                &options.scanout_node,
                                facts.firmware.scanout.instance as usize,
                                false,
                            )?
                            .ok_or("Scanout is inactive")?;
                        let rgb = frame.rgb.clone().ok_or("Scanout omitted RGB")?;
                        Ok(Capture {
                            frame: fujimod_recording::Frame::rgb(
                                frame.width as u32,
                                frame.height as u32,
                                rgb,
                            )?,
                            time: s.board.now(),
                            clock: Clock::GuestTicks,
                            output: options.output.clone(),
                            sequence,
                            reads: ReadSet::default(),
                            completion_reads,
                        })
                    })
                })
                .map(|mut capture| {
                    capture.reads = reads.clone();
                    capture
                })
        })
    }
    fn wait<'a>(&'a mut self, reads: &'a ReadSet, deadline: u64) -> LocalFuture<'a, ()> {
        Box::pin(async move {
            if self.now >= deadline {
                return Ok(());
            }
            let options = &self.options;
            self.now = self.owner.with_session(|s| {
                let maps = s.board.mappings().to_vec();
                s.board
                    .continuation_begin(reads.stores.clone(), reads.devices.clone(), maps)?;
                let result = s.run_with_limits(
                    options.window_instructions,
                    options.quantum,
                    None,
                    Some(deadline),
                );
                let cleanup = s.board.continuation_end();
                result.and(cleanup.map(|_| ()))?;
                Ok(s.board.now())
            })?;
            futures_lite::future::yield_now().await;
            Ok(())
        })
    }
    fn turn_contact(&mut self, input: &Input) -> Result<Option<String>> {
        use fujimod_emulator_session::contact_driver::Interface;
        let InputState::Turn(steps) = input.state else {
            return Ok(None);
        };
        let id = input.control.id();
        self.owner.with_session(|s| {
            let Some(driver) = s.contact_driver.as_ref() else {
                return Ok(None);
            };
            let encoder = driver
                .spec
                .interfaces
                .iter()
                .any(|i| matches!(i, Interface::Rotary { id: found, .. } if found == id));
            if encoder {
                return Ok(None);
            }
            let member = format!("{id}.{}", if steps < 0 { "decrease" } else { "increase" });
            Ok(driver
                .spec
                .actuators
                .iter()
                .any(|a| a.id == member && a.momentary && a.states.len() == 2)
                .then_some(member))
        })
    }
    fn contact(&mut self, actuator: &str, pressed: bool) -> Result<()> {
        self.owner
            .with_session(|s| drive(s, actuator, &edge(pressed)))
    }
    fn contact_accepted(
        &mut self,
        actuator: &str,
        pressed: bool,
        reads: &mut ReadSet,
    ) -> Result<bool> {
        self.owner
            .with_session(|s| tracked(s, reads, |s| pose_accepted(s, actuator, &edge(pressed))))
    }
    fn control_position(&mut self, id: &str) -> Result<Option<String>> {
        self.owner.with_session(|s| {
            Ok(s.controls
                .as_ref()
                .and_then(|p| p.positions().get(id).cloned()))
        })
    }
}
