//! FE80 reset, route setup and descriptor-driven opaque program upload.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const MAX_UPLOAD_BYTES: u32 = 16 * 1024 * 1024;
const STATUS: u64 = 0xfe80d84c;
const CONTROL_STATUS: u64 = 0xfe80c000;
const DESCRIPTOR_POINTER: u64 = 0xfe80c1c8;
const TRANSFER_CONTROL: u64 = 0xfe80c1c0;
const CHANNEL_CONTROL: u64 = 0xfe80c540;
const OUTPUT_CHANNEL_CONTROL: u64 = 0xfe80c440;
const OUTPUT_CHANNEL_CONFIG: u64 = 0xfe80c448;
const OUTPUT_CHANNEL_POINTER: u64 = 0xfe80c108;
const OUTPUT_CHANNEL_GATE: u64 = 0xfe80c100;
const OUTPUT_CHANNEL_STATUS: u64 = 0xfe80c404;
const CAPTURE_CHANNEL_CONTROL: u64 = 0xfe80c840;
const CAPTURE_CHANNEL_CONFIG: u64 = 0xfe80c848;
const CAPTURE_CHANNEL_POINTER: u64 = 0xfe80c208;
const CAPTURE_CHANNEL_GATE: u64 = 0xfe80c200;
const CAPTURE_CHANNEL_STATUS: u64 = 0xfe80c804;
const CHANNEL_COUNT: u8 = 4;
const CHANNEL_CONTROL_STRIDE: u64 = 0x40;
const CHANNEL_GATE_STRIDE: u64 = 0x30;
const RESET_PERIOD_WORDS: [u32; 4] = [
    0x000a_0000,
    0x000b_0000,
    0x000c_0000,
    0x000f_0000,
];
const ROUTE_SEQUENCE: [(u64, u32); 5] = [
    (0xfe318040, 1),
    (0xfe318040, 0),
    (0xfe318048, 1),
    (0xfe318010, 0),
    (0xfe318008, 1),
];
const REGISTERS: &[u64] = &[
    0xfe80d840,
    0xfe80d844,
    0xfe80d848,
    STATUS,
    0xfe80d800,
    0xfe80cc20,
    CHANNEL_CONTROL,
    OUTPUT_CHANNEL_STATUS,
    CAPTURE_CHANNEL_STATUS,
    DESCRIPTOR_POINTER,
    TRANSFER_CONTROL,
    CONTROL_STATUS,
    0xfe3b0000,
    0xfe318040,
    0xfe318048,
    0xfe318010,
    0xfe318008,
];

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    #[serde(default = "default_handshake_ticks")]
    pub handshake_ticks: u64,
    #[serde(default = "default_upload_bytes_per_tick")]
    pub upload_bytes_per_tick: u64,
}

const fn default_handshake_ticks() -> u64 {
    2
}
const fn default_upload_bytes_per_tick() -> u64 {
    256
}

