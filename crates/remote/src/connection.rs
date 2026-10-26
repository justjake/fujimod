//! A connection resolves references and shares the same action futures on every host.
use crate::{backend::Backend, memory::Memory, *};
use std::{collections::BTreeSet, rc::Rc};

/// A caller's standing observation, run before every wait. It returns the
/// reads it made; they join that wait's watch set, so state the caller tracks
/// is observed at every continuation boundary, including while an operation
/// waits for input readiness or acceptance.
pub type Observer = Box<dyn FnMut() -> Result<ReadSet>>;

pub struct Connection<B> {
    pub(crate) backend: B,
    pub(crate) facts: Rc<Facts>,
    observer: Option<Observer>,
    /// Touch controls with a finger down, from TouchStart until TouchEnd.
    pub(crate) touching: BTreeSet<String>,
}
impl<B: Backend> Connection<B> {
    pub fn connect(backend: B, facts: Facts) -> Result<Self> {
        facts.validate()?;
        Ok(Self {
            backend,
            facts: Rc::new(facts),
            observer: None,
            touching: BTreeSet::new(),
        })
    }
    pub fn facts(&self) -> &Facts {
        &self.facts
    }
    /// Install or remove the standing observation run before every wait.
    pub fn observe(&mut self, observer: Option<Observer>) {
        self.observer = observer;
    }
    async fn ready(&mut self, control: &Control, deadline: u64) -> Result<()> {
        crate::operations::wait_ready(self, Some(control), deadline).await
    }
    pub async fn press(&mut self, control: &Control, deadline: u64) -> Result<()> {
        crate::operations::press(self, control, deadline).await
    }
    pub async fn turn(&mut self, control: &Control, steps: i32, deadline: u64) -> Result<()> {
        crate::operations::turn(self, control, steps, deadline).await
    }
    pub async fn tap(&mut self, control: &Control, x: u32, y: u32, deadline: u64) -> Result<()> {
        crate::operations::tap(self, control, x, y, deadline).await
    }
    pub async fn drag(
        &mut self,
        control: &Control,
        points: &[(u32, u32)],
        deadline: u64,
    ) -> Result<()> {
        crate::operations::drag(self, control, points, deadline).await
    }
    pub async fn navigate(&mut self, menu: &Menu, deadline: u64) -> Result<RemoteReadResult> {
        let facts = self.facts.clone();
        crate::operations::navigate(self, &facts, menu, deadline).await
    }
    pub async fn select(&mut self, option: &OptionReference, deadline: u64) -> Result<()> {
        let facts = self.facts.clone();
        crate::operations::select(self, &facts, option, deadline).await
    }
    pub async fn screenshot(&mut self, deadline: u64) -> Result<Capture> {
        crate::operations::screenshot(self, deadline).await
    }
}
struct View<'a, B> {
    backend: &'a mut B,
    facts: &'a Facts,
}
impl<B: Backend> RemoteReader for View<'_, B> {
    fn now(&self) -> u64 {
        self.backend.now()
    }
    fn read<'a>(&'a mut self, reference: &'a Reference) -> LocalFuture<'a, RemoteReadResult> {
        Box::pin(async move {
            Memory {
                backend: self.backend,
                facts: self.facts,
                reads: ReadSet::default(),
            }
            .read_reference(reference)
            .await
        })
    }
    fn read_memory<'a>(&'a mut self, address: u64, size: usize) -> LocalFuture<'a, MemoryRead> {
        Box::pin(async move {
            let mut memory = Memory {
                backend: self.backend,
                facts: self.facts,
                reads: ReadSet::default(),
            };
            let bytes = memory.bytes(address, size).await?;
            Ok(MemoryRead {
                address,
                bytes,
                reads: memory.reads,
            })
        })
    }
    fn frame(&mut self) -> LocalFuture<'_, Capture> {
        Box::pin(async move {
            self.backend
                .frame(self.facts, &mut ReadSet::default())
                .await
        })
    }
    fn readiness<'a>(&'a mut self, control: Option<&'a Control>) -> LocalFuture<'a, Readiness> {
        Box::pin(async move {
            Memory {
                backend: self.backend,
                facts: self.facts,
                reads: ReadSet::default(),
            }
            .readiness(control)
            .await
        })
    }
}
impl<B: Backend> RemoteReader for Connection<B> {
    fn now(&self) -> u64 {
        self.backend.now()
    }
    fn read<'a>(&'a mut self, reference: &'a Reference) -> LocalFuture<'a, RemoteReadResult> {
        Box::pin(async move {
            View {
                backend: &mut self.backend,
                facts: &self.facts,
            }
            .read(reference)
            .await
        })
    }
    fn read_memory<'a>(&'a mut self, address: u64, size: usize) -> LocalFuture<'a, MemoryRead> {
        Box::pin(async move {
            View {
                backend: &mut self.backend,
                facts: &self.facts,
            }
            .read_memory(address, size)
            .await
        })
    }
    fn frame(&mut self) -> LocalFuture<'_, Capture> {
        Box::pin(async move {
            self.backend
                .frame(&self.facts, &mut ReadSet::default())
                .await
        })
    }
    fn readiness<'a>(&'a mut self, control: Option<&'a Control>) -> LocalFuture<'a, Readiness> {
        Box::pin(async move {
            View {
                backend: &mut self.backend,
                facts: &self.facts,
            }
            .readiness(control)
            .await
        })
    }
}
impl<B: Backend> Remote for Connection<B> {
    fn wait<'a>(&'a mut self, reads: &'a ReadSet, deadline: u64) -> LocalFuture<'a, ()> {
        Box::pin(async move {
            match &mut self.observer {
                Some(observe) => {
                    let mut watched = reads.clone();
                    watched.extend(observe()?);
                    self.backend.wait(&watched, deadline).await
                }
                None => self.backend.wait(reads, deadline).await,
            }
        })
    }
    fn set_input<'a>(&'a mut self, input: &'a Input, deadline: u64) -> LocalFuture<'a, ()> {
        Box::pin(async move {
            let id = input.control.id();
            if input.state.is_touch() {
                // Refuse a point outside the surface before waiting on anything.
                self.facts.physical_controls.action(
                    &self.facts.control_ids,
                    id,
                    input.state.operation(),
                )?;
            }
            let touching = self.touching.contains(id);
            match input.state {
                InputState::TouchStart { .. } if touching => {
                    return Err(format!("A finger is already down on {id}; move it instead"));
                }
                InputState::TouchMove { .. } | InputState::TouchEnd if !touching => {
                    return Err(format!("No finger is down on {id}"));
                }
                _ => {}
            }
            // Only a new press waits for UI readiness. A release, or a finger
            // already down moving or lifting, must not block on it.
            if !matches!(
                input.state,
                InputState::Released | InputState::TouchMove { .. } | InputState::TouchEnd
            ) {
                self.ready(&input.control, deadline).await?;
                let read_result = self
                    .read(&Reference::Control(input.control.clone()))
                    .await?;
                if matches!(
                    read_result.value,
                    Value::Control(ControlSnapshot {
                        supported: false,
                        ..
                    })
                ) {
                    return Err(format!("Control {} is unavailable", input.control.id()));
                }
            }
            if let InputState::Turn(steps) = input.state
                && let Some(actuator) = self.backend.turn_contact(input)?
            {
                self.facts.physical_controls.action(
                    &self.facts.control_ids,
                    id,
                    input.state.operation(),
                )?;
                if steps.unsigned_abs() > 4096 {
                    return Err("Rotation exceeds 4096 steps".into());
                }
                // Each step is one press and release of the direction's
                // contact, accepted by the scanner like a button's.
                for step in 0..steps.unsigned_abs() {
                    for pressed in [true, false] {
                        if pressed && step > 0 {
                            self.ready(&input.control, deadline).await?;
                        }
                        self.backend.contact(&actuator, pressed)?;
                        loop {
                            let mut reads = ReadSet::default();
                            if self
                                .backend
                                .contact_accepted(&actuator, pressed, &mut reads)?
                            {
                                break;
                            }
                            if self.now() >= deadline {
                                return Err(format!(
                                    "Timed out waiting for {actuator} {} in {input:?}",
                                    if pressed { "press" } else { "release" }
                                ));
                            }
                            self.wait(&reads, deadline).await?;
                        }
                    }
                }
                return Ok(());
            }
            // Backends validate the complete physical action before applying it.
            // OFF must not depend on live state reads that can block its release.
            self.backend.input(input, &self.facts).await?;
            match input.state {
                InputState::TouchStart { .. } => {
                    self.touching.insert(id.into());
                }
                InputState::TouchEnd => {
                    self.touching.remove(id);
                }
                _ => {}
            }
            loop {
                let mut reads = ReadSet::default();
                if self.backend.accepted(input, &self.facts, &mut reads)? {
                    return Ok(());
                }
                if self.now() >= deadline {
                    return Err(format!("Timed out waiting for input acceptance: {input:?}"));
                }
                self.wait(&reads, deadline).await?;
            }
        })
    }
}
