//! Passive face-detector configuration and an explicitly selected word bank.
//!
//! Configuration values follow the declared register fields. Per-channel
//! DMA accepts the inactive clear/poll/setup sequence with zeroed
//! initial status as an explicit unmeasured assumption. Detections and DMA
//! payloads remain unmodeled.
//! An ordered central reset and guest-owned local stops can cancel an armed
//! request without creating a result. Local idle status remains the declared
//! retained zero; reset does not manufacture DMA progress. A stopped request
//! may be followed directly by the next one, as in a burst.
//! Declared empty results complete an armed request after the unmeasured
//! result latency: IRQ 47, the ISR acknowledgment at +8F4, the guest's local
//! stops and a zero detection count at +600. No detections are produced.
//! RetainedWords instead exposes all aligned words in the 4 KiB bank, initially
//! zero and freely reconfigurable, without interpreting requests or DMA fields.
use fujimod_emulator_devices::exr2::irq_routes::IrqRoutes;
use fujimod_emulator_devices::{Cx, Device};
use fujimod_emulator_imaging::exr2_isp::options::face_detector::{Mode, Options, Results};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const BASE: u64 = 0xfff14000;
/// Physical completion line; the stock ISR acknowledges it at +8F4.
const IRQ: u32 = 47;
const ACKNOWLEDGE: u16 = 0x8f4;
const RESULT_COUNT: u16 = 0x600;
/// Declared, unmeasured result latency: 4,000,000 ticks (333 ms at 12 MHz).
/// X-E2S still capture changes path when a result precedes its capture wait,
/// about 290 ms after the request in emulation; that earlier path stalls on
/// other unmodeled work. This latency follows the stock path taken when the
/// request is canceled by the firmware's own timeout.
const RESULT_LATENCY: u64 = 4_000_000;

const FIXED_WORDS: &[(u16, u32)] = &[(0x08, 0x01000000), (0x564, 0xffff), (0x57c, 0)];

// Address registers retain bus addresses directly. No table is read and no
// detection is synthesized here.
const ADDRESS_WORDS: &[u16] = &[0x20, 0x24, 0x4c, 0x50, 0x560];