impl Default for Options {
    fn default() -> Self {
        Self {
            handshake_ticks: default_handshake_ticks(),
            upload_bytes_per_tick: default_upload_bytes_per_tick(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Phase {
    Idle,
    ResetPrepared,
    ResetPending,
    ResetAcknowledged,
    PeriodProgrammed,
    ResetReleased,
    Disabled,
    Routed,
    EnableRequested,
    EnablePending,
    Enabled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum UploadPhase {
    Idle,
    Prepared,
    PointerSet,
    Pending,
    Completed,
    Acknowledged,
    Stopped,
    ReleaseSet,
    Released,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum PendingKind {
    ResetHandshake,
    EnableHandshake,
    Upload,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct State {
    registers: BTreeMap<u64, u32>,
    phase: Phase,
    upload_phase: UploadPhase,
    owner: Option<usize>,
    busy: bool,
    uploaded: bool,
    route_index: Option<u8>,
    route_ready: bool,
    handshakes_started: u64,
    handshakes_completed: u64,
    uploads_started: u64,
    uploads_completed: u64,
    output_channel_control_pending: Option<u8>,
    output_channel_selected: Option<u8>,
    output_channel_programmed: u8,
    output_channel_disabled: u8,
    output_channel_open: u8,
    capture_channel_control_pending: Option<u8>,
    capture_channel_selected: Option<u8>,
    capture_channel_programmed: u8,
    capture_channel_disabled: u8,
    capture_channel_open: u8,
    program: Vec<u8>,
    pending_program: Vec<u8>,
    pending: Option<PendingKind>,
    deadline: Option<u64>,
    generation: u64,
}

/// FE80 upload transport; program bytes are opaque and never executed here.
pub struct ProgramUpload {
    state: State,
    options: Options,
}

impl ProgramUpload {
    pub fn new(options: Options) -> Result<Self, String> {
        if options.handshake_ticks == 0 || options.upload_bytes_per_tick == 0 {
            return Err("FE80 handshake delay and upload throughput must be positive".into());
        }
        Ok(Self {
            state: State {
                registers: BTreeMap::new(),
                phase: Phase::Idle,
                upload_phase: UploadPhase::Idle,
                owner: None,
                busy: false,
                uploaded: false,
                route_index: None,
                route_ready: false,
                handshakes_started: 0,
                handshakes_completed: 0,
                uploads_started: 0,
                uploads_completed: 0,
                output_channel_control_pending: None,
                output_channel_selected: None,
                output_channel_programmed: 0,
                output_channel_disabled: 0,
                output_channel_open: 0,
                capture_channel_control_pending: None,
                capture_channel_selected: None,
                capture_channel_programmed: 0,
                capture_channel_disabled: 0,
                capture_channel_open: 0,
                program: Vec::new(),
                pending_program: Vec::new(),
                pending: None,
                deadline: None,
                generation: 0,
            },
            options,
        })
    }

    fn channel_index(address: u64, base: u64, stride: u64) -> Option<u8> {
        let offset = address.checked_sub(base)?;
        if offset % stride != 0 {
            return None;
        }
        let index = offset / stride;
        (index < u64::from(CHANNEL_COUNT)).then_some(index as u8)
    }

    fn output_channel_control(address: u64) -> Option<u8> {
        Self::channel_index(address, OUTPUT_CHANNEL_CONTROL, CHANNEL_CONTROL_STRIDE)
    }

    fn output_channel_config(address: u64) -> Option<u8> {
        Self::channel_index(address, OUTPUT_CHANNEL_CONFIG, CHANNEL_CONTROL_STRIDE)
    }

    fn output_channel_pointer(address: u64) -> Option<u8> {
        Self::channel_index(address, OUTPUT_CHANNEL_POINTER, CHANNEL_GATE_STRIDE)
    }

    fn output_channel_gate(address: u64) -> Option<u8> {
        Self::channel_index(address, OUTPUT_CHANNEL_GATE, CHANNEL_GATE_STRIDE)
    }

    fn capture_channel_control(address: u64) -> Option<u8> {
        Self::channel_index(address, CAPTURE_CHANNEL_CONTROL, CHANNEL_CONTROL_STRIDE)
    }

    fn capture_channel_config(address: u64) -> Option<u8> {
        Self::channel_index(address, CAPTURE_CHANNEL_CONFIG, CHANNEL_CONTROL_STRIDE)
    }

    fn capture_channel_pointer(address: u64) -> Option<u8> {
        Self::channel_index(address, CAPTURE_CHANNEL_POINTER, CHANNEL_GATE_STRIDE)
    }

    fn capture_channel_gate(address: u64) -> Option<u8> {
        Self::channel_index(address, CAPTURE_CHANNEL_GATE, CHANNEL_GATE_STRIDE)
    }

    fn channel_register(address: u64) -> bool {
        Self::output_channel_control(address).is_some()
            || Self::output_channel_config(address).is_some()
            || Self::output_channel_pointer(address).is_some()
            || Self::output_channel_gate(address).is_some()
            || Self::capture_channel_control(address).is_some()
            || Self::capture_channel_config(address).is_some()
            || Self::capture_channel_pointer(address).is_some()
            || Self::capture_channel_gate(address).is_some()
    }

    fn known_address(address: u64) -> bool {
        REGISTERS.contains(&address) || Self::channel_register(address)
    }

    fn known(address: u64, size: u8) -> bool {
        size == 4 && address & 3 == 0 && Self::known_address(address)
    }

    fn upload_transport_open(phase: Phase) -> bool {
        // Upload is allowed while reset remains disabled. Routing may occur
        // before or after upload; both orders use the same registers.
        matches!(phase, Phase::Disabled | Phase::Routed)
    }

    fn valid_state(state: &State) -> bool {
        state
            .registers
            .keys()
            .all(|address| Self::known_address(*address))
            && state
                .registers
                .get(&STATUS)
                .is_none_or(|value| matches!(value, 0 | 1))
            && state
                .registers
                .get(&CONTROL_STATUS)
                .is_none_or(|value| matches!(value, 0 | 0x10))
            && state.pending.is_some() == state.deadline.is_some()
            && state.busy == state.pending.is_some()
            && (state.pending == Some(PendingKind::Upload)) == !state.pending_program.is_empty()
            && (state.pending != Some(PendingKind::Upload)
                || state.upload_phase == UploadPhase::Pending)
            && (state.pending != Some(PendingKind::ResetHandshake)
                || state.phase == Phase::ResetPending)
            && (state.pending != Some(PendingKind::EnableHandshake)
                || state.phase == Phase::EnablePending)
            && state.uploaded == !state.program.is_empty()
            && (state.upload_phase != UploadPhase::Completed
                || state.registers.get(&CONTROL_STATUS) == Some(&0x10))
            && (state.upload_phase == UploadPhase::Completed
                || state.registers.get(&CONTROL_STATUS).copied().unwrap_or(0) == 0)
            && state.program.len() <= MAX_UPLOAD_BYTES as usize
            && state.pending_program.len() <= MAX_UPLOAD_BYTES as usize
            && state.handshakes_completed <= state.handshakes_started
            && state.uploads_completed <= state.uploads_started
            && state
                .output_channel_control_pending
                .is_none_or(|channel| channel < CHANNEL_COUNT)
            && state
                .output_channel_selected
                .is_none_or(|channel| channel < CHANNEL_COUNT)
            && state
                .capture_channel_control_pending
                .is_none_or(|channel| channel < CHANNEL_COUNT)
            && state
                .capture_channel_selected
                .is_none_or(|channel| channel < CHANNEL_COUNT)
            && state.output_channel_programmed & !0x0f == 0
            && state.output_channel_disabled & !0x0f == 0
            && state.output_channel_open & !0x0f == 0
            && state.capture_channel_programmed & !0x0f == 0
            && state.capture_channel_disabled & !0x0f == 0
            && state.capture_channel_open & !0x0f == 0
            && state
                .registers
                .get(&OUTPUT_CHANNEL_STATUS)
                .is_none_or(|value| value & !0x0f == 0)
            && state
                .registers
                .get(&CAPTURE_CHANNEL_STATUS)
                .is_none_or(|value| value & !0x0f == 0)
            && state
                .route_index
                .is_none_or(|index| index <= ROUTE_SEQUENCE.len() as u8)
    }

    fn schedule(&mut self, cx: &mut Cx<'_>, kind: PendingKind, delay: u64) -> Result<(), String> {
        let deadline = cx
            .now()
            .checked_add(delay)
            .ok_or("FE80 deadline overflow")?;
        let generation = self
            .state
            .generation
            .checked_add(1)
            .ok_or("FE80 event generation overflow")?;
        self.state.generation = generation;
        self.state.pending = Some(kind);
        self.state.deadline = Some(deadline);
        self.state.busy = true;
        cx.schedule(deadline, generation);
        Ok(())
    }

    fn start_handshake(&mut self, cx: &mut Cx<'_>, reset: bool) -> Result<(), String> {
        self.state.handshakes_started = self
            .state
            .handshakes_started
            .checked_add(1)
            .ok_or("FE80 handshake counter overflow")?;
        self.state.registers.insert(STATUS, 0);
        self.state.phase = if reset {
            Phase::ResetPending
        } else {
            Phase::EnablePending
        };
        self.schedule(
            cx,
            if reset {
                PendingKind::ResetHandshake
            } else {
                PendingKind::EnableHandshake
            },
            self.options.handshake_ticks,
        )
    }

    fn start_upload(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        let pointer = self
            .state
            .registers
            .get(&DESCRIPTOR_POINTER)
            .copied()
            .unwrap_or(0);
        if pointer & 3 != 0 {
            return Err("FE80 descriptor must be word aligned".into());
        }
        let descriptor = cx
            .ram_bytes(u64::from(pointer), 16)
            .map_err(|error| format!("FE80 descriptor DMA read failed: {error}"))?;
        let flags = u32::from_le_bytes(descriptor[0..4].try_into().unwrap());
        let source = u32::from_le_bytes(descriptor[4..8].try_into().unwrap());
        let count = u32::from_le_bytes(descriptor[8..12].try_into().unwrap());
        let reserved = u32::from_le_bytes(descriptor[12..16].try_into().unwrap());
        let end = u64::from(source).checked_add(u64::from(count));
        if flags != 0x0300_0000
            || reserved != 0
            || count == 0
            || count > MAX_UPLOAD_BYTES
            || source & 3 != 0
            || end.is_none_or(|end| end > u64::from(u32::MAX) + 1)
        {
            return Err("unsupported FE80 DMA descriptor flags, size or alignment".into());
        }
        let program = cx
            .ram_bytes(u64::from(source), count as usize)
            .map_err(|error| format!("FE80 program DMA read failed: {error}"))?;
        if program.len() != count as usize {
            return Err("FE80 program DMA source was truncated".into());
        }
        let delay = self
            .options
            .handshake_ticks
            .max(u64::from(count).div_ceil(self.options.upload_bytes_per_tick));
        self.state.uploads_started = self
            .state
            .uploads_started
            .checked_add(1)
            .ok_or("FE80 upload counter overflow")?;
        self.state.registers.insert(CONTROL_STATUS, 0);
        self.state.pending_program = program;
        self.state.upload_phase = UploadPhase::Pending;
        if let Err(error) = self.schedule(cx, PendingKind::Upload, delay) {
            self.state.pending_program.clear();
            self.state.upload_phase = UploadPhase::PointerSet;
            self.state.uploads_started -= 1;
            return Err(error);
        }
        Ok(())
    }

    pub(super) fn advance(&mut self, cx: &mut Cx<'_>) {
        if !self
            .state
            .deadline
            .is_some_and(|deadline| deadline <= cx.now())
        {
            return;
        }
        let Some(pending) = self.state.pending.take() else {
            return;
        };
        self.state.deadline = None;
        self.state.busy = false;
        match pending {
            PendingKind::ResetHandshake => {
                self.state.registers.insert(STATUS, 1);
                self.state.phase = Phase::ResetAcknowledged;
                self.state.handshakes_completed += 1;
            }
            PendingKind::EnableHandshake => {
                self.state.registers.insert(STATUS, 1);
                self.state.phase = Phase::Enabled;
                self.state.handshakes_completed += 1;
            }
            PendingKind::Upload => {
                self.state.program = std::mem::take(&mut self.state.pending_program);
                self.state.registers.insert(CONTROL_STATUS, 0x10);
                self.state.upload_phase = UploadPhase::Completed;
                self.state.uploaded = true;
                self.state.uploads_completed += 1;
            }
        }
    }

    fn write_register(&mut self, cx: &mut Cx<'_>, address: u64, value: u32) -> Result<(), String> {
        if self.state.busy {
            return Err("FE80 setup mutation while acknowledgment is pending".into());
        }
        if address == STATUS {
            return Err("FE80 handshake status is read-only".into());
        }
        if address == 0xfe80d840 {
            if value != 0 {
                return Err("only stock FE80 reset-preparation value 0 is supported".into());
            }
            self.state.owner = Some(cx.cpu);
            self.state.phase = Phase::ResetPrepared;
            self.state.upload_phase = UploadPhase::Idle;
            self.state.uploaded = false;
            self.state.program.clear();
            self.state.pending_program.clear();
            self.state.registers.insert(STATUS, 0);
            self.state.registers.insert(CONTROL_STATUS, 0);
            self.state.output_channel_control_pending = None;
            self.state.output_channel_selected = None;
            self.state.output_channel_programmed = 0;
            self.state.output_channel_disabled = 0;
            self.state.output_channel_open = 0;
            self.state.capture_channel_control_pending = None;
            self.state.capture_channel_selected = None;
            self.state.capture_channel_programmed = 0;
            self.state.capture_channel_disabled = 0;
            self.state.capture_channel_open = 0;
            self.state.registers.retain(|known, _| {
                !Self::channel_register(*known)
                    && *known != OUTPUT_CHANNEL_STATUS
                    && *known != CAPTURE_CHANNEL_STATUS
            });
            self.state.route_ready = false;
            self.state.route_index = None;
        } else if address == 0xfe80d844 {
            if value == 1 && self.state.phase == Phase::ResetPrepared {
                self.start_handshake(cx, true)?;
                self.state.registers.insert(address, value);
                return Ok(());
            } else if value == 1
                && self.state.phase == Phase::EnableRequested
                && self.state.uploaded
                && self.state.route_ready
            {
                self.start_handshake(cx, false)?;
                self.state.registers.insert(address, value);
                return Ok(());
            } else if value == 0 && self.state.phase == Phase::PeriodProgrammed {
                self.state.phase = Phase::ResetReleased;
            } else {
                return Err(format!(
                    "unknown FE80 handshake request/order phase={:?},value={value:#x}",
                    self.state.phase
                ));
            }
        } else if address == 0xfe80d848 {
            if self.state.phase != Phase::ResetAcknowledged
                || !RESET_PERIOD_WORDS.contains(&value)
            {
                return Err("FE80 period/order is outside the recovered stock values".into());
            }
            self.state.phase = Phase::PeriodProgrammed;
        } else if address == 0xfe80d800 {
            if value == 0 && self.state.phase == Phase::ResetReleased {
                self.state.phase = Phase::Disabled;
            } else if value == 0x1ff
                && Self::upload_transport_open(self.state.phase)
                && self.state.upload_phase == UploadPhase::Released
                && self.state.route_ready
            {
                self.state.phase = Phase::EnableRequested;
            } else {
                return Err(format!(
                    "unknown FE80 enable/order phase={:?},value={value:#x}",
                    self.state.phase
                ));
            }
        } else if address == 0xfe80cc20 {
            if value != 0x44444 || self.state.phase != Phase::Disabled {
                return Err("unknown FE80 routing value/order".into());
            }
            self.state.phase = Phase::Routed;
        } else if address == CHANNEL_CONTROL {
            if value == 0x40001
                && Self::upload_transport_open(self.state.phase)
                && self.state.upload_phase == UploadPhase::Idle
            {
                self.state.upload_phase = UploadPhase::Prepared;
            } else if value == 0x40000 && self.state.upload_phase == UploadPhase::ReleaseSet {
                self.state.upload_phase = UploadPhase::Released;
            } else {
                return Err("unknown FE80 program-channel control/order".into());
            }
        } else if address == DESCRIPTOR_POINTER {
            if self.state.upload_phase != UploadPhase::Prepared
                || !Self::upload_transport_open(self.state.phase)
                || value & 3 != 0
            {
                return Err(
                    "FE80 descriptor pointer requires aligned prepared channel setup".into(),
                );
            }
            self.state.upload_phase = UploadPhase::PointerSet;
        } else if address == TRANSFER_CONTROL {
            if value == 1 && self.state.upload_phase == UploadPhase::PointerSet {
                self.start_upload(cx)?;
                self.state.registers.insert(address, value);
                return Ok(());
            } else if value == 0 && self.state.upload_phase == UploadPhase::Acknowledged {
                self.state.upload_phase = UploadPhase::Stopped;
            } else if value == 0x100 && self.state.upload_phase == UploadPhase::Stopped {
                self.state.upload_phase = UploadPhase::ReleaseSet;
            } else {
                return Err("unknown FE80 program transfer command/order".into());
            }
        } else if address == CONTROL_STATUS {
            if value == 0x10
                && self.state.upload_phase == UploadPhase::Completed
                && self.state.registers.get(&address) == Some(&0x10)
            {
                self.state.registers.insert(address, 0);
                self.state.upload_phase = UploadPhase::Acknowledged;
                return Ok(());
            } else if let Some(channel) = self.state.output_channel_control_pending {
                if value != (1u32 << channel) {
                    return Err(
                        "FE80 output-channel select does not match its prepared channel".into(),
                    );
                }
                self.state.output_channel_control_pending = None;
                self.state.output_channel_selected = Some(channel);
                return Ok(());
            } else if let Some(channel) = self.state.capture_channel_control_pending {
                if value != (0x100u32 << channel) {
                    return Err(
                        "FE80 capture-channel select does not match its prepared channel".into(),
                    );
                }
                self.state.capture_channel_control_pending = None;
                self.state.capture_channel_selected = Some(channel);
                return Ok(());
            } else {
                return Err("unknown FE80 channel select or program acknowledgment order".into());
            }
        } else if let Some(channel) = Self::output_channel_control(address) {
            let bit = 1u8 << channel;
            if value == 0x40001
                && self.state.output_channel_control_pending.is_none()
                && self.state.output_channel_selected.is_none()
            {
                self.state.output_channel_control_pending = Some(channel);
            } else if value == 0x40101 {
                self.state.output_channel_disabled |= bit;
            } else {
                return Err("unknown FE80 output-channel control/order".into());
            }
        } else if let Some(channel) = Self::output_channel_pointer(address) {
            if self.state.output_channel_selected != Some(channel)
                || value == 0
                || value & 3 != 0
            {
                return Err(
                    "FE80 output-channel address requires its selected channel and alignment"
                        .into(),
                );
            }
            self.state.output_channel_selected = None;
            self.state.output_channel_programmed |= 1u8 << channel;
        } else if let Some(channel) = Self::output_channel_config(address) {
            if value != 0 || self.state.output_channel_disabled & (1u8 << channel) == 0 {
                return Err(
                    "FE80 output-channel disable requires the stock zero configuration".into(),
                );
            }
        } else if let Some(channel) = Self::output_channel_gate(address) {
            if value != 1 || self.state.output_channel_programmed & (1u8 << channel) == 0 {
                return Err("FE80 output-channel gate requires a programmed channel".into());
            }
            self.state.output_channel_open |= 1u8 << channel;
        } else if let Some(channel) = Self::capture_channel_control(address) {
            let bit = 1u8 << channel;
            if value == 0x40001
                && self.state.capture_channel_control_pending.is_none()
                && self.state.capture_channel_selected.is_none()
            {
                self.state.capture_channel_control_pending = Some(channel);
            } else if value == 0x40101 {
                self.state.capture_channel_disabled |= bit;
            } else {
                return Err("unknown FE80 capture-channel control/order".into());
            }
        } else if let Some(channel) = Self::capture_channel_pointer(address) {
            if self.state.capture_channel_selected != Some(channel)
                || value == 0
                || value & 3 != 0
            {
                return Err(
                    "FE80 capture-channel address requires its selected channel and alignment"
                        .into(),
                );
            }
            self.state.capture_channel_selected = None;
            self.state.capture_channel_programmed |= 1u8 << channel;
        } else if let Some(channel) = Self::capture_channel_config(address) {
            if value != 0 || self.state.capture_channel_disabled & (1u8 << channel) == 0 {
                return Err(
                    "FE80 capture-channel disable requires the stock zero configuration".into(),
                );
            }
        } else if let Some(channel) = Self::capture_channel_gate(address) {
            if value != 1 || self.state.capture_channel_programmed & (1u8 << channel) == 0 {
                return Err("FE80 capture-channel gate requires a programmed channel".into());
            }
            self.state.capture_channel_open |= 1u8 << channel;
        } else if address == OUTPUT_CHANNEL_STATUS || address == CAPTURE_CHANNEL_STATUS {
            let current = self.state.registers.get(&address).copied().unwrap_or(0);
            if value != current {
                return Err(
                    "FE80 channel status accepts only the observed read-modify-write value".into(),
                );
            }
        } else if address == 0xfe3b0000 {
            if value != 7 || self.state.upload_phase != UploadPhase::Released {
                return Err("unknown companion route setup/order".into());
            }
            self.state.route_index = Some(0);
            self.state.route_ready = false;
        } else if ROUTE_SEQUENCE.iter().any(|(known, _)| *known == address) {
            if address == ROUTE_SEQUENCE[0].0
                && value == ROUTE_SEQUENCE[0].1
                && self.state.route_ready
                && self.state.upload_phase == UploadPhase::Released
            {
                self.state.route_index = Some(0);
                self.state.route_ready = false;
            }
            let Some(index) = self.state.route_index.map(usize::from) else {
                return Err("FE80 companion route sequence has not started".into());
            };
            if index >= ROUTE_SEQUENCE.len() || ROUTE_SEQUENCE[index] != (address, value) {
                return Err("unknown companion reset/setup order or value".into());
            }
            self.state.route_index = Some((index + 1) as u8);
            self.state.route_ready = index + 1 == ROUTE_SEQUENCE.len();
        } else {
            return Err(format!("unknown FE80 write {address:#x}"));
        }
        self.state.registers.insert(address, value);
        Ok(())
    }
}

impl Default for ProgramUpload {
    fn default() -> Self {
        Self::new(Options::default()).expect("default FE80 options are valid")
    }
}

impl Device for ProgramUpload {
    fujimod_emulator_devices::checkpoint_fields!(state: State);

    fn fingerprint(&self) -> std::result::Result<serde_json::Value, String> {
        Ok(self.save())
    }

    fn name(&self) -> &'static str {
        "exr2-fe80-program-upload"
    }

    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        if !Self::known(address, size) {
            cx.unmodeled();
            return 0;
        }
        if self.state.owner.is_some_and(|owner| owner != cx.cpu) {
            cx.fail("cross-CPU FE80 setup transaction is unvalidated");
            return 0;
        }
        self.advance(cx);
        if address != STATUS
            && address != CONTROL_STATUS
            && address != OUTPUT_CHANNEL_STATUS
            && address != CAPTURE_CHANNEL_STATUS
        {
            cx.fail(format!(
                "unproved FE80 controller register read {address:#x}"
            ));
            return 0;
        }
        u64::from(self.state.registers.get(&address).copied().unwrap_or(0))
    }

    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        if !Self::known(address, size) {
            cx.unmodeled();
            return;
        }
        if value > u64::from(u32::MAX) {
            cx.fail("FE80 register value exceeds 32 bits");
            return;
        }
        self.advance(cx);
        if self.state.owner.is_some_and(|owner| owner != cx.cpu) {
            cx.fail("cross-CPU FE80 setup transaction is unvalidated");
            return;
        }
        if let Err(error) = self.write_register(cx, address, value as u32) {
            cx.fail(error);
        }
    }

    fn next_deadline(&self, _now: u64) -> Option<u64> {
        self.state.deadline
    }

    fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
        if token == self.state.generation {
            self.advance(cx);
        }
    }

    fn save(&self) -> Value {
        json!({"state": self.state, "options": self.options})
    }

    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value["state"].clone())
            .map_err(|error| format!("invalid FE80 upload checkpoint: {error}"))?;
        let options: Options = serde_json::from_value(value["options"].clone())
            .map_err(|error| format!("invalid FE80 upload options: {error}"))?;
        if options.handshake_ticks != self.options.handshake_ticks
            || options.upload_bytes_per_tick != self.options.upload_bytes_per_tick
            || !Self::valid_state(&state)
        {
            return Err("invalid FE80 upload checkpoint configuration or state".into());
        }
        self.state = state;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::{Host, Record};

    const RAM_BASE: u64 = 0x4000_0000;
    struct Clock {
        now: u64,
        ram: Vec<u8>,
        deadlines: Vec<(usize, u64, u64)>,
    }

    impl Default for Clock {
        fn default() -> Self {
            Self {
                now: 0,
                ram: vec![0; 4096],
                deadlines: Vec::new(),
            }
        }
    }

    impl Host for Clock {
        fn now(&self) -> u64 {
            self.now
        }
        fn schedule(&mut self, device: usize, at: u64, token: u64) {
            self.deadlines.push((device, at, token));
        }
        fn set_irq(&mut self, _: u32, _: bool) {
            unreachable!()
        }
        fn raise_irq(&mut self, _: usize, _: u32) -> Result<(), String> {
            unreachable!()
        }
        fn ram_read(&self, address: u64, out: &mut [u8]) -> Result<(), String> {
            let start = usize::try_from(
                address
                    .checked_sub(RAM_BASE)
                    .ok_or("RAM address below test RAM")?,
            )
            .map_err(|_| "RAM address overflow")?;
            let end = start.checked_add(out.len()).ok_or("RAM range overflow")?;
            out.copy_from_slice(
                self.ram
                    .get(start..end)
                    .ok_or("RAM range outside test RAM")?,
            );
            Ok(())
        }
        fn ram_write(&mut self, address: u64, bytes: &[u8]) -> Result<(), String> {
            let start = usize::try_from(
                address
                    .checked_sub(RAM_BASE)
                    .ok_or("RAM address below test RAM")?,
            )
            .map_err(|_| "RAM address overflow")?;
            let end = start.checked_add(bytes.len()).ok_or("RAM range overflow")?;
            self.ram
                .get_mut(start..end)
                .ok_or("RAM range outside test RAM")?
                .copy_from_slice(bytes);
            Ok(())
        }
        fn record(&mut self, _: Record) {}
    }

    fn write(
        model: &mut ProgramUpload,
        host: &mut Clock,
        address: u64,
        value: u64,
    ) -> Option<String> {
        let mut cx = Cx::new(host, 0, 0, 3);
        model.write(&mut cx, address, 4, value);
        cx.take_refusal()
    }
    fn read(model: &mut ProgramUpload, host: &mut Clock, address: u64) -> (u64, Option<String>) {
        let mut cx = Cx::new(host, 0, 0, 3);
        let value = model.read(&mut cx, address, 4);
        (value, cx.take_refusal())
    }
    fn run_event(model: &mut ProgramUpload, host: &mut Clock, index: usize) {
        let (_, at, token) = host.deadlines[index];
        host.now = at;
        let mut cx = Cx::new(host, 0, 0, 3);
        model.event(&mut cx, token);
        assert_eq!(cx.take_refusal(), None);
    }
    fn put_descriptor(host: &mut Clock, pointer: u32, source: u32, program: &[u8]) {
        let mut descriptor = Vec::new();
        descriptor.extend_from_slice(&0x0300_0000u32.to_le_bytes());
        descriptor.extend_from_slice(&source.to_le_bytes());
        descriptor.extend_from_slice(&(program.len() as u32).to_le_bytes());
        descriptor.extend_from_slice(&0u32.to_le_bytes());
        host.ram_write(u64::from(pointer), &descriptor).unwrap();
        host.ram_write(u64::from(source), program).unwrap();
    }
    fn setup_disabled_with_period(model: &mut ProgramUpload, host: &mut Clock, period: u32) {
        assert_eq!(write(model, host, 0xfe80d840, 0), None);
        assert_eq!(write(model, host, 0xfe80d844, 1), None);
        run_event(model, host, 0);
        assert_eq!(read(model, host, STATUS), (1, None));
        assert_eq!(write(model, host, 0xfe80d848, u64::from(period)), None);
        assert_eq!(write(model, host, 0xfe80d844, 0), None);
        assert_eq!(write(model, host, 0xfe80d800, 0), None);
    }

    fn setup_disabled(model: &mut ProgramUpload, host: &mut Clock) {
        setup_disabled_with_period(model, host, 0x000b_0000);
    }

    fn setup(model: &mut ProgramUpload, host: &mut Clock) {
        setup_disabled(model, host);
        assert_eq!(write(model, host, 0xfe80cc20, 0x44444), None);
    }

    #[test]
    fn uploads_opaque_descriptor_data_and_completes_each_virtual_handshake() {
        let mut model = ProgramUpload::new(Options {
            handshake_ticks: 2,
            upload_bytes_per_tick: 2,
        })
        .unwrap();
        let mut host = Clock::default();
        let pointer = (RAM_BASE + 0x100) as u32;
        let source = (RAM_BASE + 0x200) as u32;
        let program = [0xe1, 0x7b, 0x00, 0xd4];
        put_descriptor(&mut host, pointer, source, &program);
        setup(&mut model, &mut host);
        assert_eq!(write(&mut model, &mut host, CHANNEL_CONTROL, 0x40001), None);
        assert_eq!(
            write(
                &mut model,
                &mut host,
                DESCRIPTOR_POINTER,
                u64::from(pointer)
            ),
            None
        );
        assert_eq!(write(&mut model, &mut host, TRANSFER_CONTROL, 1), None);
        assert_eq!(model.next_deadline(host.now), Some(4));
        assert_eq!(read(&mut model, &mut host, CONTROL_STATUS), (0, None));
        run_event(&mut model, &mut host, 1);
        assert_eq!(read(&mut model, &mut host, CONTROL_STATUS), (0x10, None));
        assert_eq!(write(&mut model, &mut host, CONTROL_STATUS, 0x10), None);
        assert_eq!(write(&mut model, &mut host, TRANSFER_CONTROL, 0), None);
        assert_eq!(write(&mut model, &mut host, TRANSFER_CONTROL, 0x100), None);
        assert_eq!(write(&mut model, &mut host, CHANNEL_CONTROL, 0x40000), None);
        assert_eq!(write(&mut model, &mut host, 0xfe3b0000, 7), None);
        for (address, value) in ROUTE_SEQUENCE {
            assert_eq!(
                write(&mut model, &mut host, address, u64::from(value)),
                None
            );
        }
        assert_eq!(write(&mut model, &mut host, 0xfe80d800, 0x1ff), None);
        assert_eq!(write(&mut model, &mut host, 0xfe80d844, 1), None);
        run_event(&mut model, &mut host, 2);
        assert_eq!(read(&mut model, &mut host, STATUS), (1, None));
        assert_eq!(model.state.program, program);
        assert_eq!(model.state.uploads_completed, 1);
    }

    #[test]
    fn uploads_while_disabled_then_routes_before_enable() {
        let mut model = ProgramUpload::new(Options {
            handshake_ticks: 2,
            upload_bytes_per_tick: 2,
        })
        .unwrap();
        let mut host = Clock::default();
        let pointer = (RAM_BASE + 0x100) as u32;
        let source = (RAM_BASE + 0x200) as u32;
        let program = [0x55, 0x1a, 0x09, 0xc3];
        put_descriptor(&mut host, pointer, source, &program);
        setup_disabled(&mut model, &mut host);
        assert_eq!(model.state.phase, Phase::Disabled);

        assert_eq!(write(&mut model, &mut host, CHANNEL_CONTROL, 0x40001), None);
        assert_eq!(
            write(
                &mut model,
                &mut host,
                DESCRIPTOR_POINTER,
                u64::from(pointer)
            ),
            None
        );
        assert_eq!(write(&mut model, &mut host, TRANSFER_CONTROL, 1), None);
        run_event(&mut model, &mut host, 1);
        assert_eq!(read(&mut model, &mut host, CONTROL_STATUS), (0x10, None));
        assert_eq!(write(&mut model, &mut host, CONTROL_STATUS, 0x10), None);
        assert_eq!(write(&mut model, &mut host, TRANSFER_CONTROL, 0), None);
        assert_eq!(write(&mut model, &mut host, TRANSFER_CONTROL, 0x100), None);
        assert_eq!(write(&mut model, &mut host, CHANNEL_CONTROL, 0x40000), None);
        assert_eq!(model.state.phase, Phase::Disabled);

        assert_eq!(write(&mut model, &mut host, 0xfe3b0000, 7), None);
        for (address, value) in ROUTE_SEQUENCE {
            assert_eq!(
                write(&mut model, &mut host, address, u64::from(value)),
                None
            );
        }
        assert_eq!(write(&mut model, &mut host, 0xfe80d800, 0x1ff), None);
        assert_eq!(write(&mut model, &mut host, 0xfe80d844, 1), None);
        run_event(&mut model, &mut host, 2);
        assert_eq!(read(&mut model, &mut host, STATUS), (1, None));
        assert_eq!(model.state.phase, Phase::Enabled);
        assert_eq!(model.state.program, program);
    }

    #[test]
    fn accepts_recovered_eleven_count_reset_period() {
        let mut model = ProgramUpload::default();
        let mut host = Clock::default();
        setup_disabled_with_period(&mut model, &mut host, 0x000a_0000);
        assert_eq!(model.state.phase, Phase::Disabled);
        assert_eq!(model.state.registers.get(&0xfe80d848), Some(&0x000a_0000));
    }

    #[test]
    fn programs_fe80_output_gate_and_disables_the_declared_channel_banks() {
        let mut model = ProgramUpload::default();
        let mut host = Clock::default();
        let descriptor = (RAM_BASE + 0x300) as u32;

        assert!(write(&mut model, &mut host, CONTROL_STATUS, 1).is_some());
        assert_eq!(write(&mut model, &mut host, OUTPUT_CHANNEL_CONTROL, 0x40001), None);
        assert_eq!(write(&mut model, &mut host, CONTROL_STATUS, 1), None);
        assert_eq!(
            write(
                &mut model,
                &mut host,
                OUTPUT_CHANNEL_POINTER,
                u64::from(descriptor)
            ),
            None
        );

        for channel in 1u8..=3 {
            let control = OUTPUT_CHANNEL_CONTROL + u64::from(channel) * CHANNEL_CONTROL_STRIDE;
            let config = OUTPUT_CHANNEL_CONFIG + u64::from(channel) * CHANNEL_CONTROL_STRIDE;
            assert_eq!(write(&mut model, &mut host, control, 0x40101), None);
            assert_eq!(write(&mut model, &mut host, config, 0), None);
        }
        assert_eq!(read(&mut model, &mut host, OUTPUT_CHANNEL_STATUS), (0, None));
        assert_eq!(write(&mut model, &mut host, OUTPUT_CHANNEL_STATUS, 0), None);

        for channel in [0u8, 2, 3] {
            let control = CAPTURE_CHANNEL_CONTROL + u64::from(channel) * CHANNEL_CONTROL_STRIDE;
            let config = CAPTURE_CHANNEL_CONFIG + u64::from(channel) * CHANNEL_CONTROL_STRIDE;
            assert_eq!(write(&mut model, &mut host, control, 0x40101), None);
            assert_eq!(write(&mut model, &mut host, config, 0), None);
        }
        assert_eq!(read(&mut model, &mut host, CAPTURE_CHANNEL_STATUS), (0, None));
        assert_eq!(write(&mut model, &mut host, CAPTURE_CHANNEL_STATUS, 0), None);
        assert_eq!(write(&mut model, &mut host, OUTPUT_CHANNEL_GATE, 1), None);

        assert_eq!(model.state.output_channel_programmed, 1);
        assert_eq!(model.state.output_channel_disabled, 0b1110);
        assert_eq!(model.state.output_channel_open, 1);
        assert_eq!(model.state.capture_channel_disabled, 0b1101);
        assert_eq!(model.state.registers.get(&OUTPUT_CHANNEL_POINTER), Some(&descriptor));
        assert_eq!(model.state.registers.get(&OUTPUT_CHANNEL_GATE), Some(&1));

        let mut restored = ProgramUpload::default();
        restored.restore(&model.save()).unwrap();
        assert_eq!(restored.state.output_channel_programmed, 1);
        assert_eq!(restored.state.output_channel_open, 1);
    }

    #[test]
    fn rejects_out_of_order_access_bad_descriptor_and_invalid_options() {
        let mut model = ProgramUpload::default();
        let mut host = Clock::default();
        assert!(write(&mut model, &mut host, TRANSFER_CONTROL, 1).is_some());
        assert!(read(&mut model, &mut host, 0xfe80d840).1.is_some());
        setup(&mut model, &mut host);
        assert_eq!(write(&mut model, &mut host, CHANNEL_CONTROL, 0x40001), None);
        assert_eq!(
            write(
                &mut model,
                &mut host,
                DESCRIPTOR_POINTER,
                (RAM_BASE + 0x100) as u64
            ),
            None
        );
        assert!(write(&mut model, &mut host, TRANSFER_CONTROL, 1).is_some());
        assert_eq!(model.state.uploads_started, 0);
        assert!(
            ProgramUpload::new(Options {
                handshake_ticks: 0,
                upload_bytes_per_tick: 1
            })
            .is_err()
        );
    }

    #[test]
    fn checkpoint_preserves_pending_bytes_and_exact_guest_deadline() {
        let options = Options {
            handshake_ticks: 3,
            upload_bytes_per_tick: 1,
        };
        let mut model = ProgramUpload::new(options).unwrap();
        let mut host = Clock::default();
        let pointer = (RAM_BASE + 0x100) as u32;
        let source = (RAM_BASE + 0x200) as u32;
        put_descriptor(&mut host, pointer, source, &[0xaa, 0xbb, 0xcc, 0xdd]);
        setup(&mut model, &mut host);
        write(&mut model, &mut host, CHANNEL_CONTROL, 0x40001);
        write(
            &mut model,
            &mut host,
            DESCRIPTOR_POINTER,
            u64::from(pointer),
        );
        write(&mut model, &mut host, TRANSFER_CONTROL, 1);
        assert_eq!(model.next_deadline(host.now), Some(7));
        let saved = model.save();
        let mut restored = ProgramUpload::new(options).unwrap();
        restored.restore(&saved).unwrap();
        assert_eq!(restored.next_deadline(host.now), Some(7));
        assert!(
            restored
                .restore(&json!({"state": {}, "options": {}}))
                .is_err()
        );
    }
}
