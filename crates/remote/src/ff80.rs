//! ff80 range reads, stock debug key events and bounded polled waits.
use crate::{backend::Backend, connection::Connection, *};
pub use fujimod_usb::ff80::commands::{
    debug::DebugEvent,
    memory::{DebugMemoryLimits, MemoryRange},
};
use fujimod_usb::{
    ff80::{
        Session,
        commands::{
            debug::{EventName, KeyState},
            memory::Address,
        },
    },
    operations::memory::read_range,
};
use std::{
    borrow::BorrowMut,
    num::NonZeroU32,
    time::{Duration, Instant},
};

pub struct Options {
    pub memory_limits: DebugMemoryLimits,
    pub event_bytes: NonZeroU32,
    pub poll_interval: Duration,
    pub output: String,
}

/// The two ff80 operations a remote backend needs from its transport. The
/// backend owns aliasing, range validation and every frame and poll decision;
/// a transport only performs one memory range read or debug event command.
/// Implementations must never retry a failed exchange.
pub trait Transport {
    /// Read exactly `range` with the caller's firmware transfer limits.
    fn memory<'a>(
        &'a mut self,
        range: MemoryRange,
        limits: DebugMemoryLimits,
    ) -> LocalFuture<'a, Vec<u8>>;
    /// Send one debug event command with at most `limit` payload bytes.
    fn event<'a>(&'a mut self, event: DebugEvent, limit: NonZeroU32) -> LocalFuture<'a, ()>;
}

impl<T: Transport + ?Sized> Transport for Box<T> {
    fn memory<'a>(
        &'a mut self,
        range: MemoryRange,
        limits: DebugMemoryLimits,
    ) -> LocalFuture<'a, Vec<u8>> {
        (**self).memory(range, limits)
    }
    fn event<'a>(&'a mut self, event: DebugEvent, limit: NonZeroU32) -> LocalFuture<'a, ()> {
        (**self).event(event, limit)
    }
}

/// The ordinary range transport over one borrowed or owned ff80 session.
pub struct RangeTransport<S> {
    session: S,
}
impl<S: BorrowMut<Session>> Transport for RangeTransport<S> {
    fn memory<'a>(
        &'a mut self,
        range: MemoryRange,
        limits: DebugMemoryLimits,
    ) -> LocalFuture<'a, Vec<u8>> {
        Box::pin(async move {
            read_range(self.session.borrow_mut(), range, limits)
                .await
                .map_err(|e| e.to_string())
        })
    }
    fn event<'a>(&'a mut self, event: DebugEvent, limit: NonZeroU32) -> LocalFuture<'a, ()> {
        Box::pin(async move {
            self.session
                .borrow_mut()
                .send_event(event, limit)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
        })
    }
}