const MODE_WORDS: &[(u16, &[u32])] = &[
    (0x54, &[0x00800100, 0x006800d0]),
    (0x60, &[0x003800bc, 0x002800bc, 0x00880006]),
    (0x64, &[0x01930191, 0x00c90191, 0x01050105]),
    (0x68, &[0x00620004, 0x00690004, 0x002b0097]),
    (0x6c, &[0x01fc01fc, 0x00fd01fc, 0x014a0149]),
    (0x70, &[0x00010000, 0x00200000, 0x00500000]),
    (0x74, &[0x0282027f, 0x0140027f, 0x01a0019f]),
    (0x78, &[0]),
    (0x7c, &[0]),
];

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct State {
    mode: Mode,
    registers: BTreeMap<u16, u32>,
    request: Option<RequestState>,
    dma: [DmaChannel; 4],
    /// Declared result deadline of the armed request.
    due: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum DmaPhase {
    #[default]
    Initial,
    ClearEnable,
    PollIdle,
    PollEnable,
    ClearCommand,
    ConfigRead,
    ConfigWrite,
    ConfigControl,
    ConfigCount,
    ConfigExtra,
    Configured,
    CloseEnable,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum DmaOperation {
    Read,
    Write,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct DmaAccess {
    operation: DmaOperation,
    offset: u8,
    value: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct DmaChannel {
    phase: DmaPhase,
    initial_phase: DmaPhase,
    initial_registers: BTreeMap<u8, u32>,
    registers: BTreeMap<u8, u32>,
    accepted_registers: BTreeMap<u8, u32>,
    accesses: Vec<DmaAccess>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum RequestPhase {
    #[default]
    Start,
    ParameterByte,
    Programming,
    Reconfiguring,
    Arming,
    InterruptSelection,
    CentralStart,
    AwaitingResult,
    ResultRaised,
    ResultStopping,
    ResultCount,
    Resetting,
    Stopping,
    Stopped,
    ClosingReset,
    ClosingChannels,
    Closed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct RequestAccess {
    operation: DmaOperation,
    offset: u16,
    value: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct RequestState {
    phase: RequestPhase,
    step: u16,
    channel_index: u8,
    optional_mask_words: Option<bool>,
    configured_channels: Vec<u8>,
    selected_channel: Option<u8>,
    initial_registers: BTreeMap<u16, u32>,
    initial_dma: [DmaChannel; 4],
    accesses: Vec<RequestAccess>,
    /// Number of guest accesses before the declared result IRQ.
    raised_after: Option<usize>,
}

/// Face detector. Only declared empty results complete a request.
pub struct FaceDetector {
    mode: Mode,
    results: Option<Results>,
    routes: Option<IrqRoutes>,
    state: State,
}

impl FaceDetector {
    pub fn new(options: Options, routes: Option<IrqRoutes>) -> Result<Self, String> {
        if options.results.is_some() && (options.mode != Mode::Configuration || routes.is_none()) {
            return Err("declared face results require configuration mode and IRQ routes".into());
        }
        Ok(Self {
            mode: options.mode,
            results: options.results,
            routes,
            state: State {
                mode: options.mode,
                ..State::default()
            },
        })
    }

    fn offset(&self, address: u64, size: u8) -> Option<u16> {
        if !(BASE..BASE + 0x1000).contains(&address) {
            return None;
        }
        let offset = (address - BASE) as u16;
        if size == 4
            && offset & 3 == 0
            && (self.mode == Mode::RetainedWords
                || offset == 0x14
                || self.results.is_some() && matches!(offset, ACKNOWLEDGE | RESULT_COUNT)
                || Self::is_config_offset(offset)
                || Self::is_request_offset(offset)
                || Self::dma_location(offset).is_some())
        {
            Some(offset)
        } else {
            None
        }
    }

    // Clear bit 0 in the local DMA status register, selected within the
    // channel's 0x80-byte DMA window, then poll until the bit is clear.
    fn dma_location(offset: u16) -> Option<(usize, u8)> {
        if !(0x800..0xa00).contains(&offset) {
            return None;
        }
        let relative = offset - 0x800;
        let local = (relative % 0x80) as u8;
        if !matches!(local, 0 | 4 | 0x0c | 0x10 | 0x20 | 0x28 | 0x44) {
            return None;
        }
        Some((usize::from(relative / 0x80), local))
    }

    fn apply_dma_access(
        channel: &mut DmaChannel,
        operation: DmaOperation,
        offset: u8,
        value: u32,
    ) -> bool {
        let stored = channel.registers.get(&offset).copied().unwrap_or(0);
        let next = match (channel.phase, operation, offset) {
            (DmaPhase::Initial, DmaOperation::Read, 4) if value == 0 && stored == 0 => {
                DmaPhase::ClearEnable
            }
            (DmaPhase::ClearEnable, DmaOperation::Write, 4) if value == 0 => DmaPhase::PollIdle,
            (DmaPhase::PollIdle, DmaOperation::Read, 0x10) if value == 0 && stored == 0 => {
                DmaPhase::PollEnable
            }
            (DmaPhase::PollEnable, DmaOperation::Read, 4) if value == 0 && stored == 0 => {
                DmaPhase::ClearCommand
            }
            (DmaPhase::ClearCommand, DmaOperation::Write, 0) if value == 0 => DmaPhase::ConfigRead,
            (DmaPhase::ConfigRead, DmaOperation::Read, 0x20)
                if value == stored && (stored == 0 || Self::valid_dma_config(stored)) =>
            {
                DmaPhase::ConfigWrite
            }
            (DmaPhase::ConfigWrite, DmaOperation::Write, 0x20) if Self::valid_dma_config(value) => {
                DmaPhase::ConfigControl
            }
            (DmaPhase::ConfigControl, DmaOperation::Write, 4) if value == 0 => {
                DmaPhase::ConfigCount
            }
            (DmaPhase::ConfigCount, DmaOperation::Write, 0x28) if value == 0x100 => {
                DmaPhase::ConfigExtra
            }
            // The idle setup uses the same extra mode word as request setup.
            // Reopening a channel can program 2 even when its retained word is 0.
            (DmaPhase::ConfigExtra, DmaOperation::Write, 0x44) if matches!(value, 0 | 2) => {
                DmaPhase::Configured
            }
            // Reopening an inactive channel polls its retained idle status
            // before repeating the existing clear/configuration sequence.
            (DmaPhase::Configured, DmaOperation::Read, 0x10) if value == 0 && stored == 0 => {
                DmaPhase::PollEnable
            }
            // Closing a configured inactive channel is another read/clear
            // of its retained disabled control word, without a DMA request.
            (DmaPhase::Configured, DmaOperation::Read, 4) if value == 0 && stored == 0 => {
                DmaPhase::CloseEnable
            }
            (DmaPhase::CloseEnable, DmaOperation::Write, 4) if value == 0 => DmaPhase::Configured,
            _ => return false,
        };
        if operation == DmaOperation::Write {
            channel.registers.insert(offset, value);
        }
        channel.phase = next;
        channel.accepted_registers = channel.registers.clone();
        channel.accesses.push(DmaAccess {
            operation,
            offset,
            value,
        });
        true
    }

    fn valid_dma_config(value: u32) -> bool {
        value & !0x00703000 == 0 && value & 0x000fffff == 0x3000 && (value >> 20) & 7 <= 4
    }

    fn valid_idle_dma_registers(registers: &BTreeMap<u8, u32>) -> bool {
        registers.iter().all(|(&offset, &value)| match offset {
            0 | 4 | 0x10 => value == 0,
            0x0c => value == 1,
            0x20 => Self::valid_dma_config(value),
            0x28 => value == 0x100,
            0x44 => matches!(value, 0 | 2),
            _ => false,
        })
    }

    fn valid_dma_channel(state: &DmaChannel) -> bool {
        if !Self::valid_idle_dma_registers(&state.initial_registers) {
            return false;
        }
        if !matches!(
            state.initial_phase,
            DmaPhase::Initial | DmaPhase::Configured
        ) {
            return false;
        }
        let mut replay =
            Self::retired_channel(state.initial_registers.clone(), state.initial_phase);
        for access in &state.accesses {
            if !Self::apply_dma_access(&mut replay, access.operation, access.offset, access.value) {
                return false;
            }
        }
        replay.phase == state.phase
            && replay.registers == state.accepted_registers
            && state.registers == state.accepted_registers
    }

    fn retired_channel(registers: BTreeMap<u8, u32>, phase: DmaPhase) -> DmaChannel {
        DmaChannel {
            phase,
            initial_phase: phase,
            initial_registers: registers.clone(),
            accepted_registers: registers.clone(),
            registers,
            accesses: Vec::new(),
        }
    }

    fn read_dma(&mut self, cx: &mut Cx<'_>, channel: usize, offset: u8) -> u64 {
        let dma = &mut self.state.dma[channel];
        let value = dma.registers.get(&offset).copied().unwrap_or(0);
        if !Self::apply_dma_access(dma, DmaOperation::Read, offset, value) {
            cx.unmodeled();
        }
        u64::from(value)
    }

    fn write_dma(&mut self, cx: &mut Cx<'_>, channel: usize, offset: u8, value: u32) {
        if !Self::apply_dma_access(
            &mut self.state.dma[channel],
            DmaOperation::Write,
            offset,
            value,
        ) {
            cx.unmodeled();
        }
    }

    fn in_range(offset: u16, start: u16, end: u16) -> bool {
        (start..end).contains(&offset)
    }

    fn is_config_offset(offset: u16) -> bool {
        FIXED_WORDS.iter().any(|(known, _)| *known == offset)
            || ADDRESS_WORDS.contains(&offset)
            || MODE_WORDS.iter().any(|(known, _)| *known == offset)
            || matches!(offset, 0x10 | 0x40 | 0x44 | 0x570)
            || (0x84..0x144).contains(&offset) && (offset - 0x84).is_multiple_of(12)
            || (0x88..0x148).contains(&offset) && (offset - 0x88).is_multiple_of(12)
            || (0x80..0x140).contains(&offset) && (offset - 0x80).is_multiple_of(12)
            || Self::in_range(offset, 0x280, 0x3e8)
            || Self::in_range(offset, 0x470, 0x4c0)
            || Self::in_range(offset, 0x4c0, 0x4d8)
            || Self::in_range(offset, 0x4e0, 0x4f8)
            || Self::in_range(offset, 0x500, 0x518)
            || Self::in_range(offset, 0x520, 0x538)
    }

    fn is_request_offset(offset: u16) -> bool {
        matches!(
            offset,
            0x00 | 0x04
                | 0x14
                | 0x48
                | 0x200
                | 0x204
                | 0x208
                | 0x210
                | 0x230
                | 0x250
                | 0x274
                | 0x278
                | 0x574
                | 0x578
                | 0x8f8
        )
    }

    fn configured_for_request(registers: &BTreeMap<u16, u32>, dma: &[DmaChannel; 4]) -> bool {
        let mut offsets = FIXED_WORDS
            .iter()
            .map(|(offset, _)| *offset)
            .collect::<Vec<_>>();
        offsets.extend(ADDRESS_WORDS);
        offsets.extend(MODE_WORDS.iter().map(|(offset, _)| *offset));
        offsets.extend([0x10, 0x40, 0x44, 0x570]);
        offsets.extend((0x80..0x140).step_by(12));
        offsets.extend((0x84..0x140).step_by(12));
        offsets.extend((0x88..0x148).step_by(12));
        for range in [
            0x280..0x3e8,
            0x470..0x4c0,
            0x4c0..0x4d8,
            0x4e0..0x4f8,
            0x500..0x518,
            0x520..0x538,
        ] {
            offsets.extend(range.step_by(4));
        }
        offsets.sort_unstable();
        offsets.dedup();
        offsets.iter().all(|offset| {
            registers
                .get(offset)
                .is_some_and(|value| Self::valid_config(*offset, *value))
        }) && dma.iter().all(|channel| {
            let word = |offset: u8| channel.registers.get(&offset).copied().unwrap_or(0);
            channel.phase == DmaPhase::Configured
                && Self::valid_dma_channel(channel)
                && word(0) == 0
                && word(4) == 0
                && word(0x10) == 0
                && word(0x20) & !0x00700000 == 0x3000
                && (word(0x20) >> 20) & 7 <= 4
                && word(0x28) == 0x100
                && matches!(word(0x44), 0 | 2)
        })
    }

    fn job_tail() -> Vec<u16> {
        let mut offsets = vec![0x210, 0x230, 0x250];
        for index in 0..10 {
            for field in [0, 4, 8, 16, 20] {
                offsets.push(0x280 + 24 * index + field);
            }
        }
        offsets.extend([0x574, 0x200, 0x578, 0x48]);
        offsets
    }

    fn dma_word(dma: &[DmaChannel; 4], channel: usize, offset: u8) -> u32 {
        dma[channel].registers.get(&offset).copied().unwrap_or(0)
    }

    fn apply_request_access(
        request: &mut RequestState,
        registers: &mut BTreeMap<u16, u32>,
        dma: &mut [DmaChannel; 4],
        operation: DmaOperation,
        offset: u16,
        value: u32,
    ) -> bool {
        match request.phase {
            RequestPhase::Start => {
                if operation != DmaOperation::Write || offset != 0x14 || value >= 0x1000 {
                    return false;
                }
                registers.insert(offset, value);
                request.phase = RequestPhase::ParameterByte;
            }
            RequestPhase::ParameterByte => {
                if operation != DmaOperation::Write || offset != 0x570 || value > 0x1f {
                    return false;
                }
                registers.insert(offset, value);
                request.phase = RequestPhase::Programming;
                request.step = 0;
            }
            RequestPhase::Programming => {
                if operation != DmaOperation::Write {
                    return false;
                }
                let tail = Self::job_tail();
                let next_step = match request.step {
                    0 if offset == 0x274 => Some(1),
                    1 if offset == 0x278 => Some(2),
                    2 if offset == 0x204 => {
                        request.optional_mask_words = Some(true);
                        Some(3)
                    }
                    2 if offset == 0x210 => {
                        request.optional_mask_words = Some(false);
                        Some(3)
                    }
                    3 if request.optional_mask_words == Some(true) && offset == 0x208 => Some(4),
                    3 if request.optional_mask_words == Some(false)
                        && tail.get(1) == Some(&offset) =>
                    {
                        Some(4)
                    }
                    step if step >= 4 => {
                        let index = if request.optional_mask_words == Some(true) {
                            usize::from(step - 4)
                        } else if request.optional_mask_words == Some(false) {
                            usize::from(step - 2)
                        } else {
                            return false;
                        };
                        if tail.get(index) == Some(&offset) {
                            if index + 1 == tail.len() {
                                request.phase = RequestPhase::Reconfiguring;
                                request.step = 0;
                                request.channel_index = 0;
                                request.selected_channel = None;
                                registers.insert(offset, value);
                                request.accesses.push(RequestAccess {
                                    operation,
                                    offset,
                                    value,
                                });
                                return true;
                            }
                            Some(step + 1)
                        } else {
                            None
                        }
                    }
                    _ => None,
                };
                let Some(next_step) = next_step else {
                    return false;
                };
                if (offset == 0x48 && !crate::valid_image_source_address(value))
                    || (offset == 0x578 && value >> 16 != value & 0xffff)
                {
                    return false;
                }
                registers.insert(offset, value);
                request.step = next_step;
            }
            RequestPhase::Reconfiguring => {
                if request.step == 0 {
                    let Some((channel, local)) = Self::dma_location(offset) else {
                        return false;
                    };
                    if operation != DmaOperation::Read
                        || local != 0x20
                        || dma[channel].phase != DmaPhase::Configured
                        || Self::dma_word(dma, channel, 4) != 0
                        || Self::dma_word(dma, channel, 0x20) != value
                    {
                        return false;
                    }
                    request.selected_channel = Some(channel as u8);
                    request.step = 1;
                } else {
                    let Some(channel) = request.selected_channel.map(usize::from) else {
                        return false;
                    };
                    let base = 0x800 + channel as u16 * 0x80;
                    let local = match request.step {
                        1 => 0x20,
                        2 => 4,
                        3 => 0x28,
                        4 => 0x44,
                        _ => return false,
                    };
                    if operation != DmaOperation::Write || offset != base + local {
                        return false;
                    }
                    let old = Self::dma_word(dma, channel, 0x20);
                    let accepted = match request.step {
                        1 => {
                            value & !0x00700000 == ((old | 0x3000) & 0x8f8fffff) & !0x00700000
                                && (value >> 20) & 7 <= 4
                        }
                        2 => value == 0,
                        3 => value == 0x100,
                        4 => value == 2,
                        _ => false,
                    };
                    if !accepted {
                        return false;
                    }
                    dma[channel].registers.insert(local as u8, value);
                    request.step += 1;
                    if request.step == 5 {
                        request.configured_channels.push(channel as u8);
                        request.selected_channel = None;
                        request.step = 0;
                        request.channel_index += 1;
                        if request.channel_index == 4 {
                            request.phase = RequestPhase::Arming;
                            request.channel_index = 0;
                        }
                    }
                }
            }
            RequestPhase::Arming => {
                const ARM_ORDER: [usize; 4] = [1, 3, 0, 2];
                let Some(&channel) = ARM_ORDER.get(usize::from(request.channel_index)) else {
                    return false;
                };
                let base = 0x800 + channel as u16 * 0x80;
                let expected = match request.step {
                    0 => (DmaOperation::Write, base + 0x0c, 1),
                    1 => (
                        DmaOperation::Read,
                        base + 0x20,
                        Self::dma_word(dma, channel, 0x20),
                    ),
                    2 => (
                        DmaOperation::Write,
                        base + 0x20,
                        Self::dma_word(dma, channel, 0x20) & !0x10000000,
                    ),
                    3 => (
                        DmaOperation::Read,
                        base + 4,
                        Self::dma_word(dma, channel, 4),
                    ),
                    4 => (DmaOperation::Write, base + 4, 1),
                    _ => return false,
                };
                if (operation, offset, value) != expected {
                    return false;
                }
                if operation == DmaOperation::Write {
                    let local = (offset - base) as u8;
                    dma[channel].registers.insert(local, value);
                }
                request.step += 1;
                if request.step == 5 {
                    request.step = 0;
                    request.channel_index += 1;
                    if request.channel_index == 4 {
                        request.phase = RequestPhase::InterruptSelection;
                        request.channel_index = 0;
                    }
                }
            }
            RequestPhase::InterruptSelection => {
                if operation != DmaOperation::Write || offset != 0x8f8 || value != 0x10000000 {
                    return false;
                }
                registers.insert(offset, value);
                request.phase = RequestPhase::CentralStart;
            }
            RequestPhase::CentralStart => {
                if operation != DmaOperation::Write || offset != 4 || value != 1 {
                    return false;
                }
                registers.insert(offset, value);
                request.phase = RequestPhase::AwaitingResult;
            }
            RequestPhase::ResultRaised => {
                if operation != DmaOperation::Write || offset != ACKNOWLEDGE || value != 1 {
                    return false;
                }
                request.phase = RequestPhase::ResultStopping;
                request.step = 0;
                request.channel_index = 0;
            }
            RequestPhase::ResultCount => {
                if operation != DmaOperation::Read || offset != RESULT_COUNT || value != 0 {
                    return false;
                }
                request.phase = RequestPhase::Stopped;
            }
            RequestPhase::AwaitingResult | RequestPhase::Stopped => {
                if operation != DmaOperation::Write || offset != 0 || value != 0 {
                    return false;
                }
                registers.insert(0, 0);
                request.phase = if request.phase == RequestPhase::AwaitingResult {
                    RequestPhase::Resetting
                } else {
                    RequestPhase::ClosingReset
                };
            }
            RequestPhase::Resetting | RequestPhase::ClosingReset => {
                if operation != DmaOperation::Write || offset != 0 || value != 1 {
                    return false;
                }
                registers.insert(0, 1);
                request.phase = if request.phase == RequestPhase::Resetting {
                    RequestPhase::Stopping
                } else {
                    RequestPhase::ClosingChannels
                };
                request.step = 0;
                request.channel_index = 0;
            }
            // The completion collector stops channels with the same local
            // sequence as a cancellation, without the central reset.
            RequestPhase::Stopping
            | RequestPhase::ClosingChannels
            | RequestPhase::ResultStopping => {
                const ORDER: [usize; 4] = [1, 3, 0, 2];
                let Some(&channel) = ORDER.get(usize::from(request.channel_index)) else {
                    return false;
                };
                let base = 0x800 + channel as u16 * 0x80;
                let control = Self::dma_word(dma, channel, 4);
                let config = Self::dma_word(dma, channel, 0x20);
                let closing = request.phase == RequestPhase::ClosingChannels;
                let expected = if closing {
                    match request.step {
                        0 => (DmaOperation::Read, base + 4, control),
                        1 => (DmaOperation::Write, base + 4, control & !1),
                        _ => return false,
                    }
                } else {
                    match request.step {
                        0 => (DmaOperation::Write, base + 0x0c, 1),
                        1 => (DmaOperation::Read, base + 4, control),
                        2 => (DmaOperation::Write, base + 4, control & !1),
                        3 => (DmaOperation::Read, base + 0x10, 0),
                        4 => (DmaOperation::Read, base + 0x20, config),
                        5 => (DmaOperation::Write, base + 0x20, config & !0x10000000),
                        _ => return false,
                    }
                };
                if (operation, offset, value) != expected
                    || (request.step == 3 && Self::dma_word(dma, channel, 0x10) != 0)
                {
                    return false;
                }
                if operation == DmaOperation::Write {
                    dma[channel].registers.insert((offset - base) as u8, value);
                }
                request.step += 1;
                if request.step == if closing { 2 } else { 6 } {
                    request.step = 0;
                    request.channel_index += 1;
                    if request.channel_index == 4 {
                        request.channel_index = 0;
                        request.phase = match request.phase {
                            RequestPhase::ClosingChannels => RequestPhase::Closed,
                            RequestPhase::ResultStopping => RequestPhase::ResultCount,
                            _ => RequestPhase::Stopped,
                        };
                    }
                }
            }
            RequestPhase::Closed => {
                if request.step == 0 {
                    if operation != DmaOperation::Write
                        || offset != 0x10
                        || !Self::valid_config(offset, value)
                        || dma
                            .iter()
                            .any(|channel| !Self::valid_idle_dma_registers(&channel.registers))
                    {
                        return false;
                    }
                    // A new configuration retires the closed transaction.
                    // Keep the real idle register values, including DMA setup;
                    // only the bounded transaction observers begin again.
                    for channel in dma.iter_mut() {
                        *channel =
                            Self::retired_channel(channel.registers.clone(), DmaPhase::Initial);
                    }
                    request.step = 1;
                    registers.insert(offset, value);
                } else if operation == DmaOperation::Write && offset == 0 {
                    // A stopped, closed request may be reset before another
                    // configuration. Preserve the ordered 0/1 control words
                    // and the access trace; this supplies no detector result.
                    if dma
                        .iter()
                        .any(|channel| !Self::valid_idle_dma_registers(&channel.registers))
                        || value != 0 && !(value == 1 && registers.get(&0) == Some(&0))
                    {
                        return false;
                    }
                    registers.insert(0, value);
                } else if let Some((channel, local)) = Self::dma_location(offset) {
                    if !Self::apply_dma_access(&mut dma[channel], operation, local, value) {
                        return false;
                    }
                } else if operation == DmaOperation::Write
                    && Self::is_config_offset(offset)
                    && Self::valid_config(offset, value)
                {
                    registers.insert(offset, value);
                } else {
                    return false;
                }
            }
        }
        request.accesses.push(RequestAccess {
            operation,
            offset,
            value,
        });
        true
    }

    fn valid_request(
        request: &RequestState,
        registers: &BTreeMap<u16, u32>,
        dma: &[DmaChannel; 4],
    ) -> bool {
        if !Self::configured_for_request(&request.initial_registers, &request.initial_dma)
            || request.initial_registers.iter().any(|(offset, value)| {
                !((*offset == 0 && *value <= 1)
                    || Self::is_config_offset(*offset) && Self::valid_config(*offset, *value)
                    || Self::is_request_offset(*offset)
                        && request
                            .initial_dma
                            .iter()
                            .all(|channel| !channel.initial_registers.is_empty())
                        && match *offset {
                            0x04 => *value == 1,
                            0x14 => *value < 0x1000,
                            0x8f8 => *value == 0x10000000,
                            _ => true,
                        })
            })
            || request.initial_dma.iter().any(|channel| {
                !Self::valid_dma_channel(channel) || channel.registers != channel.accepted_registers
            })
        {
            return false;
        }
        let mut replay = request.clone();
        replay.phase = RequestPhase::Start;
        replay.step = 0;
        replay.channel_index = 0;
        replay.optional_mask_words = None;
        replay.configured_channels.clear();
        replay.selected_channel = None;
        replay.accesses.clear();
        let mut replay_registers = request.initial_registers.clone();
        let mut replay_dma = request.initial_dma.clone();
        let raise = |replay: &mut RequestState| {
            if request.raised_after == Some(replay.accesses.len())
                && replay.phase == RequestPhase::AwaitingResult
            {
                replay.phase = RequestPhase::ResultRaised;
            }
        };
        for access in &request.accesses {
            raise(&mut replay);
            if !Self::apply_request_access(
                &mut replay,
                &mut replay_registers,
                &mut replay_dma,
                access.operation,
                access.offset,
                access.value,
            ) {
                return false;
            }
        }
        raise(&mut replay);
        request
            .raised_after
            .is_none_or(|count| count <= request.accesses.len())
            && replay.phase == request.phase
            && replay.step == request.step
            && replay.channel_index == request.channel_index
            && replay.optional_mask_words == request.optional_mask_words
            && replay.configured_channels == request.configured_channels
            && replay.selected_channel == request.selected_channel
            && replay.accesses == request.accesses
            && replay_registers == *registers
            && replay_dma == *dma
    }

    fn valid_config(offset: u16, value: u32) -> bool {
        if ADDRESS_WORDS.contains(&offset) {
            return value & 3 == 0;
        }
        if let Some((_, expected)) = FIXED_WORDS.iter().find(|(known, _)| *known == offset) {
            return value == *expected;
        }
        if let Some((_, allowed)) = MODE_WORDS.iter().find(|(known, _)| *known == offset) {
            return allowed.contains(&value);
        }
        match offset {
            0x10 => {
                value & !0x07000777 == 0
                    && [0, 4, 8, 24]
                        .into_iter()
                        .all(|shift| (value >> shift) & 7 <= 4)
            }
            0x570 => value <= 0xff,
            0x84..=0x13c if (offset - 0x84).is_multiple_of(12) => value & !0x03ffffff == 0,
            0x88..=0x140 if (offset - 0x88).is_multiple_of(12) => {
                value & 3 == 0 && (0x40000000..0x60000000).contains(&value)
            }
            _ => Self::is_config_offset(offset),
        }
    }

    fn request_access(
        &mut self,
        cx: &mut Cx<'_>,
        operation: DmaOperation,
        offset: u16,
        value: Option<u32>,
    ) -> u64 {
        let value = match (operation, value) {
            (DmaOperation::Write, Some(value)) => value,
            (DmaOperation::Read, None) => {
                if let Some((channel, local)) = Self::dma_location(offset) {
                    Self::dma_word(&self.state.dma, channel, local)
                } else {
                    self.state.registers.get(&offset).copied().unwrap_or(0)
                }
            }
            _ => {
                cx.fail("face detector request access is missing its value");
                return 0;
            }
        };
        let accepted = self.state.request.as_mut().is_some_and(|request| {
            Self::apply_request_access(
                request,
                &mut self.state.registers,
                &mut self.state.dma,
                operation,
                offset,
                value,
            )
        });
        if !accepted {
            let context = self
                .state
                .request
                .as_ref()
                .map(|request| {
                    format!(
                        " in {:?} step {} channel {}",
                        request.phase, request.step, request.channel_index
                    )
                })
                .unwrap_or_default();
            let dma_context = Self::dma_location(offset)
                .map(|(channel, local)| {
                    let state = &self.state.dma[channel];
                    format!(
                        " (dma channel {channel} register {local:#x} phase {:?} stored {:#x})",
                        state.phase,
                        state.registers.get(&local).copied().unwrap_or(0)
                    )
                })
                .unwrap_or_default();
            cx.fail(format!(
                "face detector request {:?} at {offset:#x} value {value:#x}{context}{dma_context} exceeds the modeled submission sequence",
                operation
            ));
            return 0;
        }
        u64::from(value)
    }

    // An armed request is due once; any other phase has no deadline.
    fn schedule_result(&mut self, cx: &Cx<'_>) {
        let awaiting = self.state.request.as_ref().is_some_and(|request| {
            request.phase == RequestPhase::AwaitingResult && request.raised_after.is_none()
        });
        self.state.due = match (self.results, awaiting) {
            (Some(Results::Empty), true) => self
                .state
                .due
                .or_else(|| cx.now().checked_add(RESULT_LATENCY)),
            _ => None,
        };
    }

    fn valid_state(&self, state: &State) -> bool {
        if state.mode != self.mode {
            return false;
        }
        if self.mode == Mode::RetainedWords {
            return state.request.is_none()
                && state.due.is_none()
                && state
                    .dma
                    .iter()
                    .all(|channel| *channel == DmaChannel::default())
                && state
                    .registers
                    .keys()
                    .all(|offset| *offset < 0x1000 && offset & 3 == 0);
        }
        let registers_valid = state.registers.iter().all(|(offset, value)| {
            (*offset == 0 && *value <= 1)
                || (Self::is_config_offset(*offset) && Self::valid_config(*offset, *value))
                || (Self::is_request_offset(*offset) && state.request.is_some())
        });
        // A declared deadline belongs to exactly one armed, unraised request.
        let awaiting = state.request.as_ref().is_some_and(|request| {
            request.phase == RequestPhase::AwaitingResult && request.raised_after.is_none()
        });
        let results_valid = if self.results.is_some() {
            state.due.is_some() == awaiting
        } else {
            state.due.is_none()
                && state
                    .request
                    .as_ref()
                    .is_none_or(|request| request.raised_after.is_none())
        };
        registers_valid
            && results_valid
            && if let Some(request) = &state.request {
                Self::valid_request(request, &state.registers, &state.dma)
            } else {
                state.dma.iter().all(|channel| {
                    channel.initial_phase == DmaPhase::Initial && Self::valid_dma_channel(channel)
                })
            }
    }
}

impl Default for FaceDetector {
    fn default() -> Self {
        Self::new(Options::default(), None).expect("default face detector options")
    }
}

impl Device for FaceDetector {
    fujimod_emulator_devices::checkpoint_fields!(state: State);

    fn fingerprint(&self) -> std::result::Result<serde_json::Value, String> {
        Ok(self.save())
    }

    fn name(&self) -> &'static str {
        "exr2-face-detector"
    }

    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let Some(offset) = self.offset(address, size) else {
            cx.unmodeled();
            return 0;
        };
        if self.mode == Mode::RetainedWords {
            return u64::from(self.state.registers.get(&offset).copied().unwrap_or(0));
        }
        if self.state.request.is_some() {
            return self.request_access(cx, DmaOperation::Read, offset, None);
        }
        if let Some((channel, local)) = Self::dma_location(offset) {
            return self.read_dma(cx, channel, local);
        }
        cx.unmodeled();
        0
    }

    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Some(offset) = self.offset(address, size) else {
            cx.unmodeled();
            return;
        };
        if value > u64::from(u32::MAX) {
            cx.fail("face detector word exceeds 32 bits");
            return;
        }
        let value = value as u32;
        if self.mode == Mode::RetainedWords {
            self.state.registers.insert(offset, value);
            return;
        }
        if offset == 0 && self.state.request.is_none() {
            if value == 0 || value == 1 && self.state.registers.get(&0) == Some(&0) {
                self.state.registers.insert(0, value);
            } else {
                cx.fail("face reset requires ordered control words 0 then 1");
            }
            return;
        }
        if offset == 0x14 {
            if value >= 0x1000 {
                cx.fail("face detector request must fit its 12-bit input");
                return;
            }
            let mut dma = self.state.dma.clone();
            match self.state.request.as_ref() {
                None => {}
                Some(request) if request.phase == RequestPhase::Closed && request.step == 1 => {}
                // A burst follows the ordered reset and local stops with the
                // next request directly. The stopped channels keep their
                // programmed idle words, so they stay configured without the
                // close and reconfiguration a single request performs.
                Some(request) if request.phase == RequestPhase::Stopped => {
                    if dma
                        .iter()
                        .any(|channel| !Self::valid_idle_dma_registers(&channel.registers))
                    {
                        cx.fail(
                            "face detector request requires stopped DMA channels with idle words",
                        );
                        return;
                    }
                    for channel in dma.iter_mut() {
                        *channel =
                            Self::retired_channel(channel.registers.clone(), DmaPhase::Configured);
                    }
                }
                Some(_) => {
                    cx.fail("face detector request is already retained without a modeled result");
                    return;
                }
            }
            if !Self::configured_for_request(&self.state.registers, &dma) {
                cx.fail("face detector request requires complete configuration and four inactive DMA channels");
                return;
            }
            let mut request = RequestState {
                initial_registers: self.state.registers.clone(),
                initial_dma: dma.clone(),
                ..RequestState::default()
            };
            if !Self::apply_request_access(
                &mut request,
                &mut self.state.registers,
                &mut dma,
                DmaOperation::Write,
                offset,
                value,
            ) {
                cx.fail("face detector request is outside the supported input domain");
                return;
            }
            self.state.dma = dma;
            self.state.request = Some(request);
            self.schedule_result(cx);
            return;
        }
        if self.state.request.is_some() {
            self.request_access(cx, DmaOperation::Write, offset, Some(value));
            self.schedule_result(cx);
            return;
        }
        if let Some((channel, local)) = Self::dma_location(offset) {
            self.write_dma(cx, channel, local, value);
            return;
        }
        if !Self::valid_config(offset, value) {
            cx.fail(format!(
                "face configuration at {:#010x} rejects {value:#010x}",
                BASE + u64::from(offset)
            ));
            return;
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
        let Some(routes) = &self.routes else {
            cx.fail("declared face results require IRQ routes");
            return;
        };
        if let Err(error) = routes.latch(IRQ).and_then(|irq| irq.raise(cx)) {
            cx.fail(error);
            return;
        }
        let request = self.state.request.as_mut().expect("due face request");
        request.raised_after = Some(request.accesses.len());
        request.phase = RequestPhase::ResultRaised;
        self.state.due = None;
    }

    fn save(&self) -> Value {
        serde_json::to_value(&self.state).expect("face detector state serializes")
    }

    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value.clone())
            .map_err(|error| format!("invalid face detector state: {error}"))?;
        if !self.valid_state(&state) {
            return Err("invalid face detector state".into());
        }
        self.state = state;
        Ok(())
    }
}

#[cfg(test)]
mod retained_words_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::{Host, Record};

    struct Clock;

    impl Host for Clock {
        fn now(&self) -> u64 {
            0
        }
        fn schedule(&mut self, _: usize, _: u64, _: u64) {
            unreachable!()
        }
        fn set_irq(&mut self, _: u32, _: bool) {
            unreachable!()
        }
        fn raise_irq(&mut self, _: usize, _: u32) -> Result<(), String> {
            unreachable!()
        }
        fn ram_read(&self, _: u64, _: &mut [u8]) -> Result<(), String> {
            unreachable!()
        }
        fn ram_write(&mut self, _: u64, _: &[u8]) -> Result<(), String> {
            unreachable!()
        }
        fn record(&mut self, _: Record) {}
    }

    fn write(model: &mut FaceDetector, offset: u64, value: u64) -> Option<String> {
        let mut host = Clock;
        let mut cx = Cx::new(&mut host, 0, 0, 4);
        model.write(&mut cx, BASE + offset, 4, value);
        cx.take_refusal()
    }

    fn read(model: &mut FaceDetector, offset: u64) -> Option<String> {
        let mut host = Clock;
        let mut cx = Cx::new(&mut host, 0, 0, 4);
        model.read(&mut cx, BASE + offset, 4);
        cx.take_refusal()
    }

    fn initialize(model: &mut FaceDetector, dma_extra: u64) {
        let mut offsets = FIXED_WORDS
            .iter()
            .map(|(offset, _)| *offset)
            .chain(ADDRESS_WORDS.iter().copied())
            .chain(MODE_WORDS.iter().map(|(offset, _)| *offset))
            .chain([0x10, 0x40, 0x44, 0x570])
            .chain((0x80..0x140).step_by(12))
            .chain((0x84..0x140).step_by(12))
            .chain((0x88..0x148).step_by(12))
            .collect::<Vec<_>>();
        for range in [
            0x280..0x3e8,
            0x470..0x4c0,
            0x4c0..0x4d8,
            0x4e0..0x4f8,
            0x500..0x518,
            0x520..0x538,
        ] {
            offsets.extend(range.step_by(4));
        }
        offsets.sort_unstable();
        offsets.dedup();
        for offset in offsets {
            let value = FIXED_WORDS
                .iter()
                .find_map(|(known, value)| (*known == offset).then_some(*value))
                .or_else(|| {
                    MODE_WORDS
                        .iter()
                        .find_map(|(known, values)| (*known == offset).then_some(values[0]))
                })
                .unwrap_or_else(|| match offset {
                    0x20 => 0x014721d8,
                    0x24 => 0x014731d8,
                    0x4c => 0x4275c000,
                    0x50 => 0x42765800,
                    0x560 => 0x4276a400,
                    0x84..=0x13c if (offset - 0x84) % 12 == 0 => 0x3000,
                    0x88..=0x140 if (offset - 0x88) % 12 == 0 => 0x40000000,
                    _ => 0,
                });
            assert_eq!(write(model, u64::from(offset), u64::from(value)), None);
        }
        for channel in 0..4u64 {
            let base = 0x800 + 0x80 * channel;
            assert_eq!(read(model, base + 4), None);
            assert_eq!(write(model, base + 4, 0), None);
            assert_eq!(read(model, base + 0x10), None);
            assert_eq!(read(model, base + 4), None);
            assert_eq!(write(model, base, 0), None);
            assert_eq!(read(model, base + 0x20), None);
            assert_eq!(write(model, base + 0x20, 0x3000), None);
            assert_eq!(write(model, base + 4, 0), None);
            assert_eq!(write(model, base + 0x28, 0x100), None);
            assert_eq!(write(model, base + 0x44, dma_extra), None);
        }
    }

    #[test]
    fn retains_bounded_configuration_without_exposing_unmodeled_reads() {
        let mut model = FaceDetector::default();
        assert_eq!(write(&mut model, 0x08, 0x01000000), None);
        assert_eq!(write(&mut model, 0x20, 0x014721d8), None);
        assert_eq!(write(&mut model, 0x84, 0x03ffffff), None);
        assert_eq!(write(&mut model, 0x88, 0x40000000), None);
        assert!(write(&mut model, 0x08, 0).is_some());
        assert!(write(&mut model, 0x84, 0x04000000).is_some());
        assert!(write(&mut model, 0x88, 0x40000002).is_some());
        assert!(read(&mut model, 0x08).is_some());
        assert_eq!(model.state.registers[&0x08], 0x01000000);
    }

    #[test]
    fn table_and_scratch_addresses_are_data_instead_of_firmware_signatures() {
        let mut model = FaceDetector::default();
        for &offset in ADDRESS_WORDS {
            for address in [0, 0x01234560, 0x41000000, 0x4f000004] {
                assert_eq!(write(&mut model, u64::from(offset), address), None);
                assert_eq!(model.state.registers[&offset], address as u32);
            }
            assert!(write(&mut model, u64::from(offset), 0x41000002).is_some());
        }
        assert!(model.state.request.is_none());
        let saved = model.save();
        let mut restored = FaceDetector::default();
        restored.restore(&saved).unwrap();
        assert_eq!(restored.save(), saved);
    }

    #[test]
    fn inactive_dma_channel_clear_and_poll_observes_declared_register_behavior() {
        let mut model = FaceDetector::default();
        assert_eq!(read(&mut model, 0x884), None);
        assert_eq!(write(&mut model, 0x884, 0), None);
        assert_eq!(read(&mut model, 0x890), None);
        assert_eq!(read(&mut model, 0x884), None);
        assert_eq!(model.state.dma[1].phase, DmaPhase::ClearCommand);

        let saved = model.save();
        let mut restored = FaceDetector::default();
        restored.restore(&saved).unwrap();
        assert!(read(&mut restored, 0x884).is_some());

        let mut corrupted = saved;
        corrupted["dma"][1]["phase"] = serde_json::json!("configured");
        assert!(restored.restore(&corrupted).is_err());
    }

    #[test]
    fn configured_idle_dma_close_and_reopen_preserve_registers_without_completion() {
        // X-E2 4.11 close_image_channel performs read/clear of +4 on an
        // already disabled configured channel. This is not a DMA request.
        let mut model = FaceDetector::default();
        for channel in 0..4 {
            let base = 0x800 + channel * 0x80;
            let setup = [
                (DmaOperation::Read, 4, 0),
                (DmaOperation::Write, 4, 0),
                (DmaOperation::Read, 0x10, 0),
                (DmaOperation::Read, 4, 0),
                (DmaOperation::Write, 0, 0),
                (DmaOperation::Read, 0x20, 0),
                (DmaOperation::Write, 0x20, 0x403000),
                (DmaOperation::Write, 4, 0),
                (DmaOperation::Write, 0x28, 0x100),
                (DmaOperation::Write, 0x44, 0),
            ];
            for (operation, offset, value) in setup {
                let result = match operation {
                    DmaOperation::Read => read(&mut model, base + offset),
                    DmaOperation::Write => write(&mut model, base + offset, value),
                };
                assert_eq!(result, None);
            }
            let before = model.state.dma[channel as usize].registers.clone();
            for _ in 0..2 {
                assert_eq!(read(&mut model, base + 4), None);
                let saved = model.save();
                let mut resumed = FaceDetector::default();
                resumed.restore(&saved).unwrap();
                assert!(write(&mut resumed, base + 4, 1).is_some());
                assert_eq!(write(&mut model, base + 4, 0), None);
                assert_eq!(write(&mut resumed, base + 4, 0), None);
                assert_eq!(resumed.save(), model.save());
                assert_eq!(
                    model.state.dma[channel as usize].phase,
                    DmaPhase::Configured
                );
                assert_eq!(model.state.dma[channel as usize].registers, before);
                assert!(model.state.request.is_none());
                let saved = model.save();
                resumed.restore(&saved).unwrap();
                assert_eq!(resumed.save(), saved);
            }
            // Reopen performs the same inactive poll/setup, now retaining the
            // configured values instead of starting from an empty word bank.
            for (operation, offset, value) in setup {
                let result = match operation {
                    DmaOperation::Read => read(&mut model, base + offset),
                    DmaOperation::Write => write(&mut model, base + offset, value),
                };
                assert_eq!(result, None);
            }
            assert_eq!(model.state.dma[channel as usize].registers, before);
            assert!(model.state.request.is_none());
            assert!(read(&mut model, base + 8).is_some());
        }
    }

    #[test]
    fn idle_dma_setup_replays_extra_mode_two_from_zero() {
        use DmaOperation::{Read, Write};
        let mut model = FaceDetector::default();
        // X-E2S idle reopen reaches +0x44 with 2 after this clear/poll/setup
        // prefix; the retained extra word can still be zero at that point.
        let accesses = [
            (Read, 0x884, 0),
            (Write, 0x884, 0),
            (Read, 0x890, 0),
            (Read, 0x884, 0),
            (Write, 0x880, 0),
            (Read, 0x8a0, 0),
            (Write, 0x8a0, 0x403000),
            (Write, 0x884, 0),
            (Write, 0x8a8, 0x100),
            (Write, 0x8c4, 2),
        ];
        for (operation, offset, value) in accesses {
            if offset == 0x8c4 {
                assert_eq!(model.state.dma[1].phase, DmaPhase::ConfigExtra);
                assert_eq!(model.state.dma[1].registers.get(&0x44), None);
                let saved = model.save();
                for unknown in [1, 3, 4, u32::MAX.into()] {
                    assert!(write(&mut model, offset, unknown).is_some());
                    assert_eq!(model.save(), saved);
                }
            }
            let mut host = Clock;
            let mut cx = Cx::new(&mut host, 0, 0, 4);
            if operation == Read {
                assert_eq!(model.read(&mut cx, BASE + offset, 4), value);
            } else {
                model.write(&mut cx, BASE + offset, 4, value);
            }
            assert_eq!(cx.take_refusal(), None);
            let saved = model.save();
            let mut restored = FaceDetector::default();
            restored.restore(&saved).unwrap();
            assert_eq!(restored.save(), saved);
        }
        assert_eq!(model.state.dma[1].phase, DmaPhase::Configured);
        assert_eq!(model.state.dma[1].registers[&0x44], 2);
        let mut corrupt = model.save();
        corrupt["dma"][1]["accesses"][9]["value"] = Value::from(3);
        assert!(FaceDetector::default().restore(&corrupt).is_err());
        assert_eq!(model.next_deadline(0), None);
        assert!(read(&mut model, 0x600).is_some());
    }

    fn armed_with_mask(mask: u64) -> FaceDetector {
        arm(FaceDetector::default(), mask)
    }

    fn arm(mut model: FaceDetector, mask: u64) -> FaceDetector {
        initialize(&mut model, 0);
        assert_eq!(write(&mut model, 0x14, 0x0fff), None);
        assert_eq!(
            model.state.request.as_ref().unwrap().phase,
            RequestPhase::ParameterByte
        );
        assert_eq!(write(&mut model, 0x570, 0x1f), None);
        assert!(write(&mut model, 0x278, 0).is_some());
        let mut program = vec![0x274, 0x278, 0x204, 0x208];
        program.extend(FaceDetector::job_tail());
        for offset in program {
            let value = match offset {
                0x48 => 0x40000000,
                0x578 => mask,
                _ => 0,
            };
            if offset == 0x578 {
                let before = model.save();
                assert!(write(&mut model, u64::from(offset), value ^ 1).is_some());
                assert_eq!(model.save(), before);
            }
            assert_eq!(write(&mut model, u64::from(offset), value), None);
        }
        for channel in [0u64, 2, 3, 1] {
            let base = 0x800 + 0x80 * channel;
            assert_eq!(read(&mut model, base + 0x20), None);
            assert_eq!(write(&mut model, base + 0x20, 0x3000), None);
            assert_eq!(write(&mut model, base + 4, 0), None);
            assert_eq!(write(&mut model, base + 0x28, 0x100), None);
            assert_eq!(write(&mut model, base + 0x44, 2), None);
        }
        for channel in [1u64, 3, 0, 2] {
            let base = 0x800 + 0x80 * channel;
            assert_eq!(write(&mut model, base + 0x0c, 1), None);
            assert_eq!(read(&mut model, base + 0x20), None);
            assert_eq!(write(&mut model, base + 0x20, 0x3000), None);
            assert_eq!(read(&mut model, base + 4), None);
            assert_eq!(write(&mut model, base + 4, 1), None);
        }
        assert_eq!(write(&mut model, 0x8f8, 0x10000000), None);
        assert_eq!(write(&mut model, 4, 1), None);
        model
    }

    fn armed() -> FaceDetector {
        armed_with_mask(0x000b000b)
    }

    #[test]
    fn paired_search_selections_restore_without_producing_results() {
        for mask in [0x00070007, 0x00090009, 0x000b000b] {
            let model = armed_with_mask(mask);
            let mut restored = FaceDetector::default();
            restored.restore(&model.save()).unwrap();
            assert_eq!(restored.save(), model.save());
            assert_eq!(model.next_deadline(0), None);
        }
    }

    #[test]
    fn complete_request_submission_is_ordered_and_produces_no_results_or_deadline() {
        let mut model = armed();
        assert_eq!(
            model.state.request.as_ref().unwrap().phase,
            RequestPhase::AwaitingResult
        );
        assert_eq!(model.next_deadline(0), None);
        assert!(read(&mut model, 4).is_some());
        let mut restored = FaceDetector::default();
        restored.restore(&model.save()).unwrap();
        assert_eq!(
            restored.state.request.as_ref().unwrap().phase,
            RequestPhase::AwaitingResult
        );
        assert!(
            restored
                .restore(&serde_json::json!({"registers":{}, "request":4096}))
                .is_err()
        );
    }

    #[test]
    fn reset_without_a_job_retains_the_ordered_control_pair_only() {
        let mut model = FaceDetector::default();
        assert!(write(&mut model, 0, 1).is_some());
        assert!(write(&mut model, 0, 2).is_some());
        assert_eq!(write(&mut model, 0, 0), None);
        let mut restored = FaceDetector::default();
        restored.restore(&model.save()).unwrap();
        assert_eq!(write(&mut restored, 0, 1), None);
        assert!(restored.state.request.is_none());
        assert_eq!(restored.next_deadline(0), None);
        assert!(read(&mut restored, 0x600).is_some());
    }

    fn stop_accesses(close: bool) -> Vec<(DmaOperation, u64, u64)> {
        use DmaOperation::{Read, Write};
        let mut accesses = vec![(Write, 0, 0), (Write, 0, 1)];
        for channel in [1, 3, 0, 2] {
            let base = 0x800 + channel * 0x80;
            if close {
                accesses.extend([(Read, base + 4, 0), (Write, base + 4, 0)]);
            } else {
                accesses.extend([
                    (Write, base + 0xc, 1),
                    (Read, base + 4, 1),
                    (Write, base + 4, 0),
                    (Read, base + 0x10, 0),
                    (Read, base + 0x20, 0x3000),
                    (Write, base + 0x20, 0x3000),
                ]);
            }
        }
        accesses
    }

    #[test]
    fn armed_request_can_be_reset_stopped_and_closed_without_a_result_or_irq() {
        let mut model = armed();
        let submission = model.state.request.as_ref().unwrap().accesses.clone();
        for close in [false, true] {
            for (operation, offset, value) in stop_accesses(close) {
                let saved = model.save();
                if operation == DmaOperation::Write {
                    assert!(write(&mut model, offset, value ^ 2).is_some());
                    assert_eq!(model.save(), saved);
                    assert_eq!(write(&mut model, offset, value), None);
                } else {
                    assert_eq!(read(&mut model, offset), None);
                }
                // Every partial reset/stop prefix is independently replayable.
                let mut restored = FaceDetector::default();
                restored.restore(&model.save()).unwrap();
                assert_eq!(restored.save(), model.save());
                assert_eq!(model.next_deadline(0), None);
                assert_eq!(
                    &model.state.request.as_ref().unwrap().accesses[..submission.len()],
                    submission.as_slice()
                );
            }
            assert_eq!(
                model.state.request.as_ref().unwrap().phase,
                if close {
                    RequestPhase::Closed
                } else {
                    RequestPhase::Stopped
                }
            );
            for channel in &model.state.dma {
                assert_eq!(channel.registers[&4], 0);
            }
            for offset in [0x600, 0x610, 0x8f4] {
                assert!(read(&mut model, offset).is_some());
            }
        }
        let mut corrupt = model.save();
        corrupt["request"]["accesses"]
            .as_array_mut()
            .unwrap()
            .remove(submission.len());
        assert!(FaceDetector::default().restore(&corrupt).is_err());
    }

    #[test]
    fn closed_requests_reconfigure_with_retained_idle_dma_and_bounded_history() {
        let mut model = armed();
        let submission = model.state.request.as_ref().unwrap().accesses.clone();
        for _ in 0..3 {
            for close in [false, true] {
                for (operation, offset, value) in stop_accesses(close) {
                    if operation == DmaOperation::Write {
                        assert_eq!(write(&mut model, offset, value), None);
                    } else {
                        assert_eq!(read(&mut model, offset), None);
                    }
                }
                if !close {
                    let stopped = model.save();
                    assert!(write(&mut model, 0x10, 0).is_some());
                    assert_eq!(model.save(), stopped);
                }
            }
            let closed = model.save();
            assert!(write(&mut model, 0x10, 0x80000000).is_some());
            assert_eq!(model.save(), closed);
            let retained = model.state.dma.clone();
            assert_eq!(write(&mut model, 0x10, 0), None);
            for (channel, previous) in model.state.dma.iter().zip(&retained) {
                assert_eq!(channel.registers, previous.registers);
                assert_eq!(channel.initial_registers, previous.registers);
                assert!(channel.accesses.is_empty());
                assert_eq!(channel.registers[&4], 0);
                assert_eq!(channel.registers[&0x44], 2);
            }
            let reopening = model.save();
            FaceDetector::default().restore(&reopening).unwrap();
            // Native reconfiguration can reset this already closed request.
            // It still needs the ordered pair and cannot alter retained DMA.
            assert!(write(&mut model, 0, 1).is_some());
            assert_eq!(model.save(), reopening);
            assert_eq!(write(&mut model, 0, 0), None);
            FaceDetector::default().restore(&model.save()).unwrap();
            assert_eq!(write(&mut model, 0, 1), None);
            FaceDetector::default().restore(&model.save()).unwrap();
            assert_eq!(model.next_deadline(0), None);
            for (channel, previous) in model.state.dma.iter().zip(&retained) {
                assert_eq!(channel.registers, previous.registers);
            }
            initialize(&mut model, 2);
            for access in &submission {
                if access.operation == DmaOperation::Write {
                    assert_eq!(
                        write(&mut model, access.offset.into(), access.value.into()),
                        None
                    );
                } else {
                    assert_eq!(read(&mut model, access.offset.into()), None);
                }
            }
            assert_eq!(
                model.state.request.as_ref().unwrap().phase,
                RequestPhase::AwaitingResult
            );
            assert_eq!(
                model.state.request.as_ref().unwrap().accesses.len(),
                submission.len()
            );
            assert!(
                model
                    .state
                    .dma
                    .iter()
                    .all(|channel| channel.accesses.len() == 10)
            );
            assert_eq!(model.next_deadline(0), None);
            assert!(read(&mut model, 0x600).is_some());
            let saved = model.save();
            let mut restored = FaceDetector::default();
            restored.restore(&saved).unwrap();
            assert_eq!(restored.save(), saved);
            let mut false_idle = saved.clone();
            false_idle["request"]["initial_dma"][0]["initial_registers"]["16"] = Value::from(1);
            assert!(restored.restore(&false_idle).is_err());
            assert_eq!(restored.save(), saved);
        }
    }

    fn replay(model: &mut FaceDetector, accesses: &[RequestAccess]) {
        for access in accesses {
            let offset = u64::from(access.offset);
            if access.operation == DmaOperation::Write {
                assert_eq!(write(model, offset, access.value.into()), None);
            } else {
                assert_eq!(read(model, offset), None);
            }
        }
    }

    #[test]
    fn burst_requests_follow_stopped_requests_without_closing() {
        let mut model = armed();
        let submission = model.state.request.as_ref().unwrap().accesses.clone();
        for _ in 0..3 {
            let stops = stop_accesses(false);
            for (index, (operation, offset, value)) in stops.iter().copied().enumerate() {
                // A request cannot start before every local channel stops.
                if index + 1 < stops.len() {
                    let saved = model.save();
                    assert!(write(&mut model, 0x14, 0x0fff).is_some());
                    assert_eq!(model.save(), saved);
                }
                if operation == DmaOperation::Write {
                    assert_eq!(write(&mut model, offset, value), None);
                } else {
                    assert_eq!(read(&mut model, offset), None);
                }
            }
            assert_eq!(
                model.state.request.as_ref().unwrap().phase,
                RequestPhase::Stopped
            );
            let stopped = model.state.dma.clone();
            assert!(write(&mut model, 0x14, 0x1000).is_some());
            replay(&mut model, &submission);
            let request = model.state.request.as_ref().unwrap();
            assert_eq!(request.phase, RequestPhase::AwaitingResult);
            assert_eq!(request.accesses, submission);
            for (channel, previous) in request.initial_dma.iter().zip(&stopped) {
                assert_eq!(channel.initial_phase, DmaPhase::Configured);
                assert_eq!(channel.initial_registers, previous.registers);
                assert!(channel.accesses.is_empty());
            }
            assert_eq!(model.next_deadline(0), None);
            let saved = model.save();
            let mut restored = FaceDetector::default();
            restored.restore(&saved).unwrap();
            assert_eq!(restored.save(), saved);
            let mut busy = saved.clone();
            busy["request"]["initial_dma"][0]["initial_registers"]["4"] = Value::from(1);
            assert!(FaceDetector::default().restore(&busy).is_err());
        }
        // The burst still ends with the single-request close.
        for close in [false, true] {
            for (operation, offset, value) in stop_accesses(close) {
                if operation == DmaOperation::Write {
                    assert_eq!(write(&mut model, offset, value), None);
                } else {
                    assert_eq!(read(&mut model, offset), None);
                }
            }
        }
        assert_eq!(write(&mut model, 0x10, 0), None);
        // Without a stopped request, a channel cannot claim configuration it
        // never performed.
        let mut idle = FaceDetector::default();
        initialize(&mut idle, 0);
        let mut forged = idle.save();
        let channel = &mut forged["dma"][0];
        channel["initial_registers"] = channel["registers"].clone();
        channel["accesses"] = Value::Array(Vec::new());
        channel["initial_phase"] = Value::from("configured");
        assert!(FaceDetector::default().restore(&forged).is_err());
    }

    struct Timed {
        now: u64,
        raised: Vec<(usize, u32)>,
    }

    impl Host for Timed {
        fn now(&self) -> u64 {
            self.now
        }
        fn schedule(&mut self, _: usize, _: u64, _: u64) {
            unreachable!()
        }
        fn set_irq(&mut self, _: u32, _: bool) {
            unreachable!()
        }
        fn raise_irq(&mut self, cpu: usize, line: u32) -> Result<(), String> {
            self.raised.push((cpu, line));
            Ok(())
        }
        fn ram_read(&self, _: u64, _: &mut [u8]) -> Result<(), String> {
            unreachable!()
        }
        fn ram_write(&mut self, _: u64, _: &[u8]) -> Result<(), String> {
            unreachable!()
        }
        fn record(&mut self, _: Record) {}
    }

    fn advance(model: &mut FaceDetector, now: u64) -> (Option<String>, Vec<(usize, u32)>) {
        let mut host = Timed {
            now,
            raised: Vec::new(),
        };
        let mut cx = Cx::new(&mut host, usize::MAX, 0, 4);
        model.advance(&mut cx);
        let refusal = cx.take_refusal();
        (refusal, host.raised)
    }

    fn with_empty_results() -> FaceDetector {
        let routes = IrqRoutes::default();
        routes.set(IRQ, Some(1)).unwrap();
        let options = Options {
            mode: Mode::Configuration,
            results: Some(Results::Empty),
        };
        FaceDetector::new(options, Some(routes)).unwrap()
    }

    fn assert_restores(model: &FaceDetector) {
        let mut restored = with_empty_results();
        restored.restore(&model.save()).unwrap();
        assert_eq!(restored.save(), model.save());
    }

    #[test]
    fn declared_empty_results_complete_burst_requests_through_irq_ack_stops_and_count() {
        // X70/X-E2S stock face completion: ISR 145DC54 acknowledges +8F4,
        // the collector stops channels 1,3,0,2 and reads the +600 count.
        let mut model = arm(with_empty_results(), 0x000b000b);
        let submission = model.state.request.as_ref().unwrap().accesses.clone();
        for _ in 0..3 {
            let due = model.next_deadline(0).unwrap();
            assert_eq!(due, RESULT_LATENCY);
            assert_eq!(advance(&mut model, due - 1), (None, vec![]));
            assert!(write(&mut model, 0x8f4, 1).is_some());
            assert!(read(&mut model, 0x600).is_some());
            assert_eq!(advance(&mut model, due), (None, vec![(1, IRQ)]));
            assert_eq!(model.next_deadline(0), None);
            assert_restores(&model);
            assert!(read(&mut model, 0x600).is_some());
            assert!(write(&mut model, 0x8f4, 2).is_some());
            assert_eq!(write(&mut model, 0x8f4, 1), None);
            for (operation, offset, value) in stop_accesses(false).into_iter().skip(2) {
                assert!(read(&mut model, 0x600).is_some());
                if operation == DmaOperation::Write {
                    assert_eq!(write(&mut model, offset, value), None);
                } else {
                    assert_eq!(read(&mut model, offset), None);
                }
                assert_restores(&model);
            }
            let mut host = Clock;
            let mut cx = Cx::new(&mut host, 0, 0, 4);
            assert_eq!(model.read(&mut cx, BASE + 0x600, 4), 0);
            assert_eq!(cx.take_refusal(), None);
            assert_eq!(
                model.state.request.as_ref().unwrap().phase,
                RequestPhase::Stopped
            );
            assert_restores(&model);
            let mut moved = model.save();
            let raised = moved["request"]["raised_after"].as_u64().unwrap();
            moved["request"]["raised_after"] = Value::from(raised + 1);
            assert!(with_empty_results().restore(&moved).is_err());
            assert!(FaceDetector::default().restore(&model.save()).is_err());
            replay(&mut model, &submission);
            assert_eq!(
                model.state.request.as_ref().unwrap().phase,
                RequestPhase::AwaitingResult
            );
        }
        // A cancellation withdraws the pending result.
        assert_eq!(write(&mut model, 0, 0), None);
        assert_eq!(model.next_deadline(0), None);
        assert_restores(&model);
        let mut unanswered = arm(with_empty_results(), 0x000b000b).save();
        unanswered["due"] = Value::Null;
        assert!(with_empty_results().restore(&unanswered).is_err());
        let options = Options {
            mode: Mode::Configuration,
            results: Some(Results::Empty),
        };
        assert!(FaceDetector::new(options, None).is_err());
    }

    #[test]
    fn reset_cannot_skip_local_stops_or_invent_idle_status() {
        let mut model = armed();
        assert!(write(&mut model, 0, 1).is_some());
        assert_eq!(write(&mut model, 0, 0), None);
        assert!(write(&mut model, 0x88c, 1).is_some());
        assert_eq!(write(&mut model, 0, 1), None);
        assert!(write(&mut model, 0x80c, 1).is_some());
        for (operation, offset, value) in stop_accesses(false).into_iter().skip(2).take(3) {
            if operation == DmaOperation::Write {
                assert_eq!(write(&mut model, offset, value), None);
            } else {
                assert_eq!(read(&mut model, offset), None);
            }
        }
        model.state.dma[1].registers.insert(0x10, 1);
        let saved = model.save();
        assert!(read(&mut model, 0x890).is_some());
        assert_eq!(model.save(), saved);
        assert!(write(&mut model, 0, 0).is_some());
    }
}
