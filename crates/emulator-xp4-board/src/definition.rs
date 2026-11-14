//! Explicit XP4 processor, wiring and external input selection.
use super::*;
use serde::Deserialize;

fn instruction_rate() -> (u64, u64) {
    (1, 1)
}

/// Explicit discovery-only receiver input; no physical register model.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryTxReceiver {
    pub address: u32,
    pub request: u8,
    pub capacity: usize,
    pub offer_at: u64,
    pub offer_bytes: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StandaloneDma {
    pub irq_routes: Vec<u32>,
    pub reset_profile: String,
    pub timing_profile: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Definition {
    #[serde(default)]
    pub discovery_tx_receiver: Option<DiscoveryTxReceiver>,
    /// Explicit byte I2C controllers and their declared external peers.
    #[serde(default)]
    pub byte_i2c: Vec<ByteI2c>,
    pub ram_size: usize,
    pub cores: usize,
    pub discovery: bool,
    #[serde(default)]
    pub memory_regions: Vec<super::memory::Region>,
    #[serde(default)]
    pub initial_registers: Vec<Vec<(Register, u64)>>,
    #[serde(default)]
    pub labels: Vec<u32>,
    #[serde(default)]
    pub cbar: Option<u32>,
    /// Explicit initial GIC registers; omitted preserves reset state.
    #[serde(default)]
    pub gic_initial: Option<GicInitial>,
    #[serde(default)]
    pub timer_base: Option<u32>,
    #[serde(default)]
    pub timer_layout: Option<TimerLayout>,
    #[serde(default)]
    pub timer_routes: Vec<(usize, usize)>,
    #[serde(default)]
    pub storage_jedec: Option<u32>,
    /// Immutable executable prefix of the connected XP4 NOR array.
    #[serde(default)]
    pub storage_immutable_prefix: Option<NorImmutablePrefix>,
    /// Declared controller and pad states under which CPU reads of the mapped
    /// NOR return array data; absent serves every mapped read.
    #[serde(default)]
    pub storage_mapped_read: Option<fujimod_emulator_xp4_storage::storage::MappedReadGate>,
    /// Refuse every write-enable, erase and program request to the NOR.
    #[serde(default)]
    pub storage_write_protect: bool,
    /// DMA-only connection with explicitly declared reset and timing hypotheses.
    #[serde(default)]
    pub standalone_dma: Option<StandaloneDma>,
    #[serde(default)]
    pub storage_dma_routes: Vec<u32>,
    #[serde(default)]
    pub spi_inputs: Vec<(u64, bool)>,
    #[serde(default)]
    pub control_requests: Vec<(u64, u64)>,
    /// Explicit, unmeasured functional hypotheses; absent keeps strict refusal.
    #[serde(default)]
    pub xp4_functional_system_control: Option<fujimod_emulator_xp4_system::system_control::Config>,
    #[serde(default)]
    pub storage_geometry: Option<(usize, usize, u64, u64)>,
    #[serde(default)]
    pub storage_irq: Option<u32>,
    #[serde(default)]
    pub xpp_storage: Option<XppStorage>,
    #[serde(default)]
    pub xpp_serial: Vec<u64>,
    #[serde(default)]
    pub xpp_sub_cpu: Option<XppSubCpu>,
    #[serde(default)]
    pub xpp_battery_gauge: Option<XppBatteryGauge>,
    /// X-Processor Pro serial ports with nothing attached: every byte
    /// received reads 0xff.
    #[serde(default)]
    pub xpp_idle_serial: Vec<u64>,
    /// X-mount lens on an X-Processor Pro serial port; requires
    /// `xpp_storage`, `xpp_reset_gates` and the GPIO banks.
    #[serde(default)]
    pub xpp_lens: Option<XppLens>,
    /// Shutter motor and position sensors on the GPIO banks.
    #[serde(default)]
    pub xpp_shutter: Option<XppShutter>,
    /// Keys on external interrupt lines: (line, bank, pin) and the poll period.
    #[serde(default)]
    pub xpp_key_lines: Option<XppKeyLines>,
    #[serde(default)]
    pub xpp_display_frames: Option<XppDisplayFrames>,
    #[serde(default)]
    pub xpp_adc: Option<XppAdc>,
    #[serde(default)]
    pub xpp_display_outputs: Vec<XppDisplayOutput>,
    /// X-Processor Pro clock bank: base, PLL lock ticks and running PLLs.
    #[serde(default)]
    pub xpp_clocks: Option<(u64, u64, Vec<usize>)>,
    /// X-Processor Pro imaging control page base.
    #[serde(default)]
    pub xpp_imaging_control: Option<u64>,
    /// X-Processor Pro reset and gate controls base.
    #[serde(default)]
    pub xpp_reset_gates: Option<u64>,
    #[serde(default)]
    pub xpp_sd: Option<XppSd>,
    /// X-Processor Pro DesignWare I2C buses with their declared parts.
    #[serde(default)]
    pub xpp_i2c: Vec<XppI2c>,
    /// X-Processor Pro DRAM controller base and refresh-rate level.
    #[serde(default)]
    pub xpp_dram: Option<(u64, u8)>,
    /// Physical base of a writable alias of the whole DRAM.
    #[serde(default)]
    pub dram_alias: Option<u64>,
    /// X-Processor Pro DWC3: base, interrupt line, command and transaction
    /// ticks, negotiated speed (0 high speed, 4 SuperSpeed).
    #[serde(default)]
    pub xpp_usb: Option<(u64, u32, u64, u64, u8)>,
    /// X-Processor Pro power control base (supply hold and power domains).
    #[serde(default)]
    pub xpp_power: Option<u64>,
    /// X-Processor Pro imaging statistics engine: base, interrupt line and
    /// frame period in ticks.
    #[serde(default)]
    pub xpp_statistics: Option<(u64, u32, u64)>,
    /// Uniform scene for the statistics engine: the red, green and blue sum
    /// of every zone.
    #[serde(default)]
    pub xpp_statistics_scene: Option<[u32; 3]>,
    /// Statistics engine buffers written each frame: output register offset
    /// and words.
    #[serde(default)]
    pub xpp_statistics_buffers: Vec<(u64, Vec<u32>)>,
    /// Uniform live-view image: unit base, lines and Y, Cb, Cr.
    #[serde(default)]
    pub xpp_live_view_image: Option<(u64, u32, [u8; 3])>,
    /// X-Processor Pro face detection engine: base, interrupt line and run
    /// time in ticks.
    #[serde(default)]
    pub xpp_face: Option<(u64, u32, u64)>,
    /// X-Processor Pro register blocks with no work in flight.
    #[serde(default)]
    pub xpp_register_blocks: Vec<XppRegisterBlock>,
    /// DesignWare HDMI TX base; the board has no sink attached.
    #[serde(default)]
    pub xpp_hdmi: Option<u64>,
    /// X-Processor Pro imaging frame sync; its interrupt words lie inside
    /// the imaging control page.
    #[serde(default)]
    pub xpp_frame_sync: Option<XppFrameSync>,
    #[serde(default)]
    pub cache_sizes: Option<(u32, u32, u32)>,
    #[serde(default)]
    pub io_group_base: Option<u64>,
    #[serde(default)]
    pub decompressor: Option<(u64, u32)>,
    #[serde(default)]
    pub lcdc: Vec<(u64, u64, u32, u32, Option<String>, Option<String>)>,
    #[serde(default)]
    pub dmp: Option<(u64, u32)>,
    #[serde(default)]
    pub adc: Option<([u16; 12], u64)>,
    #[serde(default)]
    pub gpio: Option<[u16; 16]>,
    /// GPIO bank base; the XP4 0xfff41000 when absent.
    #[serde(default)]
    pub gpio_base: Option<u64>,
    /// GPIO pads wired to level-sensitive interrupt inputs.
    #[serde(default)]
    pub gpio_interrupts: Vec<fujimod_emulator_devices::parts::gpio::InputIrq>,
    #[serde(default)]
    pub temperature: Option<(i32, u64)>,
    #[serde(default)]
    pub power_spi: Option<(u64, bool, usize, usize, bool, BTreeMap<u8, u16>, u64)>,
    /// Optional exact uninterpreted output; requires the power SPI peer.
    #[serde(default)]
    pub power_spi_command_output: Option<fujimod_emulator_xp4_io::power_serial::ExactCommandOutput>,
    #[serde(default)]
    pub checksum_power_spi: Option<(
        u64,
        bool,
        usize,
        usize,
        bool,
        BTreeMap<u8, u16>,
        Vec<(u8, u16)>,
        Option<fujimod_emulator_xp4_io::rtc::StagedConfig>,
        Vec<u8>,
    )>,
    #[serde(default)]
    pub imaging: Option<(
        (u64, u64, Vec<u32>, Vec<u32>, Vec<u32>, Option<Vec<u32>>),
        (u64, u64, u64, u64),
        (u16, u8),
        Option<(u64, u32, u64, u64, u64, String)>,
    )>,
    #[serde(default)]
    pub movdc: Vec<(u64, u32, u64, u64, u8)>,
    #[serde(default)]
    pub sd_clocks: Vec<(u64, u64)>,
    /// SD hosts with the card in their slot; each owns its page, clock halfword included.
    #[serde(default)]
    pub sd_hosts: Vec<SdHost>,
    /// Card-controller register windows, each excluding its clock halfword.
    #[serde(default)]
    pub register_banks: Vec<(u64, u64)>,
    /// The USB condition register `(base, cable present)`, when declared.
    #[serde(default)]
    pub usb_condition: Option<(u64, bool)>,
    /// The countdown timer bank `(base, [(channel, interrupt line)])`, when declared.
    #[serde(default)]
    pub countdown_timers: Option<(u64, Vec<(usize, u32)>)>,
    /// Command dials wired to countdown timer channels' count inputs.
    #[serde(default)]
    pub countdown_dials: Vec<CountdownDial>,
    /// The JPEG encoder with the XP4 driver's register contract, when declared.
    #[serde(default)]
    pub jpeg_encoder: Option<JpegEncoder>,
    /// Declared still-processing engines (see `crate::still`).
    #[serde(default)]
    pub still_engines: Vec<StillEngine>,
    /// The still DSP's host interface (see `crate::dsp`), when declared.
    #[serde(default)]
    pub dsp: Option<Dsp>,
    /// The face detection engine with the XP4 driver's register contract, when declared.
    #[serde(default)]
    pub face_detector: Option<FaceDetector>,
    /// The filter engine with the XP4 driver's register contract, when declared.
    #[serde(default)]
    pub filter_engine: Option<FilterEngine>,
    #[serde(default = "instruction_rate")]
    pub instructions_per_tick: (u64, u64),
    #[serde(default)]
    pub usb: Option<(u64, u32, u64, u64, u8)>,
    #[serde(default)]
    pub usb_phy: Option<(u64, [u8; 32], u32)>,
    #[serde(default)]
    pub xpp_boot: Option<XppBoot>,
    #[serde(default)]
    pub pmu_cycles_per_tick: Option<u64>,
    /// Declared (port base, send word, receive word) replies.
    #[serde(default)]
    pub xpp_serial_replies: Vec<(u64, u32, u32)>,
}

/// A command dial whose quadrature phases drive a countdown timer channel's
/// count inputs, the host's name for it, and the time between its edges.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CountdownDial {
    pub name: String,
    pub channel: usize,
    pub phase_ticks: u64,
    pub rest_phase: u8,
}

