//! Explicit XP4 processor, wiring and external input selection.
use super::*;
use serde::Deserialize;

fn instruction_rate() -> (u64, u64) {
    (1, 1)
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
    #[serde(default)]
    pub timer_base: Option<u32>,
    #[serde(default)]
    pub timer_layout: Option<TimerLayout>,
    #[serde(default)]
    pub timer_routes: Vec<(usize, usize)>,
    #[serde(default)]
    pub storage_jedec: Option<u32>,
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
    pub xp4_functional_system_control: Option<fujimod_emulator_xp4_io::system_control::Config>,
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
    #[serde(default)]
    pub temperature: Option<(i32, u64)>,
    #[serde(default)]
    pub power_spi: Option<(u64, bool, usize, usize, bool, BTreeMap<u8, u16>, u64)>,
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
        (u64, u64, Vec<u32>, Vec<u32>, Vec<u32>),
        (u64, u64, u64, u64),
        (u16, u8),
        Option<(u64, u32, u64, u64, u64, String)>,
    )>,
    #[serde(default)]
    pub movdc: Vec<(u64, u32, u64, u64, u8)>,
    #[serde(default)]
    pub sd_clocks: Vec<(u64, u64)>,
    /// Card-controller register windows, each excluding its clock halfword.
    #[serde(default)]
    pub register_banks: Vec<(u64, u64)>,
    /// The USB condition register `(base, cable present)`, when declared.
    #[serde(default)]
    pub usb_condition: Option<(u64, bool)>,
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