pub struct Ff80Backend<T> {
    transport: T,
    options: Options,
    start: Instant,
    alias: u64,
}
pub type Ff80Remote<T> = Connection<Ff80Backend<T>>;
impl<S: BorrowMut<Session>> Connection<Ff80Backend<RangeTransport<S>>> {
    pub fn into_session(self) -> S {
        self.backend.transport.session
    }
    pub fn new(session: S, facts: Facts, options: Options) -> Result<Self> {
        Self::with_transport(RangeTransport { session }, facts, options)
    }
}
impl<T: Transport> Connection<Ff80Backend<T>> {
    /// Construct over any owned transport. The caller supplies the transport's
    /// own memory limits in `options`; this constructor invents none.
    pub fn with_transport(transport: T, facts: Facts, options: Options) -> Result<Self> {
        if options.poll_interval.is_zero() || options.output.is_empty() {
            return Err("Invalid ff80 polling options".into());
        }
        let alias = facts.firmware.uncached_alias;
        Self::connect(
            Ff80Backend {
                transport,
                options,
                start: Instant::now(),
                alias,
            },
            facts,
        )
    }
}
impl<T: Transport> Backend for Ff80Backend<T> {
    fn now(&self) -> u64 {
        self.start.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64
    }
    fn memory<'a>(
        &'a mut self,
        address: u64,
        size: usize,
        reads: &'a mut ReadSet,
    ) -> LocalFuture<'a, Vec<u8>> {
        Box::pin(async move {
            let address = address | self.alias;
            let range = MemoryRange::new(
                Address(u32::try_from(address).map_err(|_| "ff80 address exceeds 32 bits")?),
                NonZeroU32::new(u32::try_from(size).map_err(|_| "ff80 range exceeds 32 bits")?)
                    .ok_or("Empty ff80 read")?,
            )
            .map_err(|e| e.to_string())?;
            reads.stores.push((address, address + size as u64));
            self.transport
                .memory(range, self.options.memory_limits)
                .await
        })
    }
    fn input<'a>(&'a mut self, input: &'a Input, facts: &'a Facts) -> LocalFuture<'a, ()> {
        Box::pin(async move {
            let id = input.control.id();
            if let InputState::Turn(steps) = input.state
                && steps.unsigned_abs() > 4096
            {
                return Err("Rotation exceeds 4096 steps".into());
            }
            if input.state.is_touch() {
                return Err(format!(
                    "Control {id}: the ff80 debug transport cannot inject touch input"
                ));
            }
            let action =
                facts
                    .physical_controls
                    .action(&facts.control_ids, id, input.state.operation())?;
            // Resolve the entire action before sending any part of it.
            let names = action
                .inputs
                .iter()
                .map(|located| {
                    located.debug_key.clone().ok_or_else(|| format!(
                    "Control {id} is a native-only input; the debug transport cannot send it"))
                    .and_then(|name| EventName::new(name).map_err(|e| e.to_string()))
                })
                .collect::<Result<Vec<_>>>()?;
            for _ in 0..action.repetitions {
                for name in &names {
                    let states = if matches!(input.state, InputState::Turn(_)) {
                        vec![KeyState::On, KeyState::Off]
                    } else if action.asserted {
                        vec![KeyState::On]
                    } else {
                        vec![KeyState::Off]
                    };
                    for state in states {
                        self.transport
                            .event(
                                DebugEvent::Key {
                                    name: name.clone(),
                                    state,
                                },
                                self.options.event_bytes,
                            )
                            .await?;
                    }
                }
            }
            Ok(())
        })
    }
    fn accepted(&mut self, _: &Input, _: &Facts, _: &mut ReadSet) -> Result<bool> {
        Ok(true)
    }
    fn frame<'a>(
        &'a mut self,
        facts: &'a Facts,
        reads: &'a mut ReadSet,
    ) -> LocalFuture<'a, Capture> {
        Box::pin(async move {
            let scanout = &facts.firmware.scanout;
            let mut memory = crate::memory::Memory {
                backend: self,
                facts,
                reads: ReadSet::default(),
                scope: true,
                depth: 0,
            };
            let before =
                u64::from(memory.word(scanout.pointer_address).await?) & scanout.address_mask;
            let size = (scanout.height as usize - 1) * scanout.stride_bytes as usize
                + scanout.width as usize * 4;
            let raw = memory
                .bytes(before | facts.firmware.uncached_alias, size)
                .await?;
            let after =
                u64::from(memory.word(scanout.pointer_address).await?) & scanout.address_mask;
            if before != after {
                return Err(format!(
                    "Framebuffer changed during capture: {before:#x} -> {after:#x}"
                ));
            }
            reads.extend(memory.reads);
            let frame = match scanout.pixel_format {
                fujimod_catalog_remote::PixelFormat::Bgra8888 => fujimod_recording::Frame::bgra(
                    scanout.width,
                    scanout.height,
                    scanout.stride_bytes as usize,
                    &raw,
                )?,
                fujimod_catalog_remote::PixelFormat::Rgba8888Premultiplied => {
                    fujimod_recording::Frame::rgba_premultiplied(
                        scanout.width,
                        scanout.height,
                        scanout.stride_bytes as usize,
                        &raw,
                    )?
                }
            };
            Ok(Capture {
                frame,
                time: self.now(),
                clock: Clock::WallNanoseconds,
                output: self.options.output.clone(),
                sequence: after,
                reads: reads.clone(),
                completion_reads: ReadSet {
                    stores: vec![(
                        scanout.pointer_address | self.alias,
                        (scanout.pointer_address | self.alias) + 4,
                    )],
                    devices: Default::default(),
                },
            })
        })
    }
    fn wait<'a>(&'a mut self, _: &'a ReadSet, deadline: u64) -> LocalFuture<'a, ()> {
        Box::pin(async move {
            async_io::Timer::after(
                self.options
                    .poll_interval
                    .min(Duration::from_nanos(deadline.saturating_sub(self.now()))),
            )
            .await;
            Ok(())
        })
    }
    fn control_position(&mut self, _: &str) -> Result<Option<String>> {
        Ok(None)
    }
}
#[cfg(test)]
#[path = "ff80_tests.rs"]
mod tests;