/// The XP4 JPEG encoder: register page base, completion interrupt line,
/// run time in ticks and the address of its memory type word.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JpegEncoder {
    pub base: u64,
    pub irq: u32,
    pub run_ticks: u64,
    pub memory_type: u64,
}

/// One declared XP4 still-processing engine: which contract, its register
/// page base, completion interrupt line and the ticks a job takes.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StillEngine {
    pub engine: crate::still::Kind,
    pub base: u64,
    pub irq: u32,
    pub run_ticks: u64,
    /// A finished job also computes the engine's declared output (opt-in;
    /// RESZ only, see `crate::still`). Absent, a job writes nothing.
    #[serde(default)]
    pub compute: bool,
}

/// The XP4 still DSP host interface: completion interrupt line and the
/// ticks a command takes.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dsp {
    pub irq: u32,
    pub run_ticks: u64,
}

/// The XP4 face detection engine: register page base, finished-run
/// interrupt line and run time in ticks.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FaceDetector {
    pub base: u64,
    pub irq: u32,
    pub run_ticks: u64,
}

/// The XP4 filter engine: register page base, address of its two memory
/// type words, finished-run interrupt line and run time in ticks.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilterEngine {
    pub base: u64,
    pub memory_types: u64,
    pub irq: u32,
    pub run_ticks: u64,
}

