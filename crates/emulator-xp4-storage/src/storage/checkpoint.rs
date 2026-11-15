//! Restore physical flash/controller state into the already connected graph.
//! Immutable flash bytes, geometry and interrupt routes belong to the factory.
use super::*;
use serde::{Deserialize, Serialize};
use fujimod_emulator_devices::checkpoint::Checkpoint;

#[derive(Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
enum Pending {
    Erase {
        at: u64,
        erase_address: u64,
        size: usize,
    },
    Program {
        at: u64,
        program_address: u64,
        bytes: Vec<u8>,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NorState {
    #[serde(default)]
    immutable_prefix: Option<usize>,
    #[serde(default)]
    executable_range: Option<ExecutableRange>,
    #[serde(default)]
    write_protect: bool,
    #[serde(default)]
    write_ranges: Vec<WriteRange>,
    #[serde(default)]
    read_backing: Option<ReadBacking>,
    jedec: u32,
    write_enable: bool,
    pending: Option<Pending>,
    erase_bytes: usize,
    page_bytes: usize,
    erase_ticks: u64,
    page_ticks: u64,
    overlay: BTreeMap<usize, Vec<u8>>,
    #[serde(default)]
    program_count: u64,
    #[serde(default)]
    programmed_ranges: Vec<(usize, usize)>,
}
fn extent(address: u64, size: usize) -> bool {
    address
        .checked_sub(FLASH)
        .and_then(|a| a.checked_add(size as u64))
        .is_some_and(|end| end <= FLASH_SIZE)
}
impl Nor {
    // Coverage combines completed operations, so adjacent authorized ranges may
    // form one span even though no single write may cross their boundary.
    fn coverage_is_admitted(&self, start: usize, end: usize) -> bool {
        if self.write_ranges.is_empty() {
            return self.check_mutation(FLASH + start as u64, end - start).is_ok();
        }
        let mut cursor = start as u64;
        for range in &self.write_ranges {
            if range.offset > cursor {
                break;
            }
            let limit = (range.offset + range.length).min(end as u64);
            if limit > cursor {
                if self.check_mutation(FLASH + cursor, (limit - cursor) as usize).is_err() {
                    return false;
                }
                cursor = limit;
                if cursor == end as u64 {
                    return true;
                }
            }
        }
        false
    }

    fn capture_state(&self) -> NorState {
        NorState {
            immutable_prefix: self.prefix.map(|prefix| prefix.bytes()),
            executable_range: self.executable.as_ref().map(|view| ExecutableRange {
                offset: view.offset(), bytes: view.len(),
            }),
            write_protect: self.write_protect, write_ranges: self.write_ranges.clone(),
            read_backing: self.read_backing.clone(),
            jedec: self.jedec, write_enable: self.write_enable,
            pending: self.pending.as_ref().map(|(at, mutation)| match mutation {
                Mutation::Erase { address, size } => Pending::Erase {
                    at: *at, erase_address: *address, size: *size,
                },
                Mutation::Program { address, bytes } => Pending::Program {
                    at: *at, program_address: *address, bytes: bytes.clone(),
                },
            }),
            erase_bytes: self.geometry.erase_bytes, page_bytes: self.geometry.page_bytes,
            erase_ticks: self.geometry.erase_ticks, page_ticks: self.geometry.page_ticks,
            overlay: self.overlay.clone(),
            program_count: self.program_coverage.count,
            programmed_ranges: self.program_coverage.ranges(),
        }
    }

    fn validate_state(
        &self,
        state: &NorState,
    ) -> Result<(Option<(u64, Mutation)>, ProgramCoverage), String> {
        let g = self.geometry;
        if state.immutable_prefix != self.prefix.map(|p| p.bytes())
            || state.executable_range != self.executable.as_ref().map(|v| ExecutableRange { offset: v.offset(), bytes: v.len() })
            || state.write_protect != self.write_protect
            || state.write_ranges != self.write_ranges
            || state.read_backing != self.read_backing
            || state.jedec != self.jedec
            || state.erase_bytes != g.erase_bytes
            || state.page_bytes != g.page_bytes
            || state.erase_ticks != g.erase_ticks
            || state.page_ticks != g.page_ticks
            || state.overlay.iter().any(|(page, bytes)| {
                *page >= FLASH_SIZE as usize / g.page_bytes
                    || self.check_mutation(FLASH + (*page * g.page_bytes) as u64, bytes.len()).is_err() || bytes.len() != g.page_bytes
            })
            || state.pending.is_some() && state.write_enable
        {
            return Err("Invalid XP4 NOR checkpoint or changed physical input".into());
        }
        let pending = match &state.pending {
            Some(Pending::Erase {
                at,
                erase_address: address,
                size,
            }) => {
                if ![4096, g.erase_bytes].contains(size)
                    || address.wrapping_sub(FLASH) % *size as u64 != 0
                    || !extent(*address, *size)
                    || self.check_mutation(*address, *size).is_err()
                {
                    return Err("Invalid pending XP4 NOR erase".into());
                }
                Some((*at, Mutation::Erase { address: *address, size: *size }))
            }
            Some(Pending::Program {
                at,
                program_address: address,
                bytes,
            }) => {
                if bytes.is_empty()
                    || !extent(*address, bytes.len())
                    || self.check_mutation(*address, bytes.len()).is_err()
                    || address.wrapping_sub(FLASH) as usize % g.page_bytes + bytes.len()
                        > g.page_bytes
                {
                    return Err("Invalid pending XP4 NOR program".into());
                }
                Some((*at, Mutation::Program { address: *address, bytes: bytes.clone() }))
            }
            None => None,
        };
        let mut previous_end = None;
        for &(start, end) in &state.programmed_ranges {
            if start >= end
                || end > FLASH_SIZE as usize
                || previous_end.is_some_and(|previous| start <= previous)
                || !self.coverage_is_admitted(start, end)
                || state.program_count == 0
            {
                return Err("Invalid XP4 NOR program coverage".into());
            }
            previous_end = Some(end);
        }
        let mut coverage = ProgramCoverage {
            count: state.program_count,
            ..Default::default()
        };
        for &(start, end) in &state.programmed_ranges {
            coverage.mark(start, end, true);
        }
        Ok((pending, coverage))
    }

    fn apply_state(&mut self, state: NorState) -> Result<(), String> {
        let (pending, coverage) = self.validate_state(&state)?;
        self.write_enable = state.write_enable;
        self.overlay = state.overlay;
        self.program_coverage = coverage;
        self.pending = pending;
        if let Some(view) = &self.executable { self.sync_executable(FLASH + view.offset() as u64, view.len()); }
        Ok(())
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FlashState {
    registers: Vec<u32>,
    next: Option<(u64, u64)>,
    operations: u64,
    jedec: u32,
    nor: NorState,
    stream: Option<Request>,
    program_fifo: Option<(u64, Vec<u8>)>,
    irq: u32,
    irq_level: bool,
    #[serde(default)]
    mapped_read_gate: Option<MappedReadGate>,
    #[serde(default)]
    pad: u32,
    #[serde(default)]
    mapped_misses: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FlashArchive {
    state: FlashState,
    pages: Vec<usize>,
    epoch: u64,
}
impl SerialFlashController {
    pub(super) fn capture_portable(&self) -> Result<Checkpoint, String> {
        let port = self.port.lock().map_err(|_| "XP4 NOR lock poisoned")?;
        let mut state = FlashState {
            registers: self.regs.to_vec(), next: self.due, operations: self.operations,
            jedec: port.nor.jedec, nor: port.nor.capture_state(), stream: port.request.clone(),
            program_fifo: port.program_data.clone(), irq: self.irq, irq_level: self.irq_level,
            mapped_read_gate: self.gate.clone(), pad: self.pad, mapped_misses: self.mapped_misses,
        };
        let (pages, blobs): (Vec<_>, Vec<_>) = std::mem::take(&mut state.nor.overlay).into_iter()
            .map(|(page, data)| (page, Arc::from(data))).unzip();
        let archive = FlashArchive { state, pages, epoch: self.epoch.load(Ordering::Relaxed) };
        Ok(Checkpoint::encode(self.name(), &archive)?.with_blobs(blobs))
    }

    fn decode_portable(&self, data: &Checkpoint) -> Result<FlashArchive, String> {
        let mut archive: FlashArchive = data.decode(self.name())?;
        let page_bytes = self.port.lock().map_err(|_| "XP4 NOR lock poisoned")?.nor.geometry.page_bytes;
        if !archive.state.nor.overlay.is_empty() || archive.pages.len() != data.blobs().len()
            || archive.pages.windows(2).any(|pair| pair[0] >= pair[1])
            || data.blobs().iter().any(|page| page.len() != page_bytes)
        {
            return Err("XP4 flash checkpoint page directory differs".into());
        }
        archive.state.nor.overlay = archive.pages.iter().copied().zip(data.blobs().iter().map(|bytes| bytes.to_vec())).collect();
        Ok(archive)
    }

    pub(super) fn validate_portable(&self, data: &Checkpoint) -> Result<(), String> {
        self.validate_state(&self.decode_portable(data)?.state).map(|_| ())
    }

    pub(super) fn restore_portable(&mut self, data: &Checkpoint) -> Result<(), String> {
        let archive = self.decode_portable(data)?;
        self.apply_state(archive.state)?;
        self.epoch.store(archive.epoch, Ordering::Relaxed);
        Ok(())
    }

    pub(super) fn restore_state(&mut self, value: &Value) -> Result<(), String> {
        let state: FlashState = serde_json::from_value(value.clone()).map_err(|error| error.to_string())?;
        self.apply_state(state)
    }

    fn validate_state(&self, state: &FlashState) -> Result<[u32; 64], String> {
        let registers: [u32; 64] = state
            .registers.as_slice()
            .try_into()
            .map_err(|_| "Invalid XP4 flash register count")?;
        let geometry = self.port.lock().unwrap().nor.geometry;
        if state.irq != self.irq
            || state.mapped_read_gate != self.gate
            || self.gate.is_none() && (state.pad != 0 || state.mapped_misses != 0)
            || state.jedec != state.nor.jedec
            || state
                .next
                .is_some_and(|(_, token)| ![1, 2, 4, 5, 6].contains(&token))
            || state
                .stream
                .as_ref()
                .is_some_and(|r| !extent(r.address, r.size) || !r.accepted && r.accepted_at != 0)
            || state
                .program_fifo
                .as_ref()
                .is_some_and(|(arrived, bytes)| {
                    // The FIFO retains its DMA arrival tick, not an address.
                    // Its destination belongs to the accepted write stream.
                    bytes.is_empty()
                        || arrived.checked_add(geometry.page_ticks).is_none()
                        || state.nor.pending.is_some()
                        || !state.nor.write_enable
                        || !state.stream.as_ref().is_some_and(|request| {
                            request.write
                                && request.accepted
                                && request.accepted_at <= *arrived
                                && request.size == bytes.len()
                                && extent(request.address, bytes.len())
                                && request.address.wrapping_sub(FLASH) as usize
                                    % geometry.page_bytes
                                    + bytes.len()
                                    <= geometry.page_bytes
                        })
                })
        {
            return Err("Invalid XP4 serial-flash checkpoint or route".into());
        }
        let port = self.port.lock().unwrap();
        if state.next.is_some_and(|(_, token)| token == 1) {
            port.nor
                .check_command(registers[0x90 / 4], registers[0x94 / 4])?;
        }
        if let Some(request) = &state.stream {
            if request.write {
                port.nor.check_mutation(request.address, request.size)?;
            } else {
                // Validate against factory coverage before any restored state
                // is committed, including streams already accepted by DMA.
                port.nor.check_read_backing(request.address, request.size)?;
            }
        }
        port.nor.validate_state(&state.nor)?;
        Ok(registers)
    }

    fn apply_state(&mut self, state: FlashState) -> Result<(), String> {
        let registers = self.validate_state(&state)?;
        let mut port = self.port.lock().map_err(|_| "XP4 NOR lock poisoned")?;
        port.nor.apply_state(state.nor)?;
        port.request = state.stream;
        port.program_data = state.program_fifo;
        self.regs = registers;
        self.due = state.next;
        self.operations = state.operations;
        self.irq_level = state.irq_level;
        self.pad = state.pad;
        self.mapped_misses = state.mapped_misses;
        Ok(())
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DmaState {
    #[serde(default)]
    serial_flash_peer: Option<bool>,
    #[serde(default)]
    reset_profile: Option<String>,
    #[serde(default)]
    timing_profile: Option<String>,
    #[serde(default)]
    assumptions_recorded: Option<bool>,
    channels: Vec<Channel>,
    irq_routes: [u32; 16],
    request_mux: BTreeMap<u64, u32>,
    request_mux_changed: [u64; 16],
    #[serde(default)]
    request: BTreeMap<u64, u32>,
    #[serde(default)]
    audio: Option<AudioPacing>,
    #[serde(default)]
    microphone_frames: Option<u64>,
}
impl DmaController {
    pub(super) fn capture_portable(&self) -> Result<Checkpoint, String> {
        if self.receiver.is_some() {
            return Err("connected transmit receiver has no portable checkpoint".into());
        }
        let standalone = self.port.is_none();
        let state = DmaState {
            serial_flash_peer: standalone.then_some(false),
            reset_profile: standalone.then(|| Self::STANDALONE_RESET.to_owned()),
            timing_profile: standalone.then(|| Self::STANDALONE_TIMING.to_owned()),
            assumptions_recorded: standalone.then_some(self.standalone_admitted),
            channels: self.channels.to_vec(), irq_routes: self.routes,
            request_mux: self.mux.clone(), request_mux_changed: self.mux_changed,
            request: self.request.clone(), audio: self.audio.clone(),
            microphone_frames: self.microphone().map(|_| self.microphone_frames),
        };
        Checkpoint::encode(self.name(), &state)
    }

    pub(super) fn validate_portable(&self, data: &Checkpoint) -> Result<(), String> {
        self.validate_state(&data.decode(self.name())?)
    }

    pub(super) fn restore_portable(&mut self, data: &Checkpoint) -> Result<(), String> {
        self.apply_state(data.decode(self.name())?)
    }

    pub(super) fn restore_state(&mut self, value: &Value) -> Result<(), String> {
        if value["transmit_receiver_connected"] == true {
            return Err("Connected transmit receiver checkpoint restore is not supported".into());
        }
        self.apply_state(serde_json::from_value(value.clone()).map_err(|error| error.to_string())?)
    }

    fn validate_state(&self, state: &DmaState) -> Result<(), String> {
        if self.receiver.is_some() {
            return Err("connected transmit receiver has no portable checkpoint".into());
        }
        let standalone = self.port.is_none();
        let mode_matches = if standalone {
            state.serial_flash_peer == Some(false)
                && state.reset_profile.as_deref() == Some(Self::STANDALONE_RESET)
                && state.timing_profile.as_deref() == Some(Self::STANDALONE_TIMING)
                && state.assumptions_recorded.is_some()
        } else {
            state.serial_flash_peer.unwrap_or(true) && state.reset_profile.is_none()
                && state.timing_profile.is_none() && state.assumptions_recorded.is_none()
        };
        if !mode_matches || state.irq_routes != self.routes
            || state.audio != self.audio
            || state.microphone_frames.is_some() != self.microphone().is_some()
            || state.channels.len() != 16
            || state.channels.iter().enumerate().any(|(n, c)| {
                c.due.is_some_and(|at| !c.submitted || at < c.submitted_at)
                    || c.submitted && u64::from(c.regs[4]) + 1 > FLASH_SIZE
                    // Only a declared audio channel runs a linked list.
                    || c.submitted && c.regs[0x24 / 4] != 0
                        && (self.audio(n).is_none() || c.regs[0x24 / 4] & 0xe != 0)
                    || standalone && c.submitted && (Self::needs_flash(c) || c.regs[0x24 / 4] != 0
                        || c.regs[0x28 / 4] & 0x10000001 != 0x10000001
                        || Self::standalone_deadline(c).is_none()
                        || c.due.is_some_and(|at| Self::standalone_deadline(c) != Some(at)))
            })
            || state.request_mux.iter().any(|(a, v)| {
                !(0xfff4a500..0xfff4a520).contains(a) || a & 3 != 0 || v & !0x3f3f != 0
            })
            || state
                .request
                .keys()
                .any(|a| !(0xfff4a800..0xfff4a880).contains(a) || a & 3 != 0)
        {
            return Err("Invalid XP4 DMA checkpoint or routes".into());
        }
        if let Some(port) = &self.port {
            let port = port.lock().unwrap();
            for channel in &state.channels {
                if channel.submitted { port.nor.check_mutation(u64::from(channel.regs[0x18 / 4]), channel.regs[0x10 / 4] as usize + 1)?; }
            }
        }
        Ok(())
    }

    fn apply_state(&mut self, state: DmaState) -> Result<(), String> {
        self.validate_state(&state)?;
        self.standalone_admitted = state.assumptions_recorded.unwrap_or(false);
        self.microphone_frames = state.microphone_frames.unwrap_or(0);
        self.channels = state
            .channels
            .try_into()
            .map_err(|_| "Invalid XP4 DMA channel count")?;
        self.mux = state.request_mux;
        self.mux_changed = state.request_mux_changed;
        self.request = state.request;
        Ok(())
    }
}