/// Register layout of the interval timer at `timer_base`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimerLayout {
    /// Eight channels; select bits set in the control word.
    Xp4,
    /// Sixteen channels; select bits clear in the control word.
    Xpp,
}

/// Byte-frame peer on one X-Processor Pro serial port: (even register,
/// 16-bit value) pairs after reset; unlisted registers read zero.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct XppBatteryGauge {
    pub port: u64,
    pub registers: Vec<(u8, u16)>,
}

/// Register-file peer on one X-Processor Pro serial port: (register, value)
/// pairs after reset; unlisted registers read zero.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct XppSubCpu {
    pub port: u64,
    pub registers: Vec<(u8, u8)>,
    /// Seconds count kept in the register file.
    #[serde(default)]
    pub clock: Option<fujimod_emulator_xpp::serial::Clock>,
}

/// X-mount lens: serial port base, information page and command replies,
/// the body's power output (bank, register, bit), the ready pin (bank, pin)
/// and its external interrupt line, the contact levels (bank, pin, high) and
/// the declared delays in ticks.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct XppLens {
    pub port: u64,
    pub page: Vec<u8>,
    pub replies: Vec<(u32, u16)>,
    pub power: (usize, usize, usize),
    pub ready: (usize, usize),
    pub line: usize,
    pub contacts: Vec<(usize, usize, bool)>,
    pub ready_ticks: u64,
    pub release_ticks: u64,
    pub drive_ticks: u64,
    pub poll_ticks: u64,
}

/// Shutter: the motor pins' mode register (bank, register) and pins, the
/// sensor inputs (bank, pin), their external interrupt line, the sensor
/// levels in the order the motor reaches them and the declared ticks.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct XppShutter {
    pub mode: (usize, usize),
    pub motor: (usize, usize),
    pub sensors: Vec<(usize, usize)>,
    pub line: usize,
    pub positions: Vec<Vec<bool>>,
    pub step_ticks: u64,
    pub poll_ticks: u64,
}

/// Keys whose GPIO inputs also reach external interrupt lines.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct XppKeyLines {
    pub lines: Vec<(usize, usize, usize)>,
    pub poll_ticks: u64,
}

/// X-Processor Pro display frame interrupts: one edge per line each period.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct XppDisplayFrames {
    #[serde(default)]
    pub timing: Option<([u64; 2], u64)>,
    pub irqs: Vec<u32>,
    pub period_ticks: u64,
}

/// X-Processor Pro display output registers: word offsets whose status bits
/// read set or clear regardless of the last write, and the optional
/// (host output, capture period in ticks) of its OSD layer scanout.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct XppDisplayOutput {
    #[serde(default)]
    pub handshake_ticks: Option<u64>,
    pub base: u64,
    pub size: u64,
    pub set_bits: Vec<(u64, u32)>,
    pub clear_bits: Vec<(u64, u32)>,
    #[serde(default)]
    pub scanout: Option<(usize, u64)>,
}

/// X-Processor Pro SD channel with a freshly formatted card in the slot.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct XppSd {
    pub options: fujimod_emulator_xpp::sd::Options,
    pub card: fujimod_emulator_storage::fat::Options,
    #[serde(default)]
    pub irq: Option<u32>,
    /// SD channel of the reset-gate line; requires `xpp_reset_gates`.
    #[serde(default)]
    pub reset_channel: Option<usize>,
}

/// An XP4 SD host with a freshly formatted card in its slot.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SdHost {
    pub options: fujimod_emulator_xp4_io::sd::Options,
    pub card: fujimod_emulator_storage::fat::Options,
    #[serde(default)]
    pub irq: Option<u32>,
}

/// X-Processor Pro imaging frame sync: register base, the address of the
/// frame interrupt words, the (source, line) pairs frames latch and the frame
/// period in ticks.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct XppFrameSync {
    pub base: u64,
    pub interrupts: u64,
    pub frame_lines: Vec<(usize, u32)>,
    pub period_ticks: u64,
}

/// X-Processor Pro register block: base, window size, the (offset, mask)
/// status bits that always read set and an optional frame or job law.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct XppRegisterBlock {
    pub base: u64,
    pub size: u64,
    pub idle: Vec<(u64, u32)>,
    #[serde(default)]
    pub frames: Option<XppBlockFrames>,
    #[serde(default)]
    pub job: Option<XppBlockJob>,
    /// (offset, value) of words that read a fixed value.
    #[serde(default)]
    pub fixed: Vec<(u64, u32)>,
    /// Commands that set a field of a status word.
    #[serde(default)]
    pub commands: Vec<XppBlockCommand>,
}

/// Command law of a register block: writing exactly `value` to `word` sets
/// the `mask` field of the `status` word to `state`.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct XppBlockCommand {
    pub word: u64,
    pub value: u32,
    pub status: u64,
    pub mask: u32,
    pub state: u32,
}

/// Job law of a register block: (offset, mask) of the start bit, of the
/// status bit (none: the job pulses its interrupt) and of a stop bit, the
/// level interrupt line and the ticks a job takes.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct XppBlockJob {
    pub start: (u64, u32),
    #[serde(default)]
    pub status: Option<(u64, u32)>,
    #[serde(default)]
    pub stop: Option<(u64, u32)>,
    pub irq: u32,
    pub ticks: u64,
}

/// Frame law of a register block: (offset, mask) of the start and stopped
/// bits, the interrupt line and the frame period in ticks.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct XppBlockFrames {
    pub start: (u64, u32),
    pub stopped: (u64, u32),
    pub irq: u32,
    pub period_ticks: u64,
}

/// One X-Processor Pro DesignWare I2C bus and the parts that answer on it.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct XppI2c {
    pub options: fujimod_emulator_ip::dw_i2c::Options,
    pub parts: Vec<fujimod_emulator_xpp::i2c::Part>,
}

/// X-Processor Pro ADC: register base, twelve 10-bit samples and the ticks a
/// single conversion takes.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct XppAdc {
    pub base: u64,
    pub samples: Vec<u16>,
    pub conversion_ticks: u64,
}

/// X-Processor Pro flash controller, read aperture and PL330 wiring.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct XppStorage {
    pub flash_controller: u64,
    pub aperture: u64,
    pub aperture_size: u64,
    pub dma: u64,
    pub dma_irq_routes: Vec<u32>,
    pub dma_setup_ticks: u64,
    pub dma_bytes_per_tick: u64,
    /// Interrupt line of the controller status.
    #[serde(default)]
    pub irq: Option<u32>,
    /// NOR device attached to one chip select, with its array timing.
    #[serde(default)]
    pub nor: Option<XppNor>,
}

/// Declared NOR register device and array behind one X-Processor Pro chip select.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct XppNor {
    pub chip_select: u8,
    pub registers: fujimod_emulator_devices::parts::micron_nor::Options,
    pub array: fujimod_emulator_xpp::flash::Array,
}

/// X-Processor Pro cold-reset controller and its boot-selection strap level.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct XppBoot {
    pub options: fujimod_emulator_xpp::boot::Options,
    pub card_strap: bool,
    /// Memory window shown at physical 0 until the guest selects the remap.
    #[serde(default)]
    pub low_alias: Option<u64>,
}

/// Explicit immutable NOR aperture policy.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NorImmutablePrefix { pub bytes: usize }

/// Initial values supported by the modeled GIC register interface.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GicInitial {
    pub distributor_control: u32,
    pub cpu_interfaces: Vec<GicCpuInitial>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GicCpuInitial {
    pub control: u32,
    pub priority_mask: u32,
    pub binary_point: u32,
}

impl GicInitial {
    pub(super) fn apply(self, gic: &mut Gic, cores: usize) -> Result<(), String> {
        if self.distributor_control > 1 || self.cpu_interfaces.len() != cores
            || self.cpu_interfaces.iter().any(|cpu| cpu.control > 1
                || cpu.priority_mask > 255 || cpu.binary_point > 7) {
            return Err("Invalid initial GIC registers or CPU interface count".into());
        }
        gic.access(0, true, 0, 4, true, self.distributor_control)?;
        for (core, cpu) in self.cpu_interfaces.into_iter().enumerate() {
            for (offset, value) in [(0, cpu.control), (4, cpu.priority_mask), (8, cpu.binary_point)] {
                gic.access(core, false, offset, 4, true, value)?;
            }
        }
        Ok(())
    }
}

/// One byte I2C controller with an explicit reset/timing fixture and peer.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ByteI2c {
    pub base: u64,
    pub irq: u32,
    pub profile: fujimod_emulator_xp4_io::i2c::Profile,
    pub peer: fujimod_emulator_xp4_io::i2c::Peer,
}
