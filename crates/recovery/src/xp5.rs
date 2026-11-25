//! Raw XP5 entry, serial NOR preparation, and optional post-DDR FAT32 loading.
use crate::image::{self, File, Result, Source, extent, put, require, word};
use crate::StartupStop;
use serde::{Deserialize, Serialize};
mod banks;
pub use banks::Banks;
mod card_boot;
pub use card_boot::CardBoot;
pub mod handoff_result;
pub use handoff_result::HandoffRange;
mod redirect;
mod template {
    include!(concat!(env!("OUT_DIR"), "/xp5_sd_loader.rs"));
}
/// The card-resident setup hooks of this build are never installed, so their offsets go unused.
#[allow(dead_code)]
mod flash_template {
    include!(concat!(env!("OUT_DIR"), "/xp5_internal_flash_loader.rs"));
}

/// The read-only startup build never installs its card-resident hooks or its gate either.
#[allow(dead_code)]
mod read_only_template {
    include!(concat!(env!("OUT_DIR"), "/xp5_read_only_loader.rs"));
}

/// The checked-redirect build: the read-only build plus one proven redirect of the stock reset
/// stage's section load. It never installs its card-resident hooks or its gate either.
#[allow(dead_code)]
mod checked_redirect_template {
    include!(concat!(env!("OUT_DIR"), "/xp5_checked_redirect_loader.rs"));
}

/// The copy-stub build: the checked-redirect build whose redirect is a stub of its own, written into
/// dead padding of the stock reset image. It never installs its card-resident hooks or its gate.
#[allow(dead_code)]
mod copy_stub_template {
    include!(concat!(env!("OUT_DIR"), "/xp5_copy_stub_loader.rs"));
}

/// The stub's own image (position independent code and a zero parameter block) and the offsets of
/// the parameters in it.
#[allow(dead_code)]
mod stub_template {
    include!(concat!(env!("OUT_DIR"), "/xp5_copy_stub.rs"));
}

/// The marker build: the copy-stub build plus a binding that names the words the loader changes in
/// the DRAM copy of the first section.
#[allow(dead_code)]
mod marker_loader_template {
    include!(concat!(env!("OUT_DIR"), "/xp5_marker_loader.rs"));
}

/// The marker program's own image (position independent code and a zero parameter block) and the
/// offsets of the parameters in it.
#[allow(dead_code)]
mod marker_image {
    include!(concat!(env!("OUT_DIR"), "/xp5_marker.rs"));
}

/// The OS build: the copy-stub build plus a binding for the compressed load of the OS section.
#[allow(dead_code)]
mod os_loader_template {
    include!(concat!(env!("OUT_DIR"), "/xp5_os_loader.rs"));
}

/// The OS stub's own image (position independent code and a zero parameter block) and the offsets of
/// the parameters in it.
#[allow(dead_code)]
mod os_stub_template {
    include!(concat!(env!("OUT_DIR"), "/xp5_os_stub.rs"));
}

/// The Exif build: the copy-stub build plus a binding for the named words of the OS section.
#[allow(dead_code)]
mod exif_loader_template {
    include!(concat!(env!("OUT_DIR"), "/xp5_exif_loader.rs"));
}

/// The Exif stub's own image (position independent code and a zero parameter block) and the offsets of
/// the parameters in it.
#[allow(dead_code)]
mod exif_stub_template {
    include!(concat!(env!("OUT_DIR"), "/xp5_exif_stub.rs"));
}

/// The hook build: the copy-stub build plus a binding for a hook at the entry of one OS function and a small program
/// in the OS's code padding.
#[allow(dead_code)]
mod hook_loader_template {
    include!(concat!(env!("OUT_DIR"), "/xp5_hook_loader.rs"));
}

/// The hook stub's own image (position independent code and a zero parameter block) and the offsets of
/// the parameters in it.
#[allow(dead_code)]
mod hook_stub_template {
    include!(concat!(env!("OUT_DIR"), "/xp5_hook_stub.rs"));
}

/// Where one compiled loader build keeps what the packager binds. `CARD` is the card-resident
/// loader (`template`); `INTERNAL_FLASH` is the same sources built for a startup that keeps the
/// installed flash attached (`flash_template`); `READ_ONLY` only reads the RAM image into spare
/// DRAM and checks it (`read_only_template`).
struct Loader {
    image: &'static [u8],
    prelude_stack_max: usize,
    setup_entry: usize,
    setup_configuration: usize,
    setup_return_branch: usize,
    setup_stack: usize,
    setup_stack_top: usize,
    sd_initialize: usize,
    sd_request: usize,
    sd_progress: usize,
    sd_board: usize,
    /// The host prelude, the root host setups, the stop-5 handoff ranges and the secondary-core
    /// release list: every build carries these tables, which stay empty unless the setup names them.
    sd_host_prelude: usize,
    sd_host_variants: usize,
    sd_handoff_ranges: usize,
    sd_secondary_release: usize,
    boot_ready: usize,
    boot_zero_gate: usize,
    externals: &'static [(usize, u32, &'static str)],
    /// Offset of the internal-flash binding; only the internal-flash build has one.
    internal_flash_binding: Option<usize>,
    /// Offset of the read-only binding; only the read-only and checked-redirect builds have one.
    read_only_binding: Option<usize>,
    /// Offset of the checked-redirect binding; only the checked-redirect and copy-stub builds have one.
    redirect_binding: Option<usize>,
    /// Offset of the copy-stub binding; only the copy-stub and marker builds have one.
    copy_stub_binding: Option<usize>,
    /// Offset of the marker binding; only the marker build has one.
    marker_binding: Option<usize>,
    /// Offset of the OS binding; only the OS build has one.
    os_binding: Option<usize>,
    /// Offset of the Exif binding; only the Exif build has one.
    exif_binding: Option<usize>,
    /// Offset of the hook binding; only the hook build has one.
    hook_binding: Option<usize>,
}
/// The XP5 redistributor window and the stride of its pages: an SGI frame is the second page of
/// a redistributor, `0x10000` above a `0x20000`-aligned base, and cores 1 to 3 follow each other.
/// A frame anywhere else (the flash aperture below all of it, the flash controller above) would
/// make the loader's interrupt-enable writes land on something that is not a GIC.
const GICR_WINDOW: std::ops::Range<u64> = 0xfa000000..0xfb000000;
const GICR_STRIDE: u64 = 0x20000;
fn valid_secondary_sgi(frames: &[u64; 3]) -> bool {
    frames
        .iter()
        .all(|&frame| GICR_WINDOW.contains(&frame) && frame % GICR_STRIDE == GICR_STRIDE / 2)
        && frames.windows(2).all(|pair| pair[1] == pair[0] + GICR_STRIDE)
}
const CARD: Loader = Loader {
    image: template::IMAGE,
    prelude_stack_max: template::PRELUDE_STACK_MAX,
    setup_entry: template::XP5_SETUP_ENTRY,
    setup_configuration: template::XP5_SETUP_CONFIGURATION,
    setup_return_branch: template::XP5_SETUP_RETURN_BRANCH,
    setup_stack: template::XP5_SETUP_STACK,
    setup_stack_top: template::XP5_SETUP_STACK_TOP,
    sd_initialize: template::SETUP_SD_INITIALIZE,
    sd_request: template::SETUP_SD_REQUEST,
    sd_progress: template::SETUP_SD_PROGRESS,
    sd_board: template::SETUP_SD_BOARD,
    sd_host_prelude: template::SETUP_SD_HOST_PRELUDE,
    sd_host_variants: template::SETUP_SD_HOST_VARIANTS,
    sd_handoff_ranges: template::SETUP_SD_HANDOFF_RANGES,
    sd_secondary_release: template::SETUP_SD_SECONDARY_RELEASE,
    boot_ready: template::A64_BOOT_READY,
    boot_zero_gate: template::A64_BOOT_ZERO_GATE,
    externals: template::EXTERNALS,
    internal_flash_binding: None,
    read_only_binding: None,
    redirect_binding: None,
    copy_stub_binding: None,
    marker_binding: None,
    os_binding: None,
    exif_binding: None,
    hook_binding: None,
};
const INTERNAL_FLASH: Loader = Loader {
    image: flash_template::IMAGE,
    prelude_stack_max: flash_template::PRELUDE_STACK_MAX,
    setup_entry: flash_template::XP5_SETUP_ENTRY,
    setup_configuration: flash_template::XP5_SETUP_CONFIGURATION,
    setup_return_branch: flash_template::XP5_SETUP_RETURN_BRANCH,
    setup_stack: flash_template::XP5_SETUP_STACK,
    setup_stack_top: flash_template::XP5_SETUP_STACK_TOP,
    sd_initialize: flash_template::SETUP_SD_INITIALIZE,
    sd_request: flash_template::SETUP_SD_REQUEST,
    sd_progress: flash_template::SETUP_SD_PROGRESS,
    sd_board: flash_template::SETUP_SD_BOARD,
    sd_host_prelude: flash_template::SETUP_SD_HOST_PRELUDE,
    sd_host_variants: flash_template::SETUP_SD_HOST_VARIANTS,
    sd_handoff_ranges: flash_template::SETUP_SD_HANDOFF_RANGES,
    sd_secondary_release: flash_template::SETUP_SD_SECONDARY_RELEASE,
    boot_ready: flash_template::A64_BOOT_READY,
    boot_zero_gate: flash_template::A64_BOOT_ZERO_GATE,
    externals: flash_template::EXTERNALS,
    internal_flash_binding: Some(flash_template::SETUP_SD_INTERNAL_FLASH),
    read_only_binding: None,
    redirect_binding: None,
    copy_stub_binding: None,
    marker_binding: None,
    os_binding: None,
    exif_binding: None,
    hook_binding: None,
};
const READ_ONLY: Loader = Loader {
    image: read_only_template::IMAGE,
    prelude_stack_max: read_only_template::PRELUDE_STACK_MAX,
    setup_entry: read_only_template::XP5_SETUP_ENTRY,
    setup_configuration: read_only_template::XP5_SETUP_CONFIGURATION,
    setup_return_branch: read_only_template::XP5_SETUP_RETURN_BRANCH,
    setup_stack: read_only_template::XP5_SETUP_STACK,
    setup_stack_top: read_only_template::XP5_SETUP_STACK_TOP,
    sd_initialize: read_only_template::SETUP_SD_INITIALIZE,
    sd_request: read_only_template::SETUP_SD_REQUEST,
    sd_progress: read_only_template::SETUP_SD_PROGRESS,
    sd_board: read_only_template::SETUP_SD_BOARD,
    sd_host_prelude: read_only_template::SETUP_SD_HOST_PRELUDE,
    sd_host_variants: read_only_template::SETUP_SD_HOST_VARIANTS,
    sd_handoff_ranges: read_only_template::SETUP_SD_HANDOFF_RANGES,
    sd_secondary_release: read_only_template::SETUP_SD_SECONDARY_RELEASE,
    boot_ready: read_only_template::A64_BOOT_READY,
    boot_zero_gate: read_only_template::A64_BOOT_ZERO_GATE,
    externals: read_only_template::EXTERNALS,
    internal_flash_binding: None,
    read_only_binding: Some(read_only_template::SETUP_SD_READ_ONLY),
    redirect_binding: None,
    copy_stub_binding: None,
    marker_binding: None,
    os_binding: None,
    exif_binding: None,
    hook_binding: None,
};
const CHECKED_REDIRECT: Loader = Loader {
    image: checked_redirect_template::IMAGE,
    prelude_stack_max: checked_redirect_template::PRELUDE_STACK_MAX,
    setup_entry: checked_redirect_template::XP5_SETUP_ENTRY,
    setup_configuration: checked_redirect_template::XP5_SETUP_CONFIGURATION,
    setup_return_branch: checked_redirect_template::XP5_SETUP_RETURN_BRANCH,
    setup_stack: checked_redirect_template::XP5_SETUP_STACK,
    setup_stack_top: checked_redirect_template::XP5_SETUP_STACK_TOP,
    sd_initialize: checked_redirect_template::SETUP_SD_INITIALIZE,
    sd_request: checked_redirect_template::SETUP_SD_REQUEST,
    sd_progress: checked_redirect_template::SETUP_SD_PROGRESS,
    sd_board: checked_redirect_template::SETUP_SD_BOARD,
    sd_host_prelude: checked_redirect_template::SETUP_SD_HOST_PRELUDE,
    sd_host_variants: checked_redirect_template::SETUP_SD_HOST_VARIANTS,
    sd_handoff_ranges: checked_redirect_template::SETUP_SD_HANDOFF_RANGES,
    sd_secondary_release: checked_redirect_template::SETUP_SD_SECONDARY_RELEASE,
    boot_ready: checked_redirect_template::A64_BOOT_READY,
    boot_zero_gate: checked_redirect_template::A64_BOOT_ZERO_GATE,
    externals: checked_redirect_template::EXTERNALS,
    internal_flash_binding: None,
    read_only_binding: Some(checked_redirect_template::SETUP_SD_READ_ONLY),
    redirect_binding: Some(checked_redirect_template::SETUP_SD_REDIRECT),
    copy_stub_binding: None,
    marker_binding: None,
    os_binding: None,
    exif_binding: None,
    hook_binding: None,
};
const COPY_STUB: Loader = Loader {
    image: copy_stub_template::IMAGE,
    prelude_stack_max: copy_stub_template::PRELUDE_STACK_MAX,
    setup_entry: copy_stub_template::XP5_SETUP_ENTRY,
    setup_configuration: copy_stub_template::XP5_SETUP_CONFIGURATION,
    setup_return_branch: copy_stub_template::XP5_SETUP_RETURN_BRANCH,
    setup_stack: copy_stub_template::XP5_SETUP_STACK,
    setup_stack_top: copy_stub_template::XP5_SETUP_STACK_TOP,
    sd_initialize: copy_stub_template::SETUP_SD_INITIALIZE,
    sd_request: copy_stub_template::SETUP_SD_REQUEST,
    sd_progress: copy_stub_template::SETUP_SD_PROGRESS,
    sd_board: copy_stub_template::SETUP_SD_BOARD,
    sd_host_prelude: copy_stub_template::SETUP_SD_HOST_PRELUDE,
    sd_host_variants: copy_stub_template::SETUP_SD_HOST_VARIANTS,
    sd_handoff_ranges: copy_stub_template::SETUP_SD_HANDOFF_RANGES,
    sd_secondary_release: copy_stub_template::SETUP_SD_SECONDARY_RELEASE,
    boot_ready: copy_stub_template::A64_BOOT_READY,
    boot_zero_gate: copy_stub_template::A64_BOOT_ZERO_GATE,
    externals: copy_stub_template::EXTERNALS,
    internal_flash_binding: None,
    read_only_binding: Some(copy_stub_template::SETUP_SD_READ_ONLY),
    redirect_binding: Some(copy_stub_template::SETUP_SD_REDIRECT),
    copy_stub_binding: Some(copy_stub_template::SETUP_SD_COPY_STUB),
    marker_binding: None,
    os_binding: None,
    exif_binding: None,
    hook_binding: None,
};
const MARKER: Loader = Loader {
    image: marker_loader_template::IMAGE,
    prelude_stack_max: marker_loader_template::PRELUDE_STACK_MAX,
    setup_entry: marker_loader_template::XP5_SETUP_ENTRY,
    setup_configuration: marker_loader_template::XP5_SETUP_CONFIGURATION,
    setup_return_branch: marker_loader_template::XP5_SETUP_RETURN_BRANCH,
    setup_stack: marker_loader_template::XP5_SETUP_STACK,
    setup_stack_top: marker_loader_template::XP5_SETUP_STACK_TOP,
    sd_initialize: marker_loader_template::SETUP_SD_INITIALIZE,
    sd_request: marker_loader_template::SETUP_SD_REQUEST,
    sd_progress: marker_loader_template::SETUP_SD_PROGRESS,
    sd_board: marker_loader_template::SETUP_SD_BOARD,
    sd_host_prelude: marker_loader_template::SETUP_SD_HOST_PRELUDE,
    sd_host_variants: marker_loader_template::SETUP_SD_HOST_VARIANTS,
    sd_handoff_ranges: marker_loader_template::SETUP_SD_HANDOFF_RANGES,
    sd_secondary_release: marker_loader_template::SETUP_SD_SECONDARY_RELEASE,
    boot_ready: marker_loader_template::A64_BOOT_READY,
    boot_zero_gate: marker_loader_template::A64_BOOT_ZERO_GATE,
    externals: marker_loader_template::EXTERNALS,
    internal_flash_binding: None,
    read_only_binding: Some(marker_loader_template::SETUP_SD_READ_ONLY),
    redirect_binding: Some(marker_loader_template::SETUP_SD_REDIRECT),
    copy_stub_binding: Some(marker_loader_template::SETUP_SD_COPY_STUB),
    marker_binding: Some(marker_loader_template::SETUP_SD_MARKER),
    os_binding: None,
    exif_binding: None,
    hook_binding: None,
};
const OS: Loader = Loader {
    image: os_loader_template::IMAGE,
    prelude_stack_max: os_loader_template::PRELUDE_STACK_MAX,
    setup_entry: os_loader_template::XP5_SETUP_ENTRY,
    setup_configuration: os_loader_template::XP5_SETUP_CONFIGURATION,
    setup_return_branch: os_loader_template::XP5_SETUP_RETURN_BRANCH,
    setup_stack: os_loader_template::XP5_SETUP_STACK,
    setup_stack_top: os_loader_template::XP5_SETUP_STACK_TOP,
    sd_initialize: os_loader_template::SETUP_SD_INITIALIZE,
    sd_request: os_loader_template::SETUP_SD_REQUEST,
    sd_progress: os_loader_template::SETUP_SD_PROGRESS,
    sd_board: os_loader_template::SETUP_SD_BOARD,
    sd_host_prelude: os_loader_template::SETUP_SD_HOST_PRELUDE,
    sd_host_variants: os_loader_template::SETUP_SD_HOST_VARIANTS,
    sd_handoff_ranges: os_loader_template::SETUP_SD_HANDOFF_RANGES,
    sd_secondary_release: os_loader_template::SETUP_SD_SECONDARY_RELEASE,
    boot_ready: os_loader_template::A64_BOOT_READY,
    boot_zero_gate: os_loader_template::A64_BOOT_ZERO_GATE,
    externals: os_loader_template::EXTERNALS,
    internal_flash_binding: None,
    read_only_binding: Some(os_loader_template::SETUP_SD_READ_ONLY),
    redirect_binding: Some(os_loader_template::SETUP_SD_REDIRECT),
    copy_stub_binding: Some(os_loader_template::SETUP_SD_COPY_STUB),
    marker_binding: None,
    os_binding: Some(os_loader_template::SETUP_SD_OS),
    exif_binding: None,
    hook_binding: None,
};
const EXIF: Loader = Loader {
    image: exif_loader_template::IMAGE,
    prelude_stack_max: exif_loader_template::PRELUDE_STACK_MAX,
    setup_entry: exif_loader_template::XP5_SETUP_ENTRY,
    setup_configuration: exif_loader_template::XP5_SETUP_CONFIGURATION,
    setup_return_branch: exif_loader_template::XP5_SETUP_RETURN_BRANCH,
    setup_stack: exif_loader_template::XP5_SETUP_STACK,
    setup_stack_top: exif_loader_template::XP5_SETUP_STACK_TOP,
    sd_initialize: exif_loader_template::SETUP_SD_INITIALIZE,
    sd_request: exif_loader_template::SETUP_SD_REQUEST,
    sd_progress: exif_loader_template::SETUP_SD_PROGRESS,
    sd_board: exif_loader_template::SETUP_SD_BOARD,
    sd_host_prelude: exif_loader_template::SETUP_SD_HOST_PRELUDE,
    sd_host_variants: exif_loader_template::SETUP_SD_HOST_VARIANTS,
    sd_handoff_ranges: exif_loader_template::SETUP_SD_HANDOFF_RANGES,
    sd_secondary_release: exif_loader_template::SETUP_SD_SECONDARY_RELEASE,
    boot_ready: exif_loader_template::A64_BOOT_READY,
    boot_zero_gate: exif_loader_template::A64_BOOT_ZERO_GATE,
    externals: exif_loader_template::EXTERNALS,
    internal_flash_binding: None,
    read_only_binding: Some(exif_loader_template::SETUP_SD_READ_ONLY),
    redirect_binding: Some(exif_loader_template::SETUP_SD_REDIRECT),
    copy_stub_binding: Some(exif_loader_template::SETUP_SD_COPY_STUB),
    marker_binding: None,
    os_binding: None,
    exif_binding: Some(exif_loader_template::SETUP_SD_EXIF),
    hook_binding: None,
};
const HOOK: Loader = Loader {
    image: hook_loader_template::IMAGE,
    prelude_stack_max: hook_loader_template::PRELUDE_STACK_MAX,
    setup_entry: hook_loader_template::XP5_SETUP_ENTRY,
    setup_configuration: hook_loader_template::XP5_SETUP_CONFIGURATION,
    setup_return_branch: hook_loader_template::XP5_SETUP_RETURN_BRANCH,
    setup_stack: hook_loader_template::XP5_SETUP_STACK,
    setup_stack_top: hook_loader_template::XP5_SETUP_STACK_TOP,
    sd_initialize: hook_loader_template::SETUP_SD_INITIALIZE,
    sd_request: hook_loader_template::SETUP_SD_REQUEST,
    sd_progress: hook_loader_template::SETUP_SD_PROGRESS,
    sd_board: hook_loader_template::SETUP_SD_BOARD,
    sd_host_prelude: hook_loader_template::SETUP_SD_HOST_PRELUDE,
    sd_host_variants: hook_loader_template::SETUP_SD_HOST_VARIANTS,
    sd_handoff_ranges: hook_loader_template::SETUP_SD_HANDOFF_RANGES,
    sd_secondary_release: hook_loader_template::SETUP_SD_SECONDARY_RELEASE,
    boot_ready: hook_loader_template::A64_BOOT_READY,
    boot_zero_gate: hook_loader_template::A64_BOOT_ZERO_GATE,
    externals: hook_loader_template::EXTERNALS,
    internal_flash_binding: None,
    read_only_binding: Some(hook_loader_template::SETUP_SD_READ_ONLY),
    redirect_binding: Some(hook_loader_template::SETUP_SD_REDIRECT),
    copy_stub_binding: Some(hook_loader_template::SETUP_SD_COPY_STUB),
    marker_binding: None,
    os_binding: None,
    exif_binding: None,
    hook_binding: Some(hook_loader_template::SETUP_SD_HOOK),
};
/// The DRAM a read-only startup may fill: the same two numbers as `READ_ONLY_FLOOR` and
/// `READ_ONLY_CEILING` in the loader source `read_only.h`, which refuses anything else. It is the
/// gap that a boot of the X100VI 1.31 firmware to its shooting screen never touches (found in the
/// emulator); the X-T5's `0x20000000` is below it, among the firmware's own buffers.
pub const READ_ONLY_FLOOR: u64 = 0x2445_9000;
pub const READ_ONLY_CEILING: u64 = 0x3e40_8000;
/// Longest wait of the load-failure fallback; the loader refuses a binding above it.
const MAX_FALLBACK_WAIT_SECONDS: u32 = 86_400;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Operation {
    pub address: u64,
    pub mask: u32,
    pub value: u32,
    pub kind: u32,
}
/// One root SD host setup tried in order by a diagnostic loader. Times are
/// milliseconds; `clock_configuration_field` is 0-3 or `u32::MAX` to leave
/// the field unchanged.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostVariant {
    pub identification_divider: u32,
    pub clock_configuration_field: u32,
    pub power_off_ms: u32,
    pub power_on_ms: u32,
    pub clock_on_ms: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Board {
    pub host: u64,
    pub timeout_setting: u16,
    pub operations: Vec<Operation>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodeProof {
    pub address: usize,
    pub size: usize,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceProof {
    pub source: Source,
    pub offset: usize,
    pub size: usize,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterWrite {
    pub address: u64,
    pub value: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Flash {
    pub controller: u64,
    pub aperture: u64,
    pub window_mask: u32,
    pub clock: u64,
    pub clock_commit: u64,
    pub pins: Vec<RegisterWrite>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lamp {
    pub pio_base: u64,
    pub clock_address: u64,
    pub clock_mask: u32,
    pub reset_address: u64,
    pub reset_mask: u32,
    pub green_port: u32,
    pub red_port: u32,
    pub pin: u32,
}
/// An extent of the RAM image that a stock-selector startup leaves zero.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cleared {
    pub address: usize,
    pub size: usize,
    pub what: String,
}
/// A startup that keeps the installed flash attached and runs the reset stage with the update's
/// own selector (1), not the preloaded one (0). The stock stage then loads the section table and
/// its sections from the installed flash as on a flash boot. What the card supplies is the reset
/// stage and whatever `ram_edits` keep the stage from loading over. A recipe names this when the
/// preloaded selector does not boot its body.
///
/// The loader sends the cores the boot ROM parks no wake. Under the preloaded selector the reset
/// stage's secondary-core entry waits for one; under the update's selector that entry returns at
/// once and the core goes on into the stage's own work, so the stage wakes the cores itself when
/// it is ready for them. A wake at the hand-off runs them beside core 0 too early.
///
/// The loader keeps the card host as the boot ROM left it after reading the loader from this
/// card: none of the recipe's board operations run. Those operations are the stock driver's
/// bring-up of a slot it finds unconfigured. On a body with a UHS-II slot they also enable the
/// slot's UHS-II interface, which the stock driver turns off again before its first command on
/// the UHS-I host and the loader does not.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StockSelector {
    /// SD clock divider the loader reads the card with, in place of the loader's own divider 1:
    /// the divider the body's stock driver programs for a default-speed card.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transfer_clock_divider: Option<u8>,
    /// Word edits of the RAM image, checked against it.
    pub ram_edits: Vec<image::Edit>,
    /// Extents of the RAM image after the reset stage that the card carries as zeros: data the
    /// running firmware loads from the installed flash itself.
    pub cleared: Vec<Cleared>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Setup {
    pub prefix: Source,
    pub low_length: usize,
    pub length_flags: u32,
    pub base: u64,
    pub stack: u64,
    pub bootstrap_offset: usize,
    pub initialize_offset: usize,
    pub gate_offset: usize,
    pub continuation: u64,
    pub initializer: CodeProof,
    pub delay: CodeProof,
    pub reset_dat: usize,
    pub reset_copy_size: usize,
    pub selector: usize,
    pub flash: Flash,
    pub lamp: Lamp,
    pub board: Board,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub host_prelude: Vec<Operation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub host_variants: Vec<HostVariant>,
    /// Ranges of 32-bit words that stop 5 reads after the CRC match and
    /// writes to the card's [`handoff_result::NAME`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub handoff_ranges: Vec<HandoffRange>,
    /// Aff0 of each core (1 to 3) that the loader wakes with Group 0 SGI 0
    /// once the image is in place, before the boot core runs it. A boot ROM
    /// that parks the other cores branches them to physical 0 on that SGI.
    /// Card boot releases the cores it holds itself and takes no list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secondary_release: Vec<u8>,
    pub proofs: Vec<SourceProof>,
    pub banks: Banks,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_boot: Option<CardBoot>,
    /// Word edits of the RAM image for a startup that keeps the installed flash attached
    /// (`StartupOptions::internal_flash`). The image selects the reset stage's preloaded mode,
    /// whose other differences from the update's own selector have to be undone here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub internal_flash_ram_edits: Vec<image::Edit>,
    /// When present, `Recipe::build_startup` builds the stock-selector startup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stock_selector: Option<StockSelector>,
}
/// Request ABI 3: six words, then the count and offset/length pairs of the
/// RAM prefix's zero runs, which the loader fills instead of reading.
const REQUEST_BYTES: usize = 56 + ZERO_RUNS * 16;
const ZERO_RUNS: usize = 16;
const ZERO_BLOCK: usize = 0x10000;

/// The prefix's all-zero 64 KiB blocks after its first, merged into runs; when
/// there are more runs than the request holds, the longest are kept.
fn zero_runs(prefix: &[u8]) -> Vec<(u64, u64)> {
    let mut runs: Vec<(u64, u64)> = Vec::new();
    for start in (ZERO_BLOCK..prefix.len() / ZERO_BLOCK * ZERO_BLOCK).step_by(ZERO_BLOCK) {
        if prefix[start..start + ZERO_BLOCK].iter().all(|v| *v == 0) {
            match runs.last_mut() {
                Some((offset, bytes)) if *offset + *bytes == start as u64 => {
                    *bytes += ZERO_BLOCK as u64
                }
                _ => runs.push((start as u64, ZERO_BLOCK as u64)),
            }
        }
    }
    if runs.len() > ZERO_RUNS {
        runs.sort_by_key(|run| std::cmp::Reverse(run.1));
        runs.truncate(ZERO_RUNS);
        runs.sort_unstable();
    }
    runs
}
fn put64(data: &mut [u8], at: usize, value: u64) -> Result<()> {
    extent(data, at, 8)?;
    data[at..at + 8].copy_from_slice(&value.to_le_bytes());
    Ok(())
}
fn branch(at: u64, target: u64, link: bool) -> Result<u32> {
    let delta = i128::from(target) - i128::from(at);
    require(
        delta % 4 == 0 && (-(1 << 27)..(1 << 27)).contains(&delta),
        "AArch64 branch outside immediate range",
    )?;
    Ok((if link { 0x94000000 } else { 0x14000000 }) | (((delta >> 2) as u32) & 0x03ffffff))
}
fn mmio(address: u64) -> bool {
    (0xf0000000..0xfffff000).contains(&address) && address.is_multiple_of(4)
}
fn mmio_halfword(address: u64) -> bool {
    (0xf0000000..0xfffff000).contains(&address) && address.is_multiple_of(2)
}
/// Whether the loader runs `op` as a board operation: 0 writes a word, 1 changes the masked bits
/// of a word, 2 waits `value` (or the startup delay when 0), 3 changes the masked bits of a
/// halfword, 4 polls a halfword until its masked bits equal `value`, 5 reads a word, changes its
/// masked bits and writes it back as a halfword, and 6 waits `value`.
fn board_operation(op: &Operation) -> bool {
    match op.kind {
        0 | 1 => mmio(op.address),
        2 => true,
        3 | 4 => {
            mmio_halfword(op.address) && op.mask <= u16::MAX.into() && op.value & !op.mask == 0
        }
        5 => mmio(op.address) && op.mask <= u16::MAX.into() && op.value & !op.mask == 0,
        6 => op.address == 0 && op.mask == 0 && (1..=1000).contains(&op.value),
        _ => false,
    }
}
/// Whether the loader runs `op` in the host prelude: a board operation, or 7 sets `mask` in a
/// halfword and polls its ready bit 1 up to `value` times, 8 records the word at `address` as
/// host observation `value`, or 9 continues the selected root host's setup, gated by the word at
/// `mask`, for up to `value` polls.
fn host_prelude_operation(op: &Operation, host: u64) -> bool {
    board_operation(op)
        || match op.kind {
            7 => {
                mmio_halfword(op.address)
                    && op.mask != 0
                    && op.mask <= u16::MAX.into()
                    && (1..=10_000).contains(&op.value)
            }
            8 => mmio(op.address) && op.mask == 0 && op.value < 3,
            9 => {
                op.address == host
                    && op.mask != 0
                    && op.mask & 3 == 0
                    && (1..=1000).contains(&op.value)
            }
            _ => false,
        }
}
impl Setup {
    fn verify(&self, inverted: &[u8]) -> Result<Vec<u8>> {
        let prefix = self.prefix.read(inverted)?;
        require(
            prefix.len() >= 0x800
                && self.low_length >= 4
                && self.low_length <= prefix.len() - 0x800,
            "XP5 prefix extent differs",
        )?;
        require(
            self.base.is_multiple_of(0x4000) && (0xff000000..0xffff0000).contains(&self.base),
            "Invalid XP5 SRAM base",
        )?;
        require(
            self.length_flags & !0xc0000000 == 0
                && word(&prefix, 0x10)? == self.length_flags | self.low_length as u32,
            "XP5 setup length field differs",
        )?;
        require(
            self.bootstrap_offset >= 0x800
                && self
                    .bootstrap_offset
                    .checked_add(48)
                    .is_some_and(|end| end <= self.low_length + 0x800)
                && self.gate_offset == self.bootstrap_offset + 40
                && self.gate_offset >= 0x800
                && self
                    .gate_offset
                    .checked_add(12)
                    .is_some_and(|end| end <= self.low_length + 0x800),
            "XP5 bootstrap escapes setup",
        )?;
        let mut sources = std::collections::BTreeMap::new();
        for proof in &self.proofs {
            let key = serde_json::to_string(&proof.source)
                .map_err(|e| image::Error::Invalid(e.to_string()))?;
            if !sources.contains_key(&key) {
                sources.insert(key.clone(), proof.source.read(inverted)?);
            }
            image::hash(
                extent(&sources[&key], proof.offset, proof.size)?,
                &proof.sha256,
            )?;
        }
        require(
            !self.proofs.is_empty(),
            "XP5 device bindings lack source proofs",
        )?;
        for proof in [&self.initializer, &self.delay] {
            let offset = (proof.address as u64)
                .checked_sub(self.base)
                .ok_or_else(|| image::Error::Invalid("XP5 code proof precedes setup".into()))?
                as usize;
            require(
                proof.size > 0
                    && offset
                        .checked_add(proof.size)
                        .is_some_and(|end| end <= self.low_length),
                "XP5 code proof escapes setup",
            )?;
            image::hash(extent(&prefix, offset + 0x800, proof.size)?, &proof.sha256)?;
        }
        require(
            self.initialize_offset >= 0x800
                && self.initializer.address as u64
                    == self.base + self.initialize_offset as u64 - 0x800,
            "XP5 initializer binding differs",
        )?;
        Ok(prefix)
    }
}
/// Card entry replays its checked header without probing installed flash.
/// Stage one probes installed flash before retaining its stock continuation.
pub fn package(
    inverted: &[u8],
    setup: &Setup,
    ram: Option<Vec<u8>>,
    success_hold_units: u64,
    stop: StartupStop,
    prepare_installed_flash: bool,
) -> Result<Vec<File>> {
    package_with_storage(
        inverted,
        setup,
        ram,
        success_hold_units,
        stop,
        prepare_installed_flash,
        None,
    )
}

/// `stop` ends the load at a diagnostic point instead of running the image, and
/// `prepare_installed_flash` makes the prelude set up and scan the installed flash as stage one
/// does; neither changes the SD image or its CRC.
pub(crate) fn package_with_storage(
    inverted: &[u8], setup: &Setup, ram: Option<Vec<u8>>, success_hold_units: u64,
    stop: StartupStop, prepare_installed_flash: bool, storage: Option<&[u8]>,
) -> Result<Vec<File>> {
    package_inner(
        inverted, setup, ram, success_hold_units, stop, prepare_installed_flash, storage, None, None, None,
    )
}

/// Startup with the installed flash left attached: the card supplies the RAM
/// image only. The setup prelude initializes and scans the installed flash as
/// stage one does, and the card-resident setup tables, namespace and unit
/// cache are not used, so no per-unit storage image is involved.
///
/// The loader is the internal-flash build of the loader sources. Throughout the load it keeps
/// the cores the boot ROM parks held (their interrupt enables masked), RAM 0 gated and the
/// image's reset entry back, and it wakes the cores, which the image's reset stage expects to
/// be awake, only once the CRC has passed. With `fallback_wait_seconds`, a failed FFBOOT load
/// (no file, a length or CRC mismatch, a read error) also waits that long, puts the stock reset
/// image back from the installed flash and continues the stock boot there instead of halting.
/// The recipe's card-resident setup supplies the cores' interrupt frames and is otherwise not
/// used.
pub(crate) fn package_internal_flash(
    inverted: &[u8], setup: &Setup, ram: Vec<u8>, success_hold_units: u64,
    fallback_wait_seconds: Option<u32>,
) -> Result<Vec<File>> {
    let mut setup = setup.clone();
    let ram_edits = std::mem::take(&mut setup.internal_flash_ram_edits);
    let secondary_sgi = setup
        .card_boot
        .take()
        .map(|card| card.secondary_sgi)
        .ok_or_else(|| {
            image::Error::Invalid(
                "Internal-flash startup needs the parked cores' interrupt frames, which the \
                 recipe's card-resident setup names"
                    .into(),
            )
        })?;
    // The loader wakes the cores it holds itself, once the CRC has passed.
    setup.secondary_release = Vec::new();
    setup.banks = Banks {
        copies: Vec::new(),
        edits: Vec::new(),
    };
    package_inner(
        inverted,
        &setup,
        Some(ram),
        success_hold_units,
        StartupStop::Continue,
        false,
        None,
        Some(InternalFlash {
            fallback_wait_seconds,
            ram_edits,
            secondary_sgi,
        }),
        None,
        None,
    )
}

/// The recipe's stock-selector startup (`Setup::stock_selector`): the card loader, with the
/// prelude's installed-flash setup and scan as in stage one and without the card start's setup
/// tables in SRAM, and a RAM image changed after the reset-copy comparison. The preloaded
/// selector goes back to the update's own word, the recipe's edits are applied and its cleared
/// extents are zeroed; the CRC the loader checks covers the result. The loader holds, gates and
/// wakes no core. `stop` is the card startup's diagnostic stop.
pub(crate) fn package_stock_selector(
    inverted: &[u8],
    setup: &Setup,
    ram: Vec<u8>,
    success_hold_units: u64,
    stop: StartupStop,
) -> Result<Vec<File>> {
    let mut setup = setup.clone();
    let stock = setup.stock_selector.take().ok_or_else(|| {
        image::Error::Invalid("The recipe names no stock-selector startup".into())
    })?;
    require(
        setup.card_boot.is_none(),
        "A stock-selector startup excludes a card-resident setup",
    )?;
    setup.internal_flash_ram_edits = Vec::new();
    setup.banks = Banks {
        copies: Vec::new(),
        edits: Vec::new(),
    };
    // The boot ROM read the loader from this card: its card routing stays.
    setup.board.operations = Vec::new();
    // The reset stage wakes the parked cores itself; a wake at the hand-off is too early.
    setup.secondary_release = Vec::new();
    package_inner(
        inverted,
        &setup,
        Some(ram),
        success_hold_units,
        stop,
        false,
        None,
        None,
        None,
        Some(&stock),
    )
}

/// A startup that only reads: the loader streams `ffboot.bin` into spare DRAM at `destination`,
/// checks its length and CRC-32, reads the DRAM back and checks it again, and goes back to the
/// stock bootstrap, which then starts from the installed flash as on a normal boot. The loader
/// writes no RAM 0, holds and gates no core and executes nothing it read; it halts if RAM 0 changed
/// while it ran. `ffboot.bin` is the RAM image as the stock update installs it: the recipe's plan
/// with its preloaded-selector edit undone (selector 1) and no RAM edits, so its reset-image prefix
/// is byte for byte the stock reset image in the flash. The stock continuation is left in place.
/// A failed read is shown on the lamp and the stock boot goes on all the same.
pub(crate) fn package_read_only(
    inverted: &[u8], setup: &Setup, ram: Vec<u8>, success_hold_units: u64, destination: u64,
) -> Result<Vec<File>> {
    let (setup, ram) = read_only_inputs(inverted, setup, ram)?;
    package_inner(
        inverted,
        &setup,
        Some(ram),
        success_hold_units,
        StartupStop::Continue,
        false,
        None,
        None,
        Some(ReadOnly { destination, redirect: None, stub: None, marker: None, os: None, exif: None, hook: None, file: None }),
        None,
    )
}

/// The copy-stub startup with a change to the first section: a hook at the section's entry point and a
/// short program in the zero padding of the section, which lights the lamp and returns (`marker.S`). The
/// card's `ffboot.bin` is the same stock-derived file as the copy stub's, byte for byte; the loader proves
/// the DRAM copy equal to the flash, then writes the named words into the DRAM copy itself, proves the
/// result against the flash again, and the copy stub checks the changed section at the moment of use (its
/// bound CRC-32 is the changed section's) and copies the flash's section on any difference.
pub(crate) fn package_marker(
    inverted: &[u8], setup: &Setup, ram: Vec<u8>, success_hold_units: u64, destination: u64,
    mode: CheckedRedirect,
) -> Result<Vec<File>> {
    let (setup, mut ram) = read_only_inputs(inverted, setup, ram)?;
    let bias = setup
        .reset_dat
        .checked_sub(redirect::RESET_FLASH)
        .ok_or_else(|| image::Error::Invalid("The reset image precedes its flash offset".into()))?;
    let reset = extent(&ram, 0, setup.reset_copy_size)?.to_vec();
    let plan = redirect::locate(&reset, inverted, bias, &ram)?;
    let prefix = (plan.dst as usize + plan.bytes + 0xfff) & !0xfff;
    require(prefix <= ram.len(), "The redirected section lies outside the RAM image")?;
    ram.truncate(prefix);
    let lamp = &setup.lamp;
    let stub_lamp = redirect::StubLamp {
        pio_base: lamp.pio_base,
        green_port: lamp.green_port,
        red_port: lamp.red_port,
        pin: lamp.pin,
    };
    let section = extent(inverted, plan.flash_offset as usize + bias, plan.bytes)?;
    let declared = redirect::declared_size(&plan, inverted, bias)?;
    let marker = redirect::marker_plan(
        &plan,
        section,
        declared,
        marker_image::IMAGE,
        &redirect::MarkerValues::for_lamp(&stub_lamp),
    )?;
    let binding = redirect::stub_binding_with(
        &plan,
        &reset,
        inverted,
        bias,
        setup.flash.aperture,
        destination,
        setup.reset_copy_size,
        ram.len(),
        mode == CheckedRedirect::Apply,
        &stub_lamp,
        stub_template::IMAGE,
        Some(marker.modified_crc32),
    )?;
    package_inner(
        inverted,
        &setup,
        Some(ram),
        success_hold_units,
        StartupStop::Continue,
        false,
        None,
        None,
        Some(ReadOnly {
            destination,
            redirect: Some(redirect::Binding(binding.redirect.0)),
            stub: Some(binding),
            marker: Some(marker),
            os: None,
            exif: None,
            hook: None,
            file: None,
        }),
        None,
    )
}

/// What a read-only startup of the stock update is built from: the setup without the
/// card-resident parts and the RAM image with the stock selector.
fn read_only_inputs(inverted: &[u8], setup: &Setup, ram: Vec<u8>) -> Result<(Setup, Vec<u8>)> {
    let mut setup = setup.clone();
    // The RAM edits are for a card that runs the image; this one only reads it.
    setup.internal_flash_ram_edits = Vec::new();
    // Nothing of the card-resident setup is used: no tables, no unit cache, no interrupt frames.
    setup.card_boot = None;
    // The stock boot that follows wakes the parked cores; this loader wakes none.
    setup.secondary_release = Vec::new();
    setup.banks = Banks {
        copies: Vec::new(),
        edits: Vec::new(),
    };
    // The plan's only edit of the reset image is the preloaded selector; give the stock word back.
    let mut ram = ram;
    let stock = extent(inverted, setup.reset_dat, setup.reset_copy_size)?;
    require(
        setup.selector.checked_add(8).is_some_and(|end| end <= setup.reset_copy_size)
            && setup.reset_copy_size <= ram.len()
            && word(&ram, setup.selector)? == 0
            && word(&ram, setup.selector + 4)? == 0,
        "XP5 RAM image does not carry the preloaded selector",
    )?;
    ram[setup.selector..setup.selector + 8].copy_from_slice(&stock[setup.selector..setup.selector + 8]);
    Ok((setup, ram))
}

/// Whether the loader changes RAM 0 once the redirect is proven.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CheckedRedirect {
    /// Prove the redirect (the DRAM copy equals the flash) and report it; write nothing to RAM 0.
    VerifyOnly,
    /// Also change the one word of the stock reset image that sends the stage to the DRAM copy.
    Apply,
}

/// A read-only startup plus one checked redirect: the loader proves, against the installed flash,
/// that a section of the DRAM copy it just read equals what the flash holds, and with `Apply`
/// changes the one word of the stock reset image in RAM 0 that makes the stock reset stage copy
/// that section from the DRAM copy instead of from the flash (`redirect.rs`). `ffboot.bin` is the
/// unedited image cut after that section, so the card holds only what the redirect names and the
/// file is read in a fraction of a second. Nothing the loader does can change what the stock stage
/// copies, only where it copies it from; an unproven redirect is never made.
pub(crate) fn package_checked_redirect(
    inverted: &[u8], setup: &Setup, ram: Vec<u8>, success_hold_units: u64, destination: u64,
    mode: CheckedRedirect,
) -> Result<Vec<File>> {
    let (setup, mut ram) = read_only_inputs(inverted, setup, ram)?;
    let bias = setup
        .reset_dat
        .checked_sub(redirect::RESET_FLASH)
        .ok_or_else(|| image::Error::Invalid("The reset image precedes its flash offset".into()))?;
    let reset = extent(&ram, 0, setup.reset_copy_size)?.to_vec();
    let plan = redirect::locate(&reset, inverted, bias, &ram)?;
    let prefix = (plan.dst as usize + plan.bytes + 0xfff) & !0xfff;
    require(prefix <= ram.len(), "The redirected section lies outside the RAM image")?;
    ram.truncate(prefix);
    let binding = redirect::binding(
        &plan,
        &reset,
        inverted,
        bias,
        setup.flash.aperture,
        destination,
        setup.reset_copy_size,
        ram.len(),
        mode == CheckedRedirect::Apply,
    )?;
    package_inner(
        inverted,
        &setup,
        Some(ram),
        success_hold_units,
        StartupStop::Continue,
        false,
        None,
        None,
        Some(ReadOnly { destination, redirect: Some(binding), stub: None, marker: None, os: None, exif: None, hook: None, file: None }),
        None,
    )
}

/// The checked redirect with a copy stub in place of the one-word source change. The loader
/// proves the same things (tables, the flash range, the DRAM copy equal to it, the stock reset
/// image) and, with `Apply`, writes a stub into the nop padding of the stock reset image and
/// replaces the stage's copy call by a call to it. The stub checks the DRAM copy and the destination
/// at the moment of use and copies from the flash on any difference (`redirect.rs`, `copy_stub.S`).
/// `ffboot.bin` is the same cut image as the checked redirect's.
pub(crate) fn package_copy_stub(
    inverted: &[u8], setup: &Setup, ram: Vec<u8>, success_hold_units: u64, destination: u64,
    mode: CheckedRedirect,
) -> Result<Vec<File>> {
    let (setup, mut ram) = read_only_inputs(inverted, setup, ram)?;
    let bias = setup
        .reset_dat
        .checked_sub(redirect::RESET_FLASH)
        .ok_or_else(|| image::Error::Invalid("The reset image precedes its flash offset".into()))?;
    let reset = extent(&ram, 0, setup.reset_copy_size)?.to_vec();
    let plan = redirect::locate(&reset, inverted, bias, &ram)?;
    let prefix = (plan.dst as usize + plan.bytes + 0xfff) & !0xfff;
    require(prefix <= ram.len(), "The redirected section lies outside the RAM image")?;
    ram.truncate(prefix);
    let lamp = &setup.lamp;
    let binding = redirect::stub_binding(
        &plan,
        &reset,
        inverted,
        bias,
        setup.flash.aperture,
        destination,
        setup.reset_copy_size,
        ram.len(),
        mode == CheckedRedirect::Apply,
        &redirect::StubLamp {
            pio_base: lamp.pio_base,
            green_port: lamp.green_port,
            red_port: lamp.red_port,
            pin: lamp.pin,
        },
        stub_template::IMAGE,
    )?;
    package_inner(
        inverted,
        &setup,
        Some(ram),
        success_hold_units,
        StartupStop::Continue,
        false,
        None,
        None,
        Some(ReadOnly { destination, redirect: Some(redirect::Binding(binding.redirect.0)), stub: Some(binding), marker: None, os: None, exif: None, hook: None, file: None }),
        None,
    )
}

/// The placeholder file of the Exif startup. The read-only startup reads one file before it checks the flash and
/// the OS bytes do not come from the card, so the file carries none: a short note and zeros, one 4 KiB read.
pub(crate) const EXIF_PLACEHOLDER_BYTES: usize = 4096;
fn exif_placeholder() -> Vec<u8> {
    let mut file = b"XP5 startup placeholder: this file carries no firmware bytes. The startup reads one file before it checks the flash.\n".to_vec();
    file.resize(EXIF_PLACEHOLDER_BYTES, 0);
    file
}

/// The OS section's named words, written after the stage's own decode: the stage decodes the section with its
/// hardware engine as it always does, and a stub (`exif_stub.S`) that the loader writes into dead padding checks
/// each named word of the fresh decode against its bound stock value and, only if all of them match, writes the
/// new values with the CPU and reads them back (`redirect.rs`). `ffboot.bin` is a 4 KiB placeholder; the loader
/// proves the compressed flash range, the section's row, the stage's call and the table of named words before it
/// writes anything to RAM 0.
pub(crate) fn package_exif(
    inverted: &[u8], setup: &Setup, ram: Vec<u8>, success_hold_units: u64, destination: u64,
    mode: CheckedRedirect,
) -> Result<Vec<File>> {
    let (setup, mut ram) = read_only_inputs(inverted, setup, ram)?;
    let bias = setup
        .reset_dat
        .checked_sub(redirect::RESET_FLASH)
        .ok_or_else(|| image::Error::Invalid("The reset image precedes its flash offset".into()))?;
    let reset = extent(&ram, 0, setup.reset_copy_size)?.to_vec();
    let plan = redirect::locate_exif(&reset, inverted, bias, &ram)?;
    let end = plan.os.dst as usize + plan.os.head_bytes;
    require(end <= ram.len(), "The OS section lies outside the RAM image")?;
    ram.truncate((end + 0xfff) & !0xfff);
    let file = exif_placeholder();
    let lamp = &setup.lamp;
    let redirect::ExifBinding { stub, exif } = redirect::exif_binding(
        &plan,
        &file,
        &reset,
        inverted,
        bias,
        setup.flash.aperture,
        destination,
        setup.reset_copy_size,
        mode == CheckedRedirect::Apply,
        &redirect::StubLamp {
            pio_base: lamp.pio_base,
            green_port: lamp.green_port,
            red_port: lamp.red_port,
            pin: lamp.pin,
        },
        exif_stub_template::IMAGE,
    )?;
    package_inner(
        inverted,
        &setup,
        Some(ram),
        success_hold_units,
        StartupStop::Continue,
        false,
        None,
        None,
        Some(ReadOnly {
            destination,
            redirect: Some(redirect::Binding(stub.redirect.0)),
            stub: Some(stub),
            marker: None,
            os: None,
            exif: Some(exif),
            hook: None,
            file: Some(file),
        }),
        None,
    )
}

/// A hook at the entry of one OS function and a small program in the OS's code padding, written after the stage's
/// own decode of the OS section: the stage decodes the section with its hardware engine as it always does, and a
/// stub (`hook_stub.S`) that the loader writes into dead padding checks the thirteen named words of the fresh
/// decode against their bound stock values and, only if all of them match, writes the new values with the CPU
/// and reads them back (`redirect.rs`). `ffboot.bin` is the same 4 KiB placeholder as the Exif startup's.
pub(crate) fn package_hook(
    inverted: &[u8], setup: &Setup, ram: Vec<u8>, success_hold_units: u64, destination: u64,
    mode: CheckedRedirect,
) -> Result<Vec<File>> {
    let (setup, mut ram) = read_only_inputs(inverted, setup, ram)?;
    let bias = setup
        .reset_dat
        .checked_sub(redirect::RESET_FLASH)
        .ok_or_else(|| image::Error::Invalid("The reset image precedes its flash offset".into()))?;
    let reset = extent(&ram, 0, setup.reset_copy_size)?.to_vec();
    let plan = redirect::locate_hook(&reset, inverted, bias, &ram)?;
    let end = plan.os.dst as usize + plan.os.head_bytes;
    require(end <= ram.len(), "The OS section lies outside the RAM image")?;
    ram.truncate((end + 0xfff) & !0xfff);
    let file = exif_placeholder();
    let lamp = &setup.lamp;
    let redirect::HookBinding { stub, hook } = redirect::hook_binding(
        &plan,
        &file,
        &reset,
        inverted,
        bias,
        setup.flash.aperture,
        destination,
        setup.reset_copy_size,
        mode == CheckedRedirect::Apply,
        &redirect::StubLamp {
            pio_base: lamp.pio_base,
            green_port: lamp.green_port,
            red_port: lamp.red_port,
            pin: lamp.pin,
        },
        hook_stub_template::IMAGE,
    )?;
    package_inner(
        inverted,
        &setup,
        Some(ram),
        success_hold_units,
        StartupStop::Continue,
        false,
        None,
        None,
        Some(ReadOnly {
            destination,
            redirect: Some(redirect::Binding(stub.redirect.0)),
            stub: Some(stub),
            marker: None,
            os: None,
            exif: None,
            hook: Some(hook),
            file: Some(file),
        }),
        None,
    )
}

/// The OS section's load, checked: the stage decodes the section with its hardware engine as it always
/// does, and a stub (`os_stub.S`) that the loader writes into dead padding compares that decode with the
/// DRAM copy of the section the card supplied, word for word, writes the copy over the decoded bytes
/// only if they are equal and compares again, and otherwise leaves (or restores) the decode itself
/// (`redirect.rs`). `ffboot.bin` is the decoded section's initialized bytes alone, which the loader
/// streams to `destination`; the loader proves the compressed flash range and the file against the
/// packager's CRC-32s before it writes anything to RAM 0.
pub(crate) fn package_os(
    inverted: &[u8], setup: &Setup, ram: Vec<u8>, success_hold_units: u64, destination: u64,
    mode: CheckedRedirect,
) -> Result<Vec<File>> {
    let (setup, mut ram) = read_only_inputs(inverted, setup, ram)?;
    let bias = setup
        .reset_dat
        .checked_sub(redirect::RESET_FLASH)
        .ok_or_else(|| image::Error::Invalid("The reset image precedes its flash offset".into()))?;
    let reset = extent(&ram, 0, setup.reset_copy_size)?.to_vec();
    let plan = redirect::locate_os(&reset, inverted, bias, &ram)?;
    let end = plan.dst as usize + plan.head_bytes;
    require(end <= ram.len(), "The OS section lies outside the RAM image")?;
    let head = extent(&ram, plan.dst as usize, plan.head_bytes)?.to_vec();
    ram.truncate((end + 0xfff) & !0xfff);
    let lamp = &setup.lamp;
    let redirect::OsBinding { stub, os } = redirect::os_binding(
        &plan,
        &head,
        &reset,
        inverted,
        bias,
        setup.flash.aperture,
        destination,
        setup.reset_copy_size,
        mode == CheckedRedirect::Apply,
        &redirect::StubLamp {
            pio_base: lamp.pio_base,
            green_port: lamp.green_port,
            red_port: lamp.red_port,
            pin: lamp.pin,
        },
        os_stub_template::IMAGE,
    )?;
    package_inner(
        inverted,
        &setup,
        Some(ram),
        success_hold_units,
        StartupStop::Continue,
        false,
        None,
        None,
        Some(ReadOnly {
            destination,
            redirect: Some(redirect::Binding(stub.redirect.0)),
            stub: Some(stub),
            marker: None,
            os: Some(os),
            exif: None,
            hook: None,
            file: Some(head),
        }),
        None,
    )
}

/// How a read-only startup differs from a card-resident one.
struct ReadOnly {
    /// DRAM address the loader streams the file to.
    destination: u64,
    /// The checked redirect the loader proves (and, if bound to, makes) once the file is read.
    redirect: Option<redirect::Binding>,
    /// The copy stub of the copy-stub build: with it, `redirect` is the stub build's binding of the
    /// copy call, not of the source translator.
    stub: Option<redirect::StubBinding>,
    /// The marker of the marker build: the words the loader changes in the DRAM copy of the section.
    marker: Option<redirect::MarkerPlan>,
    /// The OS build's words of the loader's OS binding.
    os: Option<[u64; 8]>,
    /// The Exif build's words of the loader's Exif binding.
    exif: Option<Vec<u64>>,
    /// The hook build's words of the loader's hook binding.
    hook: Option<Vec<u64>>,
    /// The file the loader reads, when it is not the RAM image (the OS build's file is the decoded
    /// section alone, which the loader streams to `destination`).
    file: Option<Vec<u8>>,
}

/// How an internal-flash startup differs from a card-resident one.
#[derive(Clone)]
struct InternalFlash {
    fallback_wait_seconds: Option<u32>,
    /// SGI frames of cores 1 to 3, held during the load.
    secondary_sgi: [u64; 3],
    /// Word edits of the RAM image, checked against it after the reset-copy comparison.
    ram_edits: Vec<image::Edit>,
}

/// Zero the cleared extents of a stock-selector image. Each is whole words after the reset
/// stage and inside the image; none overlaps another or an edit.
fn clear_ram(
    ram: &mut [u8], cleared: &[Cleared], edits: &[image::Edit], reset_copy_size: usize,
) -> Result<()> {
    let mut taken: Vec<(usize, usize)> = edits.iter().map(|edit| (edit.address, edit.address + 4)).collect();
    for extent in cleared {
        let end = extent.address.checked_add(extent.size).filter(|end| {
            extent.size != 0
                && extent.address.is_multiple_of(4)
                && extent.size.is_multiple_of(4)
                && extent.address >= reset_copy_size
                && *end <= ram.len()
        });
        let end = end.filter(|end| taken.iter().all(|&(low, high)| *end <= low || extent.address >= high));
        let end = end.ok_or_else(|| {
            image::Error::Invalid("Invalid cleared extent of the XP5 RAM image".into())
        })?;
        taken.push((extent.address, end));
    }
    for extent in cleared {
        ram[extent.address..extent.address + extent.size].fill(0);
    }
    Ok(())
}

/// Give the loader's card read the SD clock divider `divider` in place of its own divider 1.
/// The loader sets its transfer clock in one place: it writes clock control (host `+0x2c`) with
/// the divider in bits 15..8 and the internal clock enable, `mov w1, #0x101` and, within the next
/// eight instructions, `strh w1, [xN, #0x2c]`. The loader must hold that word exactly once and
/// the store after it, or nothing changes.
fn bind_transfer_clock(program: &mut [u8], divider: u8) -> Result<()> {
    const MOV_W1_DIVIDER_1: u32 = 0x52802021;
    require(
        (1..=0x80).contains(&divider) && divider.is_power_of_two(),
        "Invalid XP5 transfer clock divider",
    )?;
    let words = || (0..program.len() / 4).map(|n| n * 4);
    let sites: Vec<usize> =
        words().filter(|&at| word(program, at).is_ok_and(|w| w == MOV_W1_DIVIDER_1)).collect();
    let stored = sites.len() == 1
        && (1..=8).any(|n| {
            // strh w1, [xN, #0x2c]
            word(program, sites[0] + 4 * n).is_ok_and(|w| w & 0xfffffc1f == 0x79005801)
        });
    require(stored, "XP5 loader transfer clock differs")?;
    // mov w1, #(divider << 8 | 1)
    put(program, sites[0], 0x52800001 | (u32::from(divider) << 8 | 1) << 5)
}

/// Apply checked word edits to the RAM image: every old word must match, none may repeat or
/// touch the preloaded selector, and nothing changes unless all of them pass.
fn edit_ram(ram: &mut [u8], edits: &[image::Edit], selector: usize) -> Result<()> {
    let mut addresses = std::collections::BTreeSet::new();
    for edit in edits {
        require(
            edit.address.is_multiple_of(4)
                && !(selector..selector + 8).contains(&edit.address)
                && addresses.insert(edit.address)
                && word(ram, edit.address)? == edit.old,
            "XP5 RAM edit differs from source",
        )?;
    }
    for edit in edits {
        put(ram, edit.address, edit.new)?;
    }
    Ok(())
}

/// Fill the internal-flash loader's binding: the interrupt frames it holds during the load and,
/// with a fallback, where the stock reset image sits in the flash aperture and how long to wait
/// before putting it back. That image is the one the stock setup copied to RAM 0, so the loader
/// restores exactly what it overwrote (the gate stub and whatever of FFBOOT it had streamed).
fn bind_internal_flash(
    program: &mut [u8], loader: &Loader, setup: &Setup, flash: &InternalFlash, ram: Option<&[u8]>,
    inverted: &[u8],
) -> Result<()> {
    let at = loader.internal_flash_binding.ok_or_else(|| {
        image::Error::Invalid("This XP5 loader build has no internal-flash binding".into())
    })?;
    // Inputs, then the three words the loader writes (its fallback status and two CRCs).
    let empty = [1u64, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0].map(u64::to_le_bytes).concat();
    require(
        extent(program, at, 96)? == empty,
        "XP5 internal-flash binding template changed",
    )?;
    let page = |address: u64| address & !0xfff..(address & !0xfff) + 0x1000;
    let (aperture, window) = (setup.flash.aperture, u64::from(!setup.flash.window_mask) + 1);
    require(
        valid_secondary_sgi(&flash.secondary_sgi)
            && flash.secondary_sgi.iter().all(|&frame| {
                // Not the flash aperture and not the flash controller's page: a write there is
                // a flash command, not an interrupt-enable.
                !(aperture..aperture + window).contains(&frame)
                    && !page(setup.flash.controller).contains(&frame)
            }),
        "Invalid parked-core interrupt frames",
    )?;
    let (mut fallback, mut wait, mut source, mut bytes, mut crc) = (0u64, 0u64, 0u64, 0u64, 0u64);
    if let Some(seconds) = flash.fallback_wait_seconds {
        let restore_offset = setup
            .reset_dat
            .checked_sub(setup.prefix.dat_offset)
            .ok_or_else(|| {
                image::Error::Invalid("XP5 reset image precedes the update".into())
            })?;
        let window = u64::from(!setup.flash.window_mask) + 1;
        require(
            seconds <= MAX_FALLBACK_WAIT_SECONDS
                && setup.reset_copy_size > 0
                && setup.reset_copy_size % 8 == 0
                && restore_offset % 8 == 0
                && (restore_offset as u64)
                    .checked_add(setup.reset_copy_size as u64)
                    .is_some_and(|end| end <= window)
                && ram.is_some_and(|ram| setup.reset_copy_size <= ram.len()),
            "XP5 reset image is not a flash-aperture extent, or the wait is over a day",
        )?;
        (fallback, wait) = (1, u64::from(seconds));
        source = setup.flash.aperture + restore_offset as u64;
        bytes = setup.reset_copy_size as u64;
        // The loader checks the aperture against this before it gates RAM 0 and RAM 0 against it
        // after a restore, so a flash holding anything but this update's reset image (another
        // firmware, an unreadable aperture) never restores.
        crc = u64::from(crate::xp4::crc32(extent(inverted, setup.reset_dat, setup.reset_copy_size)?));
    }
    for (n, value) in [1, fallback, wait, source, bytes, crc]
        .into_iter()
        .chain(flash.secondary_sgi)
        .enumerate()
    {
        put64(program, at + n * 8, value)?;
    }
    Ok(())
}

/// Fill the read-only loader binding: where in DRAM the file goes, and what the RAM-0 guard
/// covers (the stock reset image's extent and its selector word). The loader refuses a bound
/// outside its own floor and ceiling, and so does the packager, before any file is written. The
/// last five words are the loader's own outputs.
fn bind_read_only(
    program: &mut [u8], loader: &Loader, setup: &Setup, read_only: &ReadOnly, bytes: usize,
) -> Result<()> {
    let at = loader.read_only_binding.ok_or_else(|| {
        image::Error::Invalid("This XP5 loader build has no read-only binding".into())
    })?;
    require(
        extent(program, at, 72)? == [1u64, 0, 0, 0, 0, 0, 0, 0, 0].map(u64::to_le_bytes).concat(),
        "XP5 read-only binding template changed",
    )?;
    let destination = read_only.destination;
    require(
        destination % 0x1000 == 0
            && (READ_ONLY_FLOOR..READ_ONLY_CEILING).contains(&destination)
            && bytes as u64 <= READ_ONLY_CEILING - destination,
        "The read-only destination is outside the DRAM the loader may fill",
    )?;
    require(
        setup.reset_copy_size >= 8
            && setup.reset_copy_size <= 0x10_0000
            && setup.reset_copy_size % 4 == 0
            && setup.selector % 4 == 0
            && setup.selector + 8 <= setup.reset_copy_size,
        "The read-only guard does not fit the stock reset image",
    )?;
    for (n, value) in [destination, setup.reset_copy_size as u64, setup.selector as u64]
        .into_iter()
        .enumerate()
    {
        put64(program, at + 8 + n * 8, value)?;
    }
    Ok(())
}

fn package_inner(
    inverted: &[u8], setup: &Setup, ram: Option<Vec<u8>>, success_hold_units: u64,
    stop: StartupStop, prepare_installed_flash: bool, storage: Option<&[u8]>,
    internal_flash: Option<InternalFlash>, read_only: Option<ReadOnly>,
    stock_selector: Option<&StockSelector>,
) -> Result<Vec<File>> {
    require(
        stop == StartupStop::Continue || ram.is_some(),
        "Diagnostic stop requires an SD image",
    )?;
    require(
        internal_flash.is_none() || read_only.is_none(),
        "An XP5 startup is internal-flash or read-only, not both",
    )?;
    require(
        stock_selector.is_none()
            || (internal_flash.is_none()
                && read_only.is_none()
                && ram.is_some()
                && storage.is_none()
                && setup.card_boot.is_none()),
        "The stock-selector startup is a card-loader startup without card-resident setup",
    )?;
    let prefix = setup.verify(inverted)?;
    let loader = if let Some(read_only) = &read_only {
        if read_only.hook.is_some() {
            &HOOK
        } else if read_only.exif.is_some() {
            &EXIF
        } else if read_only.os.is_some() {
            &OS
        } else if read_only.marker.is_some() {
            &MARKER
        } else if read_only.stub.is_some() {
            &COPY_STUB
        } else if read_only.redirect.is_some() {
            &CHECKED_REDIRECT
        } else {
            &READ_ONLY
        }
    } else if internal_flash.is_some() {
        &INTERNAL_FLASH
    } else {
        &CARD
    };
    let copied_end = setup.base + setup.low_length as u64;
    // XP5 raw-entry SRAM reserves its upper workspace for the boot service.
    let workspace = setup.base + 0x38800;
    let address = (copied_end.max(setup.stack + 0x100) + 0x3fff) & !0x3fff;
    let capacity = workspace.saturating_sub(address);
    require(
        copied_end < setup.stack && setup.stack < workspace && capacity >= 0x4000,
        "No XP5 extension above the stock stack and below boot workspace",
    )?;
    let count = word(&prefix, 0x14)? as usize;
    require(count <= 253, "XP5 header operations exceed header")?;
    let header_size = 0x800;
    let mut program = loader.image.to_vec();
    let header_at = program.len();
    program.extend_from_slice(extent(&prefix, 0, header_size)?);
    if internal_flash.is_some() || read_only.is_some() {
        require(
            ram.is_some() && storage.is_none() && setup.card_boot.is_none(),
            "The internal-flash startup needs a startup without card-resident setup",
        )?;
    }
    let mut resident_end = setup
        .banks
        .end(setup.low_length, (workspace - setup.base) as usize)?;
    if let Some(card) = &setup.card_boot {
        resident_end = resident_end.max(card.resident_end()?);
    }
    let end = (address + program.len() as u64).max(
        setup.base
            + if ram.is_some() {
                resident_end as u64
            } else {
                0
            },
    );
    let end = (end + 0x1ff) & !0x1ff;
    require(
        end <= address + capacity && end <= workspace,
        "XP5 SD loader exceeds SRAM workspace",
    )?;
    require(
        loader.prelude_stack_max + 512 <= loader.setup_stack_top - loader.setup_stack,
        "XP5 prelude exceeds private stack",
    )?;
    let cfg = loader.setup_configuration;
    let mut empty = vec![0; 216];
    empty[0] = 2;
    require(
        extent(&program, cfg, 216)? == empty,
        "XP5 setup configuration template changed",
    )?;
    let flash = &setup.flash;
    let lamp = &setup.lamp;
    require(
        [
            flash.controller,
            flash.aperture,
            flash.clock,
            flash.clock_commit,
            lamp.pio_base,
            lamp.clock_address,
            lamp.reset_address,
        ]
        .into_iter()
        .all(mmio)
            && flash.pins.len() == 6
            && flash.pins.iter().all(|p| mmio(p.address))
            && flash.window_mask != 0
            && lamp.clock_mask != 0
            && lamp.reset_mask != 0
            && lamp.green_port < 16
            && lamp.red_port < 16
            && lamp.green_port != 3
            && lamp.red_port != 3
            && lamp.green_port != lamp.red_port
            && lamp.pin < 8,
        "Invalid XP5 flash/lamp binding",
    )?;
    let stock = extent(&prefix, 0x800, setup.low_length)?;
    let first = word(stock, 0)?;
    let jump = branch(
        setup.base,
        address + loader.setup_entry as u64,
        false,
    )?;
    for (n, value) in [
        2,
        flash.controller,
        flash.aperture,
        u64::from(flash.window_mask),
        flash.clock,
        flash.clock_commit,
        1000000,
        setup.base,
        u64::from(first),
        u64::from(jump),
        lamp.pio_base,
        lamp.clock_address,
        u64::from(lamp.clock_mask),
        lamp.reset_address,
        u64::from(lamp.reset_mask),
    ]
    .into_iter()
    .enumerate()
    {
        put64(&mut program, cfg + n * 8, value)?;
    }
    for (n, value) in [lamp.green_port, lamp.red_port, lamp.pin, count as u32]
        .into_iter()
        .enumerate()
    {
        put(&mut program, cfg + 120 + n * 4, value)?;
    }
    for (n, pin) in flash.pins.iter().enumerate() {
        put(&mut program, cfg + 136 + n * 8, pin.address as u32)?;
        put(&mut program, cfg + 140 + n * 8, pin.value)?;
    }
    put64(&mut program, cfg + 184, address + header_at as u64)?;
    put64(
        &mut program,
        cfg + 192,
        // Each unit executes 1,000,000 four-NOP iterations. Callers that ask
        // for seconds pass 2,048 units per second (>= 1.024 s even at two
        // billion iterations per second).
        success_hold_units,
    )?;
    put64(
        &mut program,
        cfg + 200,
        // The prelude sets up and scans the installed flash whenever it stays in use, and when
        // a card startup asks for it.
        u64::from(
            ram.is_none()
                || prepare_installed_flash
                || internal_flash.is_some()
                || read_only.is_some()
                || stock_selector.is_some(),
        ),
    )?;
    put64(
        &mut program,
        cfg + 208,
        u64::from(stop != StartupStop::Continue),
    )?;
    if let Some(flash) = &internal_flash {
        bind_internal_flash(&mut program, loader, setup, flash, ram.as_deref(), inverted)?;
    }
    if let Some(read_only) = &read_only {
        let bytes = read_only
            .file
            .as_ref()
            .map_or_else(|| ram.as_ref().map_or(0, Vec::len), Vec::len);
        bind_read_only(&mut program, loader, setup, read_only, bytes)?;
        if let Some(binding) = &read_only.redirect {
            let at = loader.redirect_binding.ok_or_else(|| {
                image::Error::Invalid("This XP5 loader build has no checked-redirect binding".into())
            })?;
            redirect::bind(&mut program, at, binding)?;
        }
        if let Some(stub) = &read_only.stub {
            let at = loader.copy_stub_binding.ok_or_else(|| {
                image::Error::Invalid("This XP5 loader build has no copy-stub binding".into())
            })?;
            let max = if read_only.hook.is_some() { redirect::HOOK_STUB_MAX } else { redirect::STUB_MAX };
            redirect::bind_stub_within(&mut program, at, stub, max)?;
        }
        if let Some(marker) = &read_only.marker {
            let at = loader.marker_binding.ok_or_else(|| {
                image::Error::Invalid("This XP5 loader build has no marker binding".into())
            })?;
            redirect::bind_marker(&mut program, at, marker)?;
        }
        if let Some(exif) = &read_only.exif {
            let at = loader.exif_binding.ok_or_else(|| {
                image::Error::Invalid("This XP5 loader build has no Exif binding".into())
            })?;
            redirect::bind_exif(&mut program, at, exif)?;
        }
        if let Some(hook) = &read_only.hook {
            let at = loader.hook_binding.ok_or_else(|| {
                image::Error::Invalid("This XP5 loader build has no hook binding".into())
            })?;
            redirect::bind_hook(&mut program, at, hook)?;
        }
        if let Some(os) = &read_only.os {
            let at = loader.os_binding.ok_or_else(|| {
                image::Error::Invalid("This XP5 loader build has no OS binding".into())
            })?;
            redirect::bind_os(&mut program, at, os)?;
        }
    }
    let restore = loader.setup_return_branch;
    require(
        word(&program, restore)? == 0x14000000,
        "XP5 restore branch template changed",
    )?;
    put(
        &mut program,
        restore,
        branch(address + restore as u64, setup.base, false)?,
    )?;
    let call = setup.base + setup.bootstrap_offset as u64 - 0x800 + 36;
    let gate = setup.base + setup.gate_offset as u64 - 0x800;
    for &(at, kind, name) in loader.externals {
        let target = match name {
            "boot_setup_initialize" => setup.initializer.address as u64,
            "boot_setup_delay" => setup.delay.address as u64,
            _ => {
                return Err(image::Error::Invalid(
                    "Unexpected XP5 SD loader external".into(),
                ));
            }
        };
        put(
            &mut program,
            at,
            branch(address + at as u64, target, kind == 283)?,
        )?;
    }
    let mut raw = vec![0xff; (end - setup.base) as usize];
    raw[..stock.len()].copy_from_slice(stock);
    put(&mut raw, 0, jump)?;
    let mut files = Vec::new();
    if let Some(mut ram) = ram {
        require(
            !ram.is_empty()
                && ram.len() < 1 << 30
                && setup
                    .selector
                    .checked_add(8)
                    .is_some_and(|end| end <= setup.reset_copy_size)
                && setup.reset_copy_size <= ram.len(),
            "XP5 payload/reset extent differs",
        )?;
        let mut reset = extent(inverted, setup.reset_dat, setup.reset_copy_size)?.to_vec();
        require(
            word(&reset, setup.selector)? == 1 && word(&reset, setup.selector + 4)? == 0,
            "XP5 stock reset selector differs",
        )?;
        // The read-only startup keeps the stock selector in its image.
        if read_only.is_none() {
            put64(&mut reset, setup.selector, 0)?;
        }
        require(
            reset == ram[..setup.reset_copy_size],
            "XP5 reset copy differs beyond preloaded selector",
        )?;
        if let Some(flash) = &internal_flash {
            edit_ram(&mut ram, &flash.ram_edits, setup.selector)?;
        }
        if let Some(stock) = stock_selector {
            clear_ram(&mut ram, &stock.cleared, &stock.ram_edits, setup.reset_copy_size)?;
            edit_ram(&mut ram, &stock.ram_edits, setup.selector)?;
            // Checked above: the stock word is 1 and the plan's image carries 0.
            put64(&mut ram, setup.selector, 1)?;
        }
        require(
            word(&prefix, setup.bootstrap_offset + 36)?
                == branch(call, setup.initializer.address as u64, true)?,
            "XP5 initializer call differs",
        )?;
        let load = word(&prefix, setup.gate_offset)?;
        require(
            load & 0xff00001f == 0x5800001e && word(&prefix, setup.gate_offset + 4)? == 0xd65f03c0,
            "XP5 bootstrap continuation differs",
        )?;
        let displacement = (((load >> 5) & 0x7ffff) as i64 ^ 0x40000) - 0x40000;
        let literal = (setup.gate_offset as i64 + displacement * 4)
            .try_into()
            .map_err(|_| image::Error::Invalid("XP5 zero literal escapes prefix".into()))?;
        require(
            extent(&prefix, literal, 8)? == setup.continuation.to_le_bytes(),
            "XP5 continuation literal differs",
        )?;
        let board = &setup.board;
        let cfg = loader.sd_board;
        require(
            board.operations.len() <= 48 && board.timeout_setting <= 14 && mmio(board.host),
            "Invalid XP5 SD binding",
        )?;
        require(
            extent(&program, cfg, 16 + 48 * 24)?.iter().all(|v| *v == 0),
            "XP5 board template changed",
        )?;
        put64(&mut program, cfg, board.host)?;
        put(&mut program, cfg + 8, u32::from(board.timeout_setting))?;
        put(&mut program, cfg + 12, board.operations.len() as u32)?;
        for (n, op) in board.operations.iter().enumerate() {
            require(board_operation(op), "Invalid XP5 SD operation")?;
            let at = cfg + 16 + n * 24;
            put64(&mut program, at, op.address)?;
            put(&mut program, at + 8, op.mask)?;
            put(&mut program, at + 12, op.value)?;
            put(&mut program, at + 16, op.kind)?;
        }
        if let Some(divider) = stock_selector.and_then(|stock| stock.transfer_clock_divider) {
            bind_transfer_clock(&mut program, divider)?;
        }
        let host_prelude = loader.sd_host_prelude;
        require(
            setup.host_prelude.len() <= 16,
            "XP5 host prelude exceeds operation limit",
        )?;
        require(
            extent(&program, host_prelude, 8 + 16 * 24)?
                .iter()
                .all(|v| *v == 0),
            "XP5 host prelude template changed",
        )?;
        put(&mut program, host_prelude, setup.host_prelude.len() as u32)?;
        for (n, op) in setup.host_prelude.iter().enumerate() {
            require(
                host_prelude_operation(op, setup.board.host),
                "Invalid XP5 SD host prelude operation",
            )?;
            let at = host_prelude + 8 + n * 24;
            put64(&mut program, at, op.address)?;
            put(&mut program, at + 8, op.mask)?;
            put(&mut program, at + 12, op.value)?;
            put(&mut program, at + 16, op.kind)?;
        }
        let variants = loader.sd_host_variants;
        require(
            setup.host_variants.len() <= 8,
            "XP5 host variants exceed their limit",
        )?;
        require(
            extent(&program, variants, 8 + 8 * 24)?
                .iter()
                .all(|v| *v == 0),
            "XP5 host variant template changed",
        )?;
        put(&mut program, variants, setup.host_variants.len() as u32)?;
        for (n, variant) in setup.host_variants.iter().enumerate() {
            require(
                (1..=0xff).contains(&variant.identification_divider)
                    && (variant.clock_configuration_field <= 3
                        || variant.clock_configuration_field == u32::MAX)
                    && [
                        variant.power_off_ms,
                        variant.power_on_ms,
                        variant.clock_on_ms,
                    ]
                    .iter()
                    .all(|ms| *ms <= 2000),
                "Invalid XP5 host variant",
            )?;
            let at = variants + 8 + n * 24;
            for (k, value) in [
                variant.identification_divider,
                variant.clock_configuration_field,
                variant.power_off_ms,
                variant.power_on_ms,
                variant.clock_on_ms,
            ]
            .into_iter()
            .enumerate()
            {
                put(&mut program, at + 4 * k, value)?;
            }
        }
        let ranges = loader.sd_handoff_ranges;
        handoff_result::check(&setup.handoff_ranges)?;
        require(
            setup.handoff_ranges.is_empty()
                || matches!(stop, StartupStop::AfterCrcWithErrorPackets),
            "XP5 handoff ranges require the after-CRC packet stop",
        )?;
        require(
            extent(&program, ranges, 8 + handoff_result::RANGES * 16)?
                .iter()
                .all(|v| *v == 0),
            "XP5 handoff range template changed",
        )?;
        if !setup.handoff_ranges.is_empty() {
            put(&mut program, ranges, setup.handoff_ranges.len() as u32)?;
            put(&mut program, ranges + 4, handoff_result::BYTES as u32)?;
        }
        for (n, range) in setup.handoff_ranges.iter().enumerate() {
            put64(&mut program, ranges + 8 + n * 16, range.address)?;
            put(&mut program, ranges + 16 + n * 16, range.words)?;
        }
        let release = loader.sd_secondary_release;
        require(
            setup.secondary_release.is_empty()
                || (setup.card_boot.is_none()
                    && setup.secondary_release.is_sorted_by(|a, b| a < b)
                    && setup
                        .secondary_release
                        .iter()
                        .all(|core| (1..=3).contains(core))),
            "Invalid XP5 secondary release",
        )?;
        require(
            extent(&program, release, 8)?.iter().all(|v| *v == 0),
            "XP5 secondary release template changed",
        )?;
        let targets = setup
            .secondary_release
            .iter()
            .fold(0u64, |mask, core| mask | 1 << core);
        put64(&mut program, release, targets)?;
        let request = loader.sd_request;
        let mut empty = vec![0; REQUEST_BYTES];
        empty[0] = 3;
        require(
            extent(&program, request, REQUEST_BYTES)? == empty,
            "XP5 request template changed",
        )?;
        let override_file = read_only.as_ref().and_then(|r| r.file.clone());
        let (file_len, file_crc) = match &override_file {
            Some(file) => (file.len(), crate::xp4::crc32(file)),
            None => (ram.len(), crate::xp4::crc32(&ram)),
        };
        for (n, value) in [
            3,
            file_len as u64,
            u64::from(file_crc),
            100,
            100000,
            stop as u64,
        ]
        .into_iter()
        .enumerate()
        {
            put64(&mut program, request + n * 8, value)?;
        }
        put(
            &mut raw,
            (call - setup.base) as usize,
            branch(call, address + loader.sd_initialize as u64, true)?,
        )?;
        // The read-only startup leaves the stock continuation alone (the bootstrap's
        // `ldr x30, =0xf0027110; ret` into the flash, whose Aff0-zero path loads zero and
        // returns to RAM 0): nothing is written to RAM 0, so there is nothing to gate.
        if read_only.is_none() {
            put(
                &mut raw,
                (gate - setup.base) as usize,
                branch(gate, address + loader.boot_zero_gate as u64, false)?,
            )?;
        }
        if let Some(card) = &setup.card_boot {
            card.copy_unit_cache(storage, &mut raw, setup.low_length,
                (setup.stack-setup.base) as usize,
                (address-setup.base) as usize..(address-setup.base) as usize+program.len(),
                &setup.banks)?;
            card.install(inverted, storage, card_boot::Installation {
                base: setup.base, stock, program_address: address,
                raw: &mut raw, program: &mut program, ram: &mut ram,
            })?;
        } else {
            require(storage.is_none(), "Setup has no card storage binding")?;
        }
        // Runs come from the final file: card boot patches the RAM prefix above.
        let file = override_file.unwrap_or(ram);
        let bytes = u64::from_le_bytes(extent(&program, request + 8, 8)?.try_into().unwrap());
        let head = extent(&file, 0, bytes as usize)?;
        require(
            word(&program, request + 16)? == crate::xp4::crc32(head),
            "XP5 request CRC differs from the file it reads",
        )?;
        let runs = zero_runs(head);
        put64(&mut program, request + 48, runs.len() as u64)?;
        for (n, (offset, length)) in runs.into_iter().enumerate() {
            put64(&mut program, request + 56 + n * 16, offset)?;
            put64(&mut program, request + 64 + n * 16, length)?;
        }
        files.push(File::new("ffboot.bin", file)?);
        if !setup.handoff_ranges.is_empty() {
            files.push(File::new(handoff_result::NAME, handoff_result::pattern())?);
        }
    }
    require(
        extent(&program, loader.sd_progress, 64)?
            .iter()
            .all(|v| *v == 0)
            && word(&program, loader.boot_ready)? == 0,
        "XP5 handoff is already published",
    )?;
    let offset = (address - setup.base) as usize;
    raw[offset..offset + program.len()].copy_from_slice(&program);
    if !files.is_empty() {
        setup.banks.apply(
            &prefix,
            &mut raw,
            setup.low_length,
            (setup.stack - setup.base) as usize,
            offset,
            offset + program.len(),
        )?;
    }
    files.insert(0, File::new("marble_init.bin", raw)?);
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use card_boot::Payload;
    pub(super) fn source(bytes: &[u8]) -> Source {
        Source {
            dat_offset: 0,
            size: bytes.len(),
            compression: image::Compression::Raw,
            decoded_size: bytes.len(),
            sha256: fujimod_firmware::sha256(bytes),
            initialized_prefix: None,
        }
    }
    pub(super) fn fixture() -> (Vec<u8>, Setup, Vec<u8>) {
        let base = 0xff800000;
        let mut data = vec![0xff; 0x41000];
        for (at, value) in [
            (0x10, 0x4000),
            (0x14, 1),
            (0x18, 0xff900000),
            (0x1c, 7),
            (0x800, 0xd503201f),
            (0xa00, 0xd503201f),
            (0xb00, 0xd503201f),
            (0x924, branch(base + 0x124, base + 0x200, true).unwrap()),
            (0x928, 0x5800005e),
            (0x92c, 0xd65f03c0),
        ] {
            put(&mut data, at, value).unwrap();
        }
        put64(&mut data, 0x930, base + 0x3000).unwrap();
        put64(&mut data, 0x40040, 1).unwrap();
        let prefix = source(&data[..0x40000]);
        let setup = Setup {
            prefix: prefix.clone(),
            low_length: 0x4000,
            length_flags: 0,
            base,
            stack: base + 0x8000,
            bootstrap_offset: 0x900,
            initialize_offset: 0xa00,
            gate_offset: 0x928,
            continuation: base + 0x3000,
            initializer: CodeProof {
                address: (base + 0x200) as usize,
                size: 4,
                sha256: fujimod_firmware::sha256(&data[0xa00..0xa04]),
            },
            delay: CodeProof {
                address: (base + 0x300) as usize,
                size: 4,
                sha256: fujimod_firmware::sha256(&data[0xb00..0xb04]),
            },
            reset_dat: 0x40000,
            reset_copy_size: 0x1000,
            selector: 0x40,
            flash: Flash {
                controller: 0xff900000,
                aperture: 0xf0000000,
                window_mask: 0xf8000000,
                clock: 0xffa10000,
                clock_commit: 0xffa10040,
                pins: (0..6)
                    .map(|n| RegisterWrite {
                        address: 0xff901000 + n * 4,
                        value: 0,
                    })
                    .collect(),
            },
            lamp: Lamp {
                pio_base: 0xff902000,
                clock_address: 0xff903000,
                clock_mask: 1,
                reset_address: 0xff904000,
                reset_mask: 2,
                green_port: 4,
                red_port: 5,
                pin: 1,
            },
            board: Board {
                host: 0xff905000,
                timeout_setting: 14,
                operations: vec![Operation {
                    address: 0xff906000,
                    mask: 0,
                    value: 1,
                    kind: 0,
                }],
            },
            host_prelude: vec![],
            host_variants: vec![],
            handoff_ranges: vec![],
            secondary_release: vec![],
            proofs: vec![SourceProof {
                source: prefix,
                offset: 0,
                size: 4,
                sha256: fujimod_firmware::sha256(&data[..4]),
            }],
            banks: Banks {
                copies: vec![],
                edits: vec![],
            },
            card_boot: None,
            internal_flash_ram_edits: vec![],
            stock_selector: None,
        };
        let mut ram = data[0x40000..].to_vec();
        put64(&mut ram, 0x40, 0).unwrap();
        (data, setup, ram)
    }
    /// The recipe's card-resident setup, reduced to what an internal-flash startup reads from it:
    /// the interrupt frames of the parked cores.
    fn with_frames(mut setup: Setup) -> Setup {
        let payload = || Payload {
            source: source(&[0; 2]),
            file_offset: 0,
            destinations: [0xfb0a0000, 0xfb4a0000],
        };
        setup.card_boot = Some(CardBoot {
            hooks: vec![],
            dma: CodeProof {
                address: 0,
                size: 0,
                sha256: String::new(),
            },
            layout_marker: 0,
            jedec_id: 1,
            secondary_sgi: [0xfa0b0000, 0xfa0d0000, 0xfa0f0000],
            payloads: [payload(), payload(), payload(), payload()],
            namespace: None,
            unit_cache: None,
            runtime: None,
            ram_edits: vec![],
        });
        setup
    }

    #[test]
    fn zero_runs_merge_whole_zero_blocks_after_the_first() {
        let block = ZERO_BLOCK;
        let mut prefix = vec![0; 5 * block + 0x800];
        prefix[3 * block + 7] = 1;
        assert_eq!(
            zero_runs(&prefix),
            [
                (block as u64, 2 * block as u64),
                (4 * block as u64, block as u64)
            ]
        );
        // More runs than the request holds: the longest stay, in order.
        let mut many = vec![1; 90 * block];
        for n in 0..20 {
            let length = if n == 7 { 3 } else { 1 };
            many[(2 + 4 * n) * block..(2 + 4 * n + length) * block].fill(0);
        }
        let runs = zero_runs(&many);
        assert_eq!(runs.len(), ZERO_RUNS);
        assert!(runs.contains(&((30 * block) as u64, 3 * block as u64)));
        assert!(runs.is_sorted());
    }
    #[test]
    fn the_request_lists_the_ram_prefix_zero_runs() {
        let (data, s, mut ram) = fixture();
        ram.resize(5 * ZERO_BLOCK, 0);
        ram[3 * ZERO_BLOCK] = 9;
        let files = package(
            &data,
            &s,
            Some(ram.clone()),
            0,
            StartupStop::Continue,
            false,
        )
        .unwrap();
        let request = 0xc000 + template::SETUP_SD_REQUEST;
        let raw = &files[0].bytes;
        let value =
            |at: usize| u64::from_le_bytes(raw[request + at..request + at + 8].try_into().unwrap());
        assert_eq!((value(0), value(8), value(48)), (3, ram.len() as u64, 2));
        assert_eq!(
            [value(56), value(64), value(72), value(80)],
            [0x10000, 0x20000, 0x40000, 0x10000]
        );
        assert!(
            raw[request + 88..request + REQUEST_BYTES]
                .iter()
                .all(|v| *v == 0)
        );
    }
    #[test]
    fn startup_skips_installed_flash_probe_while_stage_one_preserves_it() {
        let (data, s, ram) = fixture();
        for payload in [None, Some(ram.clone())] {
            let combined = payload.is_some();
            let files = package(&data, &s, payload, 0, StartupStop::Continue, false).unwrap();
            let raw = &files[0].bytes;
            assert_eq!(files.len(), if combined { 2 } else { 1 });
            assert_eq!(files[0].name, "marble_init.bin");
            let source = 0xc000 + template::XP5_SETUP_CONFIGURATION + 200;
            assert_eq!(
                u64::from_le_bytes(raw[source..source + 8].try_into().unwrap()),
                u64::from(!combined)
            );
            let address = s.base + 0xc000;
            assert_eq!(
                word(raw, 0).unwrap(),
                branch(s.base, address + template::XP5_SETUP_ENTRY as u64, false).unwrap()
            );
            let mut restored = raw[..s.low_length].to_vec();
            put(&mut restored, 0, word(&data, 0x800).unwrap()).unwrap();
            if combined {
                assert_eq!(files[1].name, "ffboot.bin");
                assert_eq!(files[1].bytes, ram);
                for at in [0x124, 0x128] {
                    put(&mut restored, at, word(&data, at + 0x800).unwrap()).unwrap();
                }
                let request = 0xc000 + template::SETUP_SD_REQUEST;
                assert_eq!(
                    u64::from_le_bytes(raw[request + 8..request + 16].try_into().unwrap()),
                    ram.len() as u64
                );
                assert_eq!(word(raw, request + 16).unwrap(), crate::xp4::crc32(&ram));
            }
            assert_eq!(restored, data[0x800..0x800 + s.low_length]);
            let header = 0xc000 + template::IMAGE.len();
            assert_eq!(&raw[header..header + 0x800], &data[..0x800]);
            assert!(raw.len() <= 0x38800);
            assert_eq!(
                word(raw, 0xc000 + template::XP5_SETUP_RETURN_BRANCH).unwrap(),
                branch(
                    address + template::XP5_SETUP_RETURN_BRANCH as u64,
                    s.base,
                    false
                )
                .unwrap()
            );
        }
    }
    #[test]
    fn internal_flash_startup_probes_flash_like_stage_one_and_drops_card_resident_tables() {
        let (data, mut s, ram) = fixture();
        let plain = s.clone();
        s.banks = Banks {
            copies: vec![banks::Copy {
                source_offset: 0x1000,
                destination_offset: 0x20000,
                size: 16,
                sha256: fujimod_firmware::sha256(&data[0x1000..0x1010]),
            }],
            edits: vec![],
        };
        let flag = |raw: &[u8], configuration: usize| {
            let source = 0xc000 + configuration + 200;
            u64::from_le_bytes(raw[source..source + 8].try_into().unwrap())
        };
        let card = package(&data, &s, Some(ram.clone()), 0, StartupStop::Continue, false).unwrap();
        let flash = package_internal_flash(&data, &with_frames(s.clone()), ram.clone(), 0, None).unwrap();
        assert_eq!(
            flash.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
            ["marble_init.bin", "ffboot.bin"]
        );
        assert_eq!(flash[1].bytes, ram);
        assert_eq!(
            (
                flag(&card[0].bytes, template::XP5_SETUP_CONFIGURATION),
                flag(&flash[0].bytes, flash_template::XP5_SETUP_CONFIGURATION)
            ),
            (0, 1)
        );
        // The card start relocates the setup tables into SRAM; the flash start does not.
        assert_eq!(&card[0].bytes[0x20000..0x20010], &data[0x1000..0x1010]);
        assert!(flash[0].bytes.len() < 0x20000);
        // Against a card start with no extras, the flash-probe word differs, and so does the
        // loader build (checked in the next test).
        let bare = package(&data, &plain, Some(ram), 0, StartupStop::Continue, false).unwrap();
        assert_eq!(bare[1].bytes, flash[1].bytes);
        assert_eq!(flag(&bare[0].bytes, template::XP5_SETUP_CONFIGURATION), 0);
    }

    /// A recipe that names a stock-selector startup, and a RAM image with data after the reset
    /// stage (the fixture's image is the reset stage alone).
    fn stock_selector_fixture() -> (Vec<u8>, Setup, Vec<u8>) {
        let (data, mut setup, mut ram) = fixture();
        ram.extend(std::iter::repeat_n(0x5a, 0x2000));
        setup.stock_selector = Some(StockSelector {
            transfer_clock_divider: None,
            ram_edits: vec![image::Edit {
                address: 0x100,
                old: 0xffff_ffff,
                new: 0xd2a4_0000,
                what: "decode the section to scratch DRAM".into(),
            }],
            cleared: vec![Cleared {
                address: 0x2000,
                size: 0x800,
                what: "loaded from the installed flash".into(),
            }],
        });
        (data, setup, ram)
    }

    #[test]
    fn stock_selector_startup_keeps_the_updates_selector_and_the_installed_flash() {
        let (data, setup, ram) = stock_selector_fixture();
        let files =
            package_stock_selector(&data, &setup, ram.clone(), 0, StartupStop::Continue).unwrap();
        assert_eq!(
            files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
            ["marble_init.bin", "ffboot.bin"]
        );
        // The image: the update's own selector, the edit, the cleared extent, nothing else.
        let mut expected = ram.clone();
        put64(&mut expected, setup.selector, 1).unwrap();
        put(&mut expected, 0x100, 0xd2a4_0000).unwrap();
        expected[0x2000..0x2800].fill(0);
        assert_eq!(files[1].bytes, expected);
        assert_eq!(&files[1].bytes[..0x1000], {
            let mut reset = data[0x40000..0x41000].to_vec();
            put(&mut reset, 0x100, 0xd2a4_0000).unwrap();
            reset
        });
        // The loader is the card loader of a plain card start of the same recipe. Two things
        // differ in its file: the prelude sets up and scans the installed flash, and the request
        // carries the changed image's CRC.
        let mut plain = setup.clone();
        plain.stock_selector = None;
        let card = package(
            &data,
            &plain,
            Some(ram.clone()),
            0,
            StartupStop::Continue,
            false,
        )
        .unwrap();
        let (raw, stock) = (&card[0].bytes, &files[0].bytes);
        assert_eq!(raw.len(), stock.len());
        let source = 0xc000 + template::XP5_SETUP_CONFIGURATION + 200;
        let request = 0xc000 + template::SETUP_SD_REQUEST;
        assert_eq!((word(raw, source).unwrap(), word(stock, source).unwrap()), (0, 1));
        assert_eq!(word(stock, request + 8).unwrap() as usize, expected.len());
        assert_eq!(word(stock, request + 16).unwrap(), crate::xp4::crc32(&expected));
        // The stock-selector loader runs none of the recipe's board operations: the boot ROM's
        // card routing stays. The plain card start keeps them.
        let board = 0xc000 + template::SETUP_SD_BOARD;
        let operations = 16 + 24 * plain.board.operations.len();
        assert!(!plain.board.operations.is_empty());
        assert_eq!(word(raw, board + 12).unwrap() as usize, plain.board.operations.len());
        assert_eq!(word(stock, board + 12).unwrap(), 0);
        assert!(stock[board + 16..board + operations].iter().all(|b| *b == 0));
        assert_eq!(stock[board..board + 12], raw[board..board + 12]);
        let differing: Vec<usize> = (0..raw.len())
            .filter(|&at| raw[at] != stock[at] && !(board + 12..board + operations).contains(&at))
            .map(|at| at & !7)
            .collect();
        assert!(differing.iter().all(|at| [source, request + 16].contains(at)), "{differing:x?}");
        // It is not the internal-flash build, which holds and wakes the parked cores.
        let flash = package_internal_flash(&data, &with_frames(plain), ram.clone(), 0, None).unwrap();
        assert_ne!(flash[0].bytes.len(), stock.len());
    }

    /// A recipe's transfer clock divider changes one instruction of the loader, the divider
    /// of its clock control write, and a loader without that one write refuses.
    #[test]
    fn stock_selector_startup_binds_the_recipes_transfer_clock_divider() {
        let (data, mut setup, ram) = stock_selector_fixture();
        let own = package_stock_selector(&data, &setup, ram.clone(), 0, StartupStop::Continue).unwrap();
        setup.stock_selector.as_mut().unwrap().transfer_clock_divider = Some(4);
        let bound = package_stock_selector(&data, &setup, ram.clone(), 0, StartupStop::Continue).unwrap();
        assert_eq!(own[1].bytes, bound[1].bytes);
        let (own, bound) = (&own[0].bytes, &bound[0].bytes);
        let differing: Vec<usize> = (0..own.len()).filter(|&at| own[at] != bound[at]).map(|at| at & !3).collect();
        assert!(!differing.is_empty() && differing.iter().all(|at| *at == differing[0]), "{differing:x?}");
        // mov w1, #0x101 becomes mov w1, #0x401.
        assert_eq!(word(own, differing[0]).unwrap(), 0x52802021);
        assert_eq!(word(bound, differing[0]).unwrap(), 0x52808021);
        for divider in [0, 3, 0xff] {
            setup.stock_selector.as_mut().unwrap().transfer_clock_divider = Some(divider);
            assert!(package_stock_selector(&data, &setup, ram.clone(), 0, StartupStop::Continue).is_err(), "{divider}");
        }
        let mut program = template::IMAGE.to_vec();
        bind_transfer_clock(&mut program, 4).unwrap();
        // The bound loader no longer holds the divider-1 write.
        assert!(bind_transfer_clock(&mut program, 4).is_err());
        assert!(bind_transfer_clock(&mut [0; 64], 4).is_err());
    }

    #[test]
    fn stock_selector_startup_refuses_what_it_cannot_check() {
        let (data, setup, ram) = stock_selector_fixture();
        let with = |change: &dyn Fn(&mut StockSelector)| {
            let mut setup = setup.clone();
            change(setup.stock_selector.as_mut().unwrap());
            package_stock_selector(&data, &setup, ram.clone(), 0, StartupStop::Continue)
        };
        assert!(with(&|_| {}).is_ok());
        // An edit whose old word differs, or one of the selector.
        assert!(with(&|s| s.ram_edits[0].old = 0x1234_5678).is_err());
        let selector = setup.selector;
        assert!(with(&|s| s.ram_edits[0].address = selector).is_err());
        // A cleared extent inside the reset stage, past the image, unaligned, empty or overlapping.
        assert!(with(&|s| s.cleared[0].address = 0x800).is_err());
        assert!(with(&|s| s.cleared[0].size = 0x2000).is_err());
        assert!(with(&|s| s.cleared[0].address = 0x2002).is_err());
        assert!(with(&|s| s.cleared[0].size = 0).is_err());
        assert!(with(&|s| {
            let again = s.cleared[0].clone();
            s.cleared.push(again);
        })
        .is_err());
        assert!(with(&|s| s.ram_edits[0].address = 0x2400).is_err());
        // A recipe without the startup, or with a card-resident setup as well.
        let mut plain = setup.clone();
        plain.stock_selector = None;
        assert!(
            package_stock_selector(&data, &plain, ram.clone(), 0, StartupStop::Continue).is_err()
        );
        assert!(
            package_stock_selector(
                &data,
                &with_frames(setup.clone()),
                ram.clone(),
                0,
                StartupStop::Continue
            )
            .is_err()
        );
        // Nothing of it reaches the other startups of the same recipe.
        let card = package(
            &data,
            &setup,
            Some(ram.clone()),
            0,
            StartupStop::Continue,
            false,
        )
        .unwrap();
        assert_eq!(card[1].bytes, ram);
    }

    #[test]
    fn internal_flash_applies_the_recipes_ram_edits_checked_and_in_the_crc() {
        let (data, s, ram) = fixture();
        let edit = |address: usize, old: u32| image::Edit {
            address,
            old,
            new: 0x1400_0068,
            what: "branch to the retained-metadata path".into(),
        };
        let with = |edits: Vec<image::Edit>| {
            let mut setup = with_frames(s.clone());
            setup.internal_flash_ram_edits = edits;
            setup
        };
        let plain = package_internal_flash(&data, &with_frames(s.clone()), ram.clone(), 0, None).unwrap();
        let edited =
            package_internal_flash(&data, &with(vec![edit(0x100, 0xffff_ffff)]), ram.clone(), 0, None)
                .unwrap();
        let mut expected = ram.clone();
        put(&mut expected, 0x100, 0x1400_0068).unwrap();
        assert_eq!(edited[1].bytes, expected);
        assert_eq!(plain[1].bytes, ram);
        let request = 0xc000 + flash_template::SETUP_SD_REQUEST;
        assert_eq!(word(&edited[0].bytes, request + 16).unwrap(), crate::xp4::crc32(&expected));
        assert_ne!(
            word(&edited[0].bytes, request + 16).unwrap(),
            word(&plain[0].bytes, request + 16).unwrap()
        );
        assert_eq!(edited[0].bytes.len(), plain[0].bytes.len());
        // A card start does not use them.
        let mut card_setup = s.clone();
        card_setup.internal_flash_ram_edits = vec![edit(0x100, 0xffff_ffff)];
        let card = package(
            &data,
            &card_setup,
            Some(ram.clone()),
            0,
            StartupStop::Continue,
            false,
        )
        .unwrap();
        assert_eq!(card[1].bytes, ram);
        // A changed old word, a repeat or the selector refuses.
        for edits in [
            vec![edit(0x100, 0x1234_5678)],
            vec![edit(0x100, 0xffff_ffff), edit(0x100, 0xffff_ffff)],
            vec![edit(s.selector, 0)],
        ] {
            assert!(package_internal_flash(&data, &with(edits), ram.clone(), 0, None).is_err());
        }
    }

    #[test]
    fn ram_edits_change_nothing_unless_every_one_passes() {
        let mut ram = vec![0xff; 0x20];
        let edit = |address: usize, old: u32| image::Edit {
            address,
            old,
            new: 0,
            what: "edit".into(),
        };
        let before = ram.clone();
        assert!(edit_ram(&mut ram, &[edit(0, 0xffff_ffff), edit(4, 1)], 0x40).is_err());
        assert_eq!(ram, before);
        assert!(edit_ram(&mut ram, &[edit(2, 0xffff_ffff)], 0x40).is_err());
        assert!(edit_ram(&mut ram, &[edit(0x20, 0xffff_ffff)], 0x40).is_err());
        edit_ram(&mut ram, &[edit(0, 0xffff_ffff), edit(8, 0xffff_ffff)], 0x40).unwrap();
        assert_eq!(&ram[..12], &[0, 0, 0, 0, 255, 255, 255, 255, 0, 0, 0, 0]);
    }

    #[test]
    fn boot_files_never_include_the_update_and_internal_flash_takes_no_unit_storage() {
        use crate::StartupOptions;
        let (data, s, _) = fixture();
        let update = fujimod_firmware::invert(&data);
        let mut recipe = crate::Recipe::bundled("x100vi-131").unwrap().unwrap();
        recipe.source_size = update.len();
        recipe.source_sha256 = fujimod_firmware::sha256(&update);
        recipe.updater = None;
        recipe.plan = Some(image::Plan {
            size: 0x1000,
            entry: 0,
            regions: vec![image::Region {
                name: "reset".into(),
                address: 0,
                source: Source {
                    dat_offset: 0x40000,
                    size: 0x1000,
                    compression: image::Compression::Raw,
                    decoded_size: 0x1000,
                    sha256: fujimod_firmware::sha256(&data[0x40000..0x41000]),
                    initialized_prefix: None,
                },
            }],
            bss: vec![],
            edits: vec![image::Edit {
                address: 0x40,
                old: 1,
                new: 0,
                what: "preloaded selector".into(),
            }],
        });
        recipe.xp5 = Some(with_frames(s));
        let options = StartupOptions {
            internal_flash: true,
            ..StartupOptions::default()
        };
        let names = |files: &[File]| files.iter().map(|f| f.name.clone()).collect::<Vec<_>>();
        let everything = recipe.build_startup_with_options(&update, options).unwrap();
        assert_eq!(names(&everything), ["marble_init.bin", "ffboot.bin", "FPUPDATE.DAT"]);
        let boot = recipe.build_boot_files(&update, options).unwrap();
        assert_eq!(names(&boot), ["marble_init.bin", "ffboot.bin"]);
        assert_eq!(boot[0].bytes, everything[0].bytes);
        assert_eq!(boot[1].bytes, everything[1].bytes);
        assert!(recipe.build_startup_with_storage(&update, &[0; 8], options).is_err());
        let mut stage_one = recipe.clone();
        stage_one.generation = crate::Generation::Xp4;
        assert!(stage_one.build_boot_files(&update, options).is_err());
    }

    /// Every word of the internal-flash loader the packager may touch: the words of every build
    /// and its own binding.
    fn bound_ranges() -> Vec<std::ops::Range<usize>> {
        let mut ranges = shared_bound_ranges(&INTERNAL_FLASH);
        ranges.push(
            flash_template::SETUP_SD_INTERNAL_FLASH..flash_template::SETUP_SD_INTERNAL_FLASH + 96,
        );
        ranges
    }

    /// The words every loader build lets the packager touch: the setup configuration, the SD
    /// board, host prelude, root host setup, handoff range, secondary release and request tables,
    /// the restore branch and the calls into the stock setup.
    fn shared_bound_ranges(loader: &Loader) -> Vec<std::ops::Range<usize>> {
        let mut ranges = vec![
            loader.setup_configuration..loader.setup_configuration + 216,
            loader.sd_board..loader.sd_board + 16 + 48 * 24,
            loader.sd_host_prelude..loader.sd_host_prelude + 8 + 16 * 24,
            loader.sd_host_variants..loader.sd_host_variants + 8 + 8 * 24,
            loader.sd_handoff_ranges..loader.sd_handoff_ranges + 8 + handoff_result::RANGES * 16,
            loader.sd_secondary_release..loader.sd_secondary_release + 8,
            loader.sd_request..loader.sd_request + REQUEST_BYTES,
            loader.setup_return_branch..loader.setup_return_branch + 4,
        ];
        ranges.extend(loader.externals.iter().map(|&(at, _, _)| at..at + 4));
        ranges
    }

    #[test]
    fn the_internal_flash_loader_binds_its_frames_and_the_fallback_only_on_request() {
        let (data, s, ram) = fixture();
        let setup = with_frames(s.clone());
        let at = 0xc000 + flash_template::SETUP_SD_INTERNAL_FLASH;
        let field = |raw: &[u8], n: usize| {
            u64::from_le_bytes(raw[at + 8 * n..at + 8 * n + 8].try_into().unwrap())
        };
        let gate = (s.gate_offset - 0x800) as usize;
        let address = s.base + 0xc000;
        for wait in [None, Some(7), Some(0), Some(MAX_FALLBACK_WAIT_SECONDS)] {
            let files = package_internal_flash(&data, &setup, ram.clone(), 0, wait).unwrap();
            let raw = &files[0].bytes;
            assert_eq!(files[1].bytes, ram);
            assert!(raw.len() <= 0x38800);
            assert_eq!(field(raw, 0), 1);
            let restore = s.flash.aperture + (s.reset_dat - s.prefix.dat_offset) as u64;
            let measured = u64::from(crate::xp4::crc32(
                &data[s.reset_dat..s.reset_dat + s.reset_copy_size],
            ));
            let expected = match wait {
                None => [0, 0, 0, 0, 0],
                Some(wait) => [1, u64::from(wait), restore, s.reset_copy_size as u64, measured],
            };
            assert_eq!((1..6).map(|n| field(raw, n)).collect::<Vec<_>>(), expected);
            assert_eq!(
                (6..9).map(|n| field(raw, n)).collect::<Vec<_>>(),
                [0xfa0b0000, 0xfa0d0000, 0xfa0f0000]
            );
            // The loader's own status words start empty.
            assert_eq!((9..12).map(|n| field(raw, n)).collect::<Vec<_>>(), [0, 0, 0]);
            // The bootstrap's gate is the loader's own.
            assert_eq!(
                word(raw, gate).unwrap(),
                branch(
                    s.base + gate as u64,
                    address + flash_template::A64_BOOT_ZERO_GATE as u64,
                    false
                )
                .unwrap()
            );
            // The loader is the compiled internal-flash build: nothing outside the bound ranges
            // differs from it, and nothing is appended.
            let bound = bound_ranges();
            let loader = &raw[0xc000..0xc000 + flash_template::IMAGE.len()];
            for at in (0..loader.len()).step_by(4) {
                if !bound.iter().any(|range| range.contains(&at)) {
                    assert_eq!(word(loader, at).unwrap(), word(flash_template::IMAGE, at).unwrap(), "{at:#x}");
                }
            }
            let header = 0xc000 + flash_template::IMAGE.len();
            assert!(raw[header + 0x800..].iter().all(|&byte| byte == 0xff));
        }
    }

    #[test]
    fn the_internal_flash_loader_needs_frames_the_fallback_a_flash_extent_and_a_bounded_wait() {
        let (data, s, ram) = fixture();
        // No card-resident setup in the recipe: no interrupt frames to hold.
        assert!(package_internal_flash(&data, &s, ram.clone(), 0, None).is_err());
        let setup = with_frames(s.clone());
        assert!(package_internal_flash(&data, &setup, ram.clone(), 0, Some(1)).is_ok());
        let frames = |change: &dyn Fn(&mut [u64; 3])| {
            let mut setup = setup.clone();
            change(&mut setup.card_boot.as_mut().unwrap().secondary_sgi);
            setup
        };
        for bad in [
            frames(&|sgi| sgi[1] = sgi[0]),
            frames(&|sgi| sgi[2] = 0xfa0f0010),
            frames(&|sgi| sgi[0] = 0x1000_0000),
            frames(&|sgi| sgi[0] = 0),
            // The flash aperture, the flash controller's page, a frame outside the
            // redistributor window and frames that are not one stride apart are refused.
            frames(&|sgi| sgi[0] = 0xf0010000),
            frames(&|sgi| sgi[0] = 0xf7ff0000),
            frames(&|sgi| sgi[1] = 0xff910000),
            frames(&|sgi| sgi[2] = 0xfa110000),
            frames(&|sgi| *sgi = [0xfb010000, 0xfb030000, 0xfb050000]),
            // Correctly strided, second-page frames that lie wholly in the flash aperture: only
            // the window clause refuses them.
            frames(&|sgi| *sgi = [0xf0010000, 0xf0030000, 0xf0050000]),
            frames(&|sgi| *sgi = [0xfa0b0000, 0xfa0f0000, 0xfa0d0000]),
        ] {
            assert!(package_internal_flash(&data, &bad, ram.clone(), 0, None).is_err());
        }
        // The wait is bounded at a day.
        assert!(package_internal_flash(&data, &setup, ram.clone(), 0, Some(MAX_FALLBACK_WAIT_SECONDS + 1)).is_err());
        // The stock reset image has to be a flash-aperture extent that the RAM image covers.
        let mut narrow = setup.clone();
        narrow.flash.window_mask = 0xffff_f000;
        assert!(package_internal_flash(&data, &narrow, ram.clone(), 0, None).is_ok());
        assert!(package_internal_flash(&data, &narrow, ram.clone(), 0, Some(1)).is_err());
        let mut odd = setup.clone();
        odd.reset_copy_size = 0xffc;
        assert!(package_internal_flash(&data, &odd, ram.clone(), 0, None).is_ok());
        assert!(package_internal_flash(&data, &odd, ram.clone(), 0, Some(1)).is_err());
    }

    #[test]
    fn the_internal_flash_loader_asset_keeps_its_binding_template_and_the_default_loader_has_none() {
        // The packager refuses a loader whose binding is not the empty ABI-1 template.
        assert_eq!(flash_template::SETUP_SD_INTERNAL_FLASH % 8, 0);
        assert_eq!(
            &flash_template::IMAGE[flash_template::SETUP_SD_INTERNAL_FLASH
                ..flash_template::SETUP_SD_INTERNAL_FLASH + 96],
            [1u64, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0].map(u64::to_le_bytes).concat()
        );
        assert_eq!(INTERNAL_FLASH.internal_flash_binding, Some(flash_template::SETUP_SD_INTERNAL_FLASH));
        assert_eq!(CARD.internal_flash_binding, None);
        // The default loader is the card-resident build: its layout names no such binding.
        let layout = include_str!("../assets/sd-boot-v1-xp5-loader.generated.json");
        assert!(!layout.contains("setup_sd_internal_flash"));
        assert_ne!(template::IMAGE, flash_template::IMAGE);
        // Both builds share the prelude the packager bounds and configures.
        assert_eq!(template::PRELUDE_STACK_MAX, flash_template::PRELUDE_STACK_MAX);
        assert_eq!(
            template::XP5_SETUP_RETURN_BRANCH,
            flash_template::XP5_SETUP_RETURN_BRANCH
        );
    }

    #[test]
    fn success_hold_is_optional_and_changes_only_its_configuration() {
        let (data, s, ram) = fixture();
        assert_eq!(crate::StartupOptions::default().success_hold_seconds, 0);
        for payload in [None, Some(ram)] {
            let normal =
                package(&data, &s, payload.clone(), 0, StartupStop::Continue, false).unwrap();
            let at = 0xc000 + template::XP5_SETUP_CONFIGURATION + 192;
            assert_eq!(&normal[0].bytes[at..at + 8], &[0; 8]);
            for units in [1, 32, 6_144, u64::from(u32::MAX) * 2_048] {
                let held = package(
                    &data,
                    &s,
                    payload.clone(),
                    units,
                    StartupStop::Continue,
                    false,
                )
                .unwrap();
                let mut expected = normal[0].bytes.clone();
                put64(&mut expected, at, units).unwrap();
                assert_eq!(expected, held[0].bytes);
                if normal.len() == 2 {
                    assert_eq!(normal[1].bytes, held[1].bytes);
                }
            }
        }
    }
    #[test]
    fn diagnostic_stops_change_only_the_stop_request() {
        let (data, setup, ram) = fixture();
        let normal = package(
            &data,
            &setup,
            Some(ram.clone()),
            0,
            StartupStop::Continue,
            false,
        )
        .unwrap();
        let at = 0xc000 + template::SETUP_SD_REQUEST + 40;
        for stop in [
            StartupStop::AfterCardInit,
            StartupStop::AfterCardInitWithErrorBlink,
            StartupStop::AfterSetup,
            StartupStop::AfterCrc,
        ] {
            assert!(package(&data, &setup, None, 0, stop, false).is_err());
            let checked = package(&data, &setup, Some(ram.clone()), 0, stop, false).unwrap();
            let mut expected = normal[0].bytes.clone();
            put64(
                &mut expected,
                0xc000 + template::XP5_SETUP_CONFIGURATION + 208,
                1,
            )
            .unwrap();
            put64(&mut expected, at, stop as u64).unwrap();
            assert_eq!(checked[0].bytes, expected);
            assert_eq!(checked[1].bytes, normal[1].bytes);
        }
    }
    #[test]
    fn installed_flash_preparation_changes_only_the_entry_source() {
        let (data, setup, ram) = fixture();
        let normal = package(
            &data,
            &setup,
            Some(ram.clone()),
            0,
            StartupStop::AfterCrc,
            false,
        )
        .unwrap();
        let prepared = package(&data, &setup, Some(ram), 0, StartupStop::AfterCrc, true).unwrap();
        let mut expected = normal[0].bytes.clone();
        put64(
            &mut expected,
            0xc000 + template::XP5_SETUP_CONFIGURATION + 200,
            1,
        )
        .unwrap();
        assert_eq!(prepared[0].bytes, expected);
        assert_eq!(prepared[1].bytes, normal[1].bytes);
    }

    #[test]
    fn startup_accepts_a_shorter_checked_setup_prefix() {
        let (data, mut setup, ram) = fixture();
        setup.prefix = source(&data[..0x28000]);
        setup.proofs[0].source = setup.prefix.clone();
        assert!(package(&data, &setup, Some(ram), 0, StartupStop::Continue, false).is_ok());
        setup.prefix = source(&data[..0x800]);
        setup.proofs[0].source = setup.prefix.clone();
        assert!(package(&data, &setup, None, 0, StartupStop::Continue, false).is_err());
    }

    #[test]
    fn changed_source_and_incomplete_bindings_refuse() {
        let (data, s, ram) = fixture();
        let mut changed = data.clone();
        changed[0x1000] ^= 1;
        assert!(package(&changed, &s, None, 0, StartupStop::Continue, false).is_err());
        let mut changed = s.clone();
        changed.flash.pins.clear();
        assert!(package(&data, &changed, None, 0, StartupStop::Continue, false).is_err());
        let mut changed = s.clone();
        changed.initializer.sha256 = "0".repeat(64);
        assert!(package(&data, &changed, None, 0, StartupStop::Continue, false).is_err());
        let mut changed = s.clone();
        changed.board.operations[0].kind = 7;
        assert!(
            package(
                &data,
                &changed,
                Some(ram.clone()),
                0,
                StartupStop::Continue,
                false
            )
            .is_err()
        );
        let mut changed = s.clone();
        changed.gate_offset = 0x940;
        assert!(package(&data, &changed, Some(ram), 0, StartupStop::Continue, false).is_err());
        let mut changed = s.clone();
        changed.stack = s.base + 0x38000;
        assert!(package(&data, &changed, None, 0, StartupStop::Continue, false).is_err());
    }

    #[test]
    fn board_operations_accept_checked_halfword_widths() {
        let (data, mut setup, ram) = fixture();
        setup.board.operations = vec![
            Operation {
                address: 0xff906002,
                mask: 0x1100,
                value: 0x1100,
                kind: 3,
            },
            Operation {
                address: 0xff906004,
                mask: 1,
                value: 1,
                kind: 5,
            },
        ];
        package(
            &data,
            &setup,
            Some(ram.clone()),
            0,
            StartupStop::Continue,
            false,
        )
        .unwrap();
        setup.board.operations[0].mask = 0x10000;
        assert!(
            package(
                &data,
                &setup,
                Some(ram.clone()),
                0,
                StartupStop::Continue,
                false
            )
            .is_err()
        );
        setup.board.operations[0].mask = 0x1100;
        setup.board.operations[1].address += 2;
        assert!(package(&data, &setup, Some(ram), 0, StartupStop::Continue, false).is_err());
    }
    #[test]
    fn host_prelude_serializes_separately_and_enforces_its_limit() {
        let (data, mut setup, ram) = fixture();
        setup.host_prelude = vec![Operation {
            address: 0xff907000,
            mask: 0x10,
            value: 0x10,
            kind: 1,
        }];
        let files = package(
            &data,
            &setup,
            Some(ram.clone()),
            0,
            StartupStop::Continue,
            false,
        )
        .unwrap();
        let at = 0xc000 + template::SETUP_SD_HOST_PRELUDE;
        assert_eq!(word(&files[0].bytes, at).unwrap(), 1);
        assert_eq!(word(&files[0].bytes, at + 4).unwrap(), 0);
        assert_eq!(
            u64::from_le_bytes(files[0].bytes[at + 8..at + 16].try_into().unwrap()),
            0xff907000
        );
        assert_eq!(word(&files[0].bytes, at + 16).unwrap(), 0x10);
        assert_eq!(word(&files[0].bytes, at + 20).unwrap(), 0x10);
        assert_eq!(word(&files[0].bytes, at + 24).unwrap(), 1);

        setup.host_prelude = vec![
            Operation {
                address: 0xff907000,
                mask: 0,
                value: 0,
                kind: 0,
            };
            17
        ];
        assert!(package(&data, &setup, Some(ram), 0, StartupStop::Continue, false).is_err());
    }
    #[test]
    fn host_variants_serialize_in_order_and_are_bounded() {
        let (data, mut setup, ram) = fixture();
        let variant = HostVariant {
            identification_divider: 0x3e,
            clock_configuration_field: u32::MAX,
            power_off_ms: 500,
            power_on_ms: 250,
            clock_on_ms: 10,
        };
        setup.host_variants = vec![variant.clone(); 2];
        setup.host_variants[0].identification_divider = 0xfa;
        let build = |setup: &Setup| {
            package(
                &data,
                setup,
                Some(ram.clone()),
                0,
                StartupStop::Continue,
                false,
            )
        };
        let files = build(&setup).unwrap();
        let at = 0xc000 + template::SETUP_SD_HOST_VARIANTS;
        let words: Vec<u32> = (0..14)
            .map(|n| word(&files[0].bytes, at + 4 * n).unwrap())
            .collect();
        assert_eq!(
            words,
            [
                2,
                0,
                0xfa,
                u32::MAX,
                500,
                250,
                10,
                0,
                0x3e,
                u32::MAX,
                500,
                250,
                10,
                0
            ]
        );
        for broken in [
            HostVariant {
                identification_divider: 0,
                ..variant.clone()
            },
            HostVariant {
                identification_divider: 0x100,
                ..variant.clone()
            },
            HostVariant {
                clock_configuration_field: 4,
                ..variant.clone()
            },
            HostVariant {
                power_off_ms: 2001,
                ..variant.clone()
            },
        ] {
            setup.host_variants = vec![broken];
            assert!(build(&setup).is_err());
        }
        setup.host_variants = vec![variant; 9];
        assert!(build(&setup).is_err());
    }
    #[test]
    fn handoff_ranges_serialize_and_add_the_result_file_for_the_after_crc_stop() {
        let (data, mut setup, ram) = fixture();
        setup.handoff_ranges = vec![
            HandoffRange {
                address: 0xff90e104,
                words: 3,
            },
            HandoffRange {
                address: 0xfa0b0100,
                words: 1,
            },
        ];
        let build = |setup: &Setup, stop| package(&data, setup, Some(ram.clone()), 0, stop, false);
        let files = build(&setup, StartupStop::AfterCrcWithErrorPackets).unwrap();
        let at = 0xc000 + template::SETUP_SD_HANDOFF_RANGES;
        let words: Vec<u32> = (0..10)
            .map(|n| word(&files[0].bytes, at + 4 * n).unwrap())
            .collect();
        assert_eq!(
            words,
            [2, 1 << 20, 0xff90e104, 0, 3, 0, 0xfa0b0100, 0, 1, 0]
        );
        let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            ["marble_init.bin", "ffboot.bin", handoff_result::NAME]
        );
        assert_eq!(files[2].bytes, handoff_result::pattern());
        assert!(build(&setup, StartupStop::Continue).is_err());
        setup.handoff_ranges[1].address = 0xfa0b0102;
        assert!(build(&setup, StartupStop::AfterCrcWithErrorPackets).is_err());
        setup.handoff_ranges = vec![];
        let plain = build(&setup, StartupStop::AfterCrcWithErrorPackets).unwrap();
        assert_eq!(plain.len(), 2);
        assert_eq!(word(&plain[0].bytes, at).unwrap(), 0);
    }
    #[test]
    fn secondary_release_writes_the_sgi_target_list() {
        let (data, mut setup, ram) = fixture();
        let build = |setup: &Setup| {
            package(
                &data,
                setup,
                Some(ram.clone()),
                0,
                StartupStop::Continue,
                false,
            )
        };
        let at = 0xc000 + template::SETUP_SD_SECONDARY_RELEASE;
        let plain = build(&setup).unwrap();
        assert_eq!(word(&plain[0].bytes, at).unwrap(), 0);
        setup.secondary_release = vec![1, 2, 3];
        let files = build(&setup).unwrap();
        assert_eq!(word(&files[0].bytes, at).unwrap(), 0xe);
        assert_eq!(word(&files[0].bytes, at + 4).unwrap(), 0);
        for broken in [vec![0], vec![4], vec![2, 1], vec![1, 1]] {
            setup.secondary_release = broken;
            assert!(build(&setup).is_err());
        }
    }
    #[test]
    fn startups_that_wake_no_core_at_the_hand_off_drop_the_secondary_release() {
        let (data, mut setup, ram) = stock_selector_fixture();
        setup.secondary_release = vec![1, 2, 3];
        let release = |raw: &[u8], at: usize| word(raw, 0xc000 + at).unwrap();
        let stock =
            package_stock_selector(&data, &setup, ram.clone(), 0, StartupStop::Continue).unwrap();
        assert_eq!(
            release(&stock[0].bytes, template::SETUP_SD_SECONDARY_RELEASE),
            0
        );
        let only = package_read_only(&data, &setup, ram.clone(), 0, 0x3000_0000).unwrap();
        assert_eq!(
            release(
                &only[0].bytes,
                read_only_template::SETUP_SD_SECONDARY_RELEASE
            ),
            0
        );
        let flash =
            package_internal_flash(&data, &with_frames(setup.clone()), ram.clone(), 0, None)
                .unwrap();
        assert_eq!(
            release(&flash[0].bytes, flash_template::SETUP_SD_SECONDARY_RELEASE),
            0
        );
        setup.stock_selector = None;
        let card = package(&data, &setup, Some(ram), 0, StartupStop::Continue, false).unwrap();
        assert_eq!(
            release(&card[0].bytes, template::SETUP_SD_SECONDARY_RELEASE),
            0xe
        );
    }
    #[test]
    fn the_stock_selector_startup_takes_the_diagnostic_stop() {
        let (data, setup, ram) = stock_selector_fixture();
        let stopped =
            package_stock_selector(&data, &setup, ram.clone(), 0, StartupStop::AfterCrc).unwrap();
        let raw = &stopped[0].bytes;
        let stop = 0xc000 + template::XP5_SETUP_CONFIGURATION + 208;
        assert_eq!(word(raw, stop).unwrap(), 1);
        assert_eq!(
            word(raw, 0xc000 + template::SETUP_SD_REQUEST + 40).unwrap(),
            2
        );
        let plain = package_stock_selector(&data, &setup, ram, 0, StartupStop::Continue).unwrap();
        assert_eq!(word(&plain[0].bytes, stop).unwrap(), 0);
    }
    #[test]
    fn host_prelude_handshake_serializes_and_bounds_attempts() {
        let (data, mut setup, ram) = fixture();
        setup.host_prelude = vec![
            Operation {
                address: 0xff9b222c,
                mask: 1,
                value: 1000,
                kind: 7,
            },
            Operation {
                address: 0xff9b2244,
                mask: 0,
                value: 0,
                kind: 8,
            },
        ];
        let files = package(
            &data,
            &setup,
            Some(ram.clone()),
            0,
            StartupStop::AfterCrcWithErrorPackets,
            false,
        )
        .unwrap();
        let at = 0xc000 + template::SETUP_SD_HOST_PRELUDE;
        assert_eq!(word(&files[0].bytes, at).unwrap(), 2);
        assert_eq!(word(&files[0].bytes, at + 16).unwrap(), 1);
        assert_eq!(word(&files[0].bytes, at + 20).unwrap(), 1000);
        assert_eq!(word(&files[0].bytes, at + 24).unwrap(), 7);
        assert_eq!(word(&files[0].bytes, at + 8 + 24 + 12).unwrap(), 0);
        assert_eq!(word(&files[0].bytes, at + 8 + 24 + 16).unwrap(), 8);
        setup.host_prelude[1].value = 3;
        assert!(
            package(
                &data,
                &setup,
                Some(ram.clone()),
                0,
                StartupStop::AfterCrcWithErrorPackets,
                false
            )
            .is_err()
        );
        setup.host_prelude[1].value = 0;
        setup.host_prelude[0].value = 0;
        assert!(
            package(
                &data,
                &setup,
                Some(ram.clone()),
                0,
                StartupStop::AfterCrcWithErrorPackets,
                false
            )
            .is_err()
        );
        setup.host_prelude[0].value = 1000;
        setup.host_prelude[0].address += 1;
        assert!(
            package(
                &data,
                &setup,
                Some(ram),
                0,
                StartupStop::AfterCrcWithErrorPackets,
                false
            )
            .is_err()
        );
    }

    #[test]
    fn host_continuation_requires_selected_root_and_bounded_gate() {
        let (data, mut setup, ram) = fixture();
        setup.host_prelude = vec![Operation {
            address: setup.board.host,
            mask: 0x205c344,
            value: 1000,
            kind: 9,
        }];
        assert!(
            package(
                &data,
                &setup,
                Some(ram.clone()),
                0,
                StartupStop::AfterCrcWithErrorPackets,
                false
            )
            .is_ok()
        );
        setup.host_prelude[0].address += 4;
        assert!(
            package(
                &data,
                &setup,
                Some(ram.clone()),
                0,
                StartupStop::AfterCrcWithErrorPackets,
                false
            )
            .is_err()
        );
        setup.host_prelude[0].address = setup.board.host;
        setup.host_prelude[0].mask |= 1;
        assert!(
            package(
                &data,
                &setup,
                Some(ram.clone()),
                0,
                StartupStop::AfterCrcWithErrorPackets,
                false
            )
            .is_err()
        );
        setup.host_prelude[0].mask &= !1;
        setup.host_prelude[0].value = 1001;
        assert!(
            package(
                &data,
                &setup,
                Some(ram),
                0,
                StartupStop::AfterCrcWithErrorPackets,
                false
            )
            .is_err()
        );
    }
    #[test]
    fn oversized_header_and_changed_payload_refuse() {
        let (mut data, mut s, mut ram) = fixture();
        ram[0x20] ^= 1;
        assert!(package(&data, &s, Some(ram), 0, StartupStop::Continue, false).is_err());
        put(&mut data, 0x14, 254).unwrap();
        s.prefix = source(&data[..0x40000]);
        s.proofs[0].source = s.prefix.clone();
        assert!(package(&data, &s, None, 0, StartupStop::Continue, false).is_err());
    }

    /// Every word of the read-only loader the packager may touch.
    fn read_only_bound_ranges() -> Vec<std::ops::Range<usize>> {
        use read_only_template as t;
        let mut ranges = shared_bound_ranges(&READ_ONLY);
        ranges.push(t::SETUP_SD_READ_ONLY..t::SETUP_SD_READ_ONLY + 72);
        ranges
    }

    #[test]
    fn the_read_only_startup_binds_a_destination_and_leaves_the_stock_continuation_alone() {
        let (data, s, ram) = fixture();
        let at = 0xc000 + read_only_template::SETUP_SD_READ_ONLY;
        let gate = (s.gate_offset - 0x800) as usize;
        let call = (s.bootstrap_offset - 0x800 + 36) as usize;
        let address = s.base + 0xc000;
        // The recipe's card-resident setup (interrupt frames) is not needed and not used.
        for setup in [s.clone(), with_frames(s.clone())] {
            for destination in [READ_ONLY_FLOOR, 0x3000_0000, READ_ONLY_CEILING - 0x1000] {
                let files = package_read_only(&data, &setup, ram.clone(), 0, destination).unwrap();
                assert_eq!(
                    files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
                    ["marble_init.bin", "ffboot.bin"]
                );
                let raw = &files[0].bytes;
                // The RAM image as the update installs it: the stock selector, no edits, so its
                // reset-image prefix is the stock reset image byte for byte.
                assert_eq!(files[1].bytes, data[0x40000..0x41000].to_vec());
                assert_ne!(files[1].bytes, ram);
                assert!(raw.len() <= 0x38800);
                let field = |n: usize| u64::from_le_bytes(raw[at + 8 * n..at + 8 * n + 8].try_into().unwrap());
                // The binding: the destination, then what the RAM-0 guard covers (the stock reset
                // image's extent and its selector word); the loader's five output words start empty.
                assert_eq!(
                    (0..4).map(field).collect::<Vec<_>>(),
                    [1, destination, s.reset_copy_size as u64, s.selector as u64]
                );
                assert!((4..9).all(|n| field(n) == 0));
                // The stock initializer's call goes to the loader; the stock continuation
                // (`ldr x30, =0xf0027110; ret`, into the flash) is untouched, so no gate waits
                // and nothing goes to RAM 0.
                assert_eq!(
                    word(raw, call).unwrap(),
                    branch(
                        s.base + call as u64,
                        address + read_only_template::SETUP_SD_INITIALIZE as u64,
                        true
                    )
                    .unwrap()
                );
                assert_eq!(word(raw, gate).unwrap(), word(&data, gate + 0x800).unwrap());
                assert_eq!(word(raw, gate + 4).unwrap(), word(&data, gate + 0x804).unwrap());
                // It probes the installed flash like stage one and requests the whole file.
                let flag = 0xc000 + read_only_template::XP5_SETUP_CONFIGURATION + 200;
                assert_eq!(u64::from_le_bytes(raw[flag..flag + 8].try_into().unwrap()), 1);
                let request = 0xc000 + read_only_template::SETUP_SD_REQUEST;
                assert_eq!(
                    u64::from_le_bytes(raw[request + 8..request + 16].try_into().unwrap()),
                    ram.len() as u64
                );
                assert_eq!(word(raw, request + 16).unwrap(), crate::xp4::crc32(&files[1].bytes));
                // The loader is the compiled read-only build: nothing outside the bound ranges
                // differs from it, and nothing is appended.
                let bound = read_only_bound_ranges();
                let loader = &raw[0xc000..0xc000 + read_only_template::IMAGE.len()];
                for at in (0..loader.len()).step_by(4) {
                    if !bound.iter().any(|range| range.contains(&at)) {
                        assert_eq!(
                            word(loader, at).unwrap(),
                            word(read_only_template::IMAGE, at).unwrap(),
                            "{at:#x}"
                        );
                    }
                }
                let header = 0xc000 + read_only_template::IMAGE.len();
                assert!(raw[header + 0x800..].iter().all(|&byte| byte == 0xff));
            }
        }
        // Unlike an internal-flash startup's image, which carries the preloaded selector and the
        // recipe's edits so that it could run, this one differs from it by exactly those.
        let flash = package_internal_flash(&data, &with_frames(s.clone()), ram.clone(), 0, None).unwrap();
        let only = package_read_only(&data, &s, ram.clone(), 0, 0x3000_0000).unwrap();
        assert_eq!(flash[1].bytes, ram);
        let differing: Vec<usize> =
            (0..ram.len()).filter(|&n| flash[1].bytes[n] != only[1].bytes[n]).collect();
        assert_eq!(differing, [s.selector]);
        assert_ne!(flash[0].bytes, only[0].bytes);
    }

    #[test]
    fn the_read_only_startup_ignores_the_recipes_ram_edits_and_keeps_the_stock_selector() {
        let (data, s, ram) = fixture();
        let mut setup = s.clone();
        setup.internal_flash_ram_edits = vec![image::Edit {
            address: 0x100,
            old: 0xffff_ffff,
            new: 0x1400_0068,
            what: "branch to the retained-metadata path".into(),
        }];
        let only = package_read_only(&data, &setup, ram.clone(), 0, 0x3000_0000).unwrap();
        let plain = package_read_only(&data, &s, ram.clone(), 0, 0x3000_0000).unwrap();
        assert_eq!(only[1].bytes, plain[1].bytes);
        assert_eq!(only[1].bytes, data[0x40000..0x41000].to_vec());
        assert_eq!(only[0].bytes, plain[0].bytes);
        let request = 0xc000 + read_only_template::SETUP_SD_REQUEST;
        assert_eq!(word(&only[0].bytes, request + 16).unwrap(), crate::xp4::crc32(&only[1].bytes));
        // An image that does not carry the preloaded selector (so is not the plan's) refuses, and
        // so does one whose reset prefix differs from the stock image anywhere else.
        let mut stock_selector = ram.clone();
        put64(&mut stock_selector, s.selector, 1).unwrap();
        assert!(package_read_only(&data, &s, stock_selector, 0, 0x3000_0000).is_err());
        let mut changed = ram.clone();
        changed[0x100] ^= 1;
        assert!(package_read_only(&data, &s, changed, 0, 0x3000_0000).is_err());
    }

    #[test]
    fn the_read_only_destination_has_to_lie_wholly_in_the_dram_the_loader_may_fill() {
        let (data, s, ram) = fixture();
        let size = ram.len() as u64;
        assert_eq!(size, 0x1000);
        assert_eq!((READ_ONLY_FLOOR, READ_ONLY_CEILING), (0x2445_9000, 0x3e40_8000));
        for good in [READ_ONLY_FLOOR, READ_ONLY_CEILING - size, 0x3000_0000] {
            assert!(package_read_only(&data, &s, ram.clone(), 0, good).is_ok(), "{good:#x}");
        }
        for bad in [
            0,
            0x1000,
            0x0690_f000,
            // the X-T5's address, where the X100VI keeps live-view buffers, and the first
            // megabytes above the stock RAM image
            0x2000_0000,
            0x1000_0000,
            READ_ONLY_FLOOR - 0x1000,
            // not 4 KiB aligned
            0x3000_0004,
            0x2000_0800,
            // the file would run past the ceiling, or start at or beyond it
            READ_ONLY_CEILING - size + 0x1000,
            READ_ONLY_CEILING,
            0xf000_0000,
            0xfa00_0000,
            u64::MAX,
        ] {
            assert!(package_read_only(&data, &s, ram.clone(), 0, bad).is_err(), "{bad:#x}");
        }
    }

    #[test]
    fn the_read_only_guard_needs_a_reset_image_that_holds_its_selector_word() {
        let (data, s, ram) = fixture();
        assert!(package_read_only(&data, &s, ram.clone(), 0, 0x3000_0000).is_ok());
        // A guard shorter than the 8-byte selector word cannot hold it (the loader would
        // underflow `guard_bytes - 8`), whatever else is true of the image.
        for size in [0usize, 4, 7] {
            let mut short = s.clone();
            short.reset_copy_size = size;
            assert!(package_read_only(&data, &short, ram.clone(), 0, 0x3000_0000).is_err(), "{size}");
        }
        // The selector word has to lie wholly inside the guarded extent and be word aligned.
        for (selector, size) in [(0x40usize, 0x44usize), (0x42, 0x1000), (0xff8, 0x1000)] {
            let mut odd = s.clone();
            odd.selector = selector;
            odd.reset_copy_size = size;
            assert!(package_read_only(&data, &odd, ram.clone(), 0, 0x3000_0000).is_err(), "{selector:#x} {size:#x}");
        }
    }

    #[test]
    fn the_read_only_loader_asset_keeps_its_binding_template_and_no_other_build_has_one() {
        use read_only_template as t;
        assert_eq!(t::SETUP_SD_READ_ONLY % 8, 0);
        assert_eq!(
            &t::IMAGE[t::SETUP_SD_READ_ONLY..t::SETUP_SD_READ_ONLY + 72],
            [1u64, 0, 0, 0, 0, 0, 0, 0, 0].map(u64::to_le_bytes).concat()
        );
        assert_eq!(READ_ONLY.read_only_binding, Some(t::SETUP_SD_READ_ONLY));
        assert_eq!((CARD.read_only_binding, INTERNAL_FLASH.read_only_binding), (None, None));
        assert_eq!(READ_ONLY.internal_flash_binding, None);
        let layout = include_str!("../assets/sd-boot-v1-xp5-read-only-loader.generated.json");
        assert!(layout.contains("setup_sd_read_only") && !layout.contains("setup_sd_internal_flash"));
        assert!(!include_str!("../assets/sd-boot-v1-xp5-loader.generated.json").contains("setup_sd_read_only"));
        assert!(!include_str!("../assets/sd-boot-v1-xp5-internal-flash-loader.generated.json")
            .contains("setup_sd_read_only"));
        assert_ne!(t::IMAGE, template::IMAGE);
        assert_ne!(t::IMAGE, flash_template::IMAGE);
        // Same prelude as the other two builds.
        assert_eq!(t::PRELUDE_STACK_MAX, template::PRELUDE_STACK_MAX);
        assert_eq!(t::XP5_SETUP_RETURN_BRANCH, template::XP5_SETUP_RETURN_BRANCH);
    }

    /// The synthetic update of `redirect::tests::stage` in the xp5 fixture's setup: the stock reset
    /// stage with the section load, both table copies, both tables and a raw section 0.
    fn redirect_fixture() -> (Vec<u8>, Setup, Vec<u8>) {
        let (fixture_data, mut s, _) = fixture();
        let (reset, mut update, _) = redirect::tests::stage(0);
        let ram_size = 0x8_1000 + redirect::tests::E2E_SECTION_BYTES;
        // The fixture's first 0x40000 bytes stay (its prefix proofs hash them); the stage sits where
        // the fixture's reset image does, with the selector word 1.
        update[..0x40000].copy_from_slice(&fixture_data[..0x40000]);
        let mut stock = reset.clone();
        put64(&mut stock, s.selector, 1).unwrap();
        update[0x40000..0x40000 + stock.len()].copy_from_slice(&stock);
        s.reset_copy_size = 0xb000;
        let mut ram = vec![0u8; ram_size];
        ram[..0xb000].copy_from_slice(&stock);
        put64(&mut ram, s.selector, 0).unwrap();
        ram[0x4_0000..0x4_0800].copy_from_slice(&update[0x64_0000..0x64_0800]);
        ram[0x8_1000..].copy_from_slice(&update[0xc_0000..0xc_0000 + redirect::tests::E2E_SECTION_BYTES]);
        (update, s, ram)
    }

    #[test]
    fn the_checked_redirect_packages_a_cut_image_and_binds_every_proof() {
        let (data, s, ram) = redirect_fixture();
        let at = 0xc000 + checked_redirect_template::SETUP_SD_REDIRECT;
        let read_only = 0xc000 + checked_redirect_template::SETUP_SD_READ_ONLY;
        for (mode, apply) in [(CheckedRedirect::Apply, 1u64), (CheckedRedirect::VerifyOnly, 0)] {
            let files = package_checked_redirect(&data, &s, ram.clone(), 0, 0x3000_0000, mode).unwrap();
            assert_eq!(
                files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
                ["marble_init.bin", "ffboot.bin"]
            );
            // The image is cut after the section, and is the unedited image's prefix.
            let ffboot = &files[1].bytes;
            assert_eq!(ffboot.len(), 0x8_3000);
            let mut expected = ram.clone();
            put64(&mut expected, s.selector, 1).unwrap();
            assert_eq!(ffboot[..], expected[..0x8_3000]);
            let marble = &files[0].bytes;
            let word64 = |at: usize, n: usize| u64::from_le_bytes(marble[at + 8 * n..at + 8 * n + 8].try_into().unwrap());
            assert_eq!((word64(at, 0), word64(at, 1), word64(at, 2)), (1, apply, 0x2cf0));
            assert_eq!(word64(at, 4), 0xb264_0680);
            assert_eq!((word64(at, 7), word64(at, 8), word64(at, 9)), (0x8_1000, 0x2000, 0xf00c_0000));
            // The outputs stay zero, the request names the cut file, the read-only binding the destination.
            assert!((16..20).all(|n| word64(at, n) == 0));
            let request = 0xc000 + checked_redirect_template::SETUP_SD_REQUEST;
            assert_eq!(
                (word64(request, 1), word64(request, 2)),
                (ffboot.len() as u64, u64::from(crate::xp4::crc32(ffboot)))
            );
            assert_eq!(word64(read_only, 1), 0x3000_0000);
        }
        // The two modes differ in the apply word alone.
        let apply = package_checked_redirect(&data, &s, ram.clone(), 0, 0x3000_0000, CheckedRedirect::Apply).unwrap();
        let verify = package_checked_redirect(&data, &s, ram.clone(), 0, 0x3000_0000, CheckedRedirect::VerifyOnly).unwrap();
        let differing: Vec<usize> =
            (0..apply[0].bytes.len()).filter(|&n| apply[0].bytes[n] != verify[0].bytes[n]).collect();
        assert_eq!(differing, [at + 8]);
        assert_eq!(apply[1].bytes, verify[1].bytes);
    }

    #[test]
    fn the_checked_redirect_refuses_what_it_cannot_prove() {
        let (data, s, ram) = redirect_fixture();
        let go = |data: &[u8], s: &Setup, ram: Vec<u8>, destination: u64| {
            package_checked_redirect(data, s, ram, 0, destination, CheckedRedirect::Apply)
        };
        assert!(go(&data, &s, ram.clone(), 0x3000_0000).is_ok());
        // A destination outside the window, unaligned, not one run of ones, or one that overlaps the
        // section's own address.
        for bad in [0x2000_0000, 0x3000_0004, 0x3000_1000, READ_ONLY_CEILING, 0x2445_9000] {
            assert!(go(&data, &s, ram.clone(), bad).is_err(), "{bad:#x}");
        }
        // The card's section differs from the flash's, the cold table differs, the stage differs.
        let mut bad = ram.clone();
        bad[0x8_1000 + 5] ^= 1;
        assert!(go(&data, &s, bad, 0x3000_0000).is_err());
        let mut bad = ram.clone();
        bad[0x4_0058] ^= 1;
        assert!(go(&data, &s, bad, 0x3000_0000).is_err());
        let mut other = data.clone();
        other[0x40000 + 0x2cf0] ^= 0x20;
        let mut ram_other = ram.clone();
        ram_other[0x2cf0] ^= 0x20;
        assert!(go(&other, &s, ram_other, 0x3000_0000).is_err());
        // The plain fixture has no such stage at all.
        let (plain, plain_setup, plain_ram) = fixture();
        assert!(go(&plain, &plain_setup, plain_ram, 0x3000_0000).is_err());
    }

    #[test]
    fn the_checked_redirect_loader_asset_keeps_its_two_binding_templates_and_no_other_build_has_them() {
        use checked_redirect_template as t;
        assert_eq!((t::SETUP_SD_REDIRECT % 8, t::SETUP_SD_READ_ONLY % 8), (0, 0));
        let mut template = vec![1u64];
        template.resize(20, 0);
        assert_eq!(
            &t::IMAGE[t::SETUP_SD_REDIRECT..t::SETUP_SD_REDIRECT + 160],
            template.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>()
        );
        assert_eq!(CHECKED_REDIRECT.redirect_binding, Some(t::SETUP_SD_REDIRECT));
        assert_eq!(CHECKED_REDIRECT.read_only_binding, Some(t::SETUP_SD_READ_ONLY));
        assert_eq!(
            (CARD.redirect_binding, INTERNAL_FLASH.redirect_binding, READ_ONLY.redirect_binding),
            (None, None, None)
        );
        let layout = include_str!("../assets/sd-boot-v1-xp5-checked-redirect-loader.generated.json");
        assert!(layout.contains("setup_sd_redirect") && layout.contains("setup_sd_read_only"));
        for other in [
            include_str!("../assets/sd-boot-v1-xp5-loader.generated.json"),
            include_str!("../assets/sd-boot-v1-xp5-internal-flash-loader.generated.json"),
            include_str!("../assets/sd-boot-v1-xp5-read-only-loader.generated.json"),
        ] {
            assert!(!other.contains("setup_sd_redirect"));
        }
    }

    #[test]
    fn the_copy_stub_packages_the_same_cut_image_and_binds_the_stub_and_every_proof() {
        let (data, s, ram) = redirect_fixture();
        let at = 0xc000 + copy_stub_template::SETUP_SD_REDIRECT;
        let stub_at = 0xc000 + copy_stub_template::SETUP_SD_COPY_STUB;
        let read_only = 0xc000 + copy_stub_template::SETUP_SD_READ_ONLY;
        let checked = package_checked_redirect(&data, &s, ram.clone(), 0, 0x3000_0000, CheckedRedirect::Apply).unwrap();
        for (mode, apply) in [(CheckedRedirect::Apply, 1u64), (CheckedRedirect::VerifyOnly, 0)] {
            let files = package_copy_stub(&data, &s, ram.clone(), 0, 0x3000_0000, mode).unwrap();
            assert_eq!(
                files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
                ["marble_init.bin", "ffboot.bin"]
            );
            // The file is the checked redirect's cut image, byte for byte.
            assert_eq!(files[1].bytes, checked[1].bytes);
            let marble = &files[0].bytes;
            let word64 = |at: usize, n: usize| u64::from_le_bytes(marble[at + 8 * n..at + 8 * n + 8].try_into().unwrap());
            // The call word is the copy call (0x2d04), its stock word and a `bl` to the stub at 0x200.
            assert_eq!((word64(at, 0), word64(at, 1), word64(at, 2)), (1, apply, 0x2d04));
            assert_eq!(word64(at, 3), 0x9400_0000 | (((0xa090i64 - 0x2d04) / 4) as u64 & 0x03ff_ffff));
            assert_eq!(word64(at, 4), 0x9400_0000 | (((0x200i64 - 0x2d04) / 4) as u64 & 0x03ff_ffff));
            assert_eq!((word64(at, 7), word64(at, 8), word64(at, 9)), (0x8_1000, 0x2000, 0xf00c_0000));
            assert!((16..20).all(|n| word64(at, n) == 0));
            // The stub binding: where, how long, the filler word, the CRC-32, the bytes, zero outputs.
            let len = stub_template::IMAGE.len() as u64;
            assert_eq!((word64(stub_at, 0), word64(stub_at, 1), word64(stub_at, 2), word64(stub_at, 3)), (1, 0x200, len, 0xd503_201f));
            let image = &marble[stub_at + 48..stub_at + 48 + len as usize];
            assert_eq!(word64(stub_at, 4), u64::from(crate::xp4::crc32(image)));
            assert_eq!(word64(stub_at, 5), stub_template::XP5_STUB_PARAMS as u64);
            assert_eq!(image[..stub_template::XP5_STUB_PARAMS], stub_template::IMAGE[..stub_template::XP5_STUB_PARAMS]);
            assert!(marble[stub_at + 48 + len as usize..stub_at + 48 + 1024 + 32].iter().all(|b| *b == 0));
            assert_eq!(word64(read_only, 1), 0x3000_0000);
            let request = 0xc000 + copy_stub_template::SETUP_SD_REQUEST;
            assert_eq!(
                (word64(request, 1), word64(request, 2)),
                (files[1].bytes.len() as u64, u64::from(crate::xp4::crc32(&files[1].bytes)))
            );
        }
        let apply = package_copy_stub(&data, &s, ram.clone(), 0, 0x3000_0000, CheckedRedirect::Apply).unwrap();
        let verify = package_copy_stub(&data, &s, ram.clone(), 0, 0x3000_0000, CheckedRedirect::VerifyOnly).unwrap();
        let differing: Vec<usize> =
            (0..apply[0].bytes.len()).filter(|&n| apply[0].bytes[n] != verify[0].bytes[n]).collect();
        assert_eq!(differing, [at + 8]);
        // The loader itself is a build of its own: the checked-redirect card's loader differs.
        assert_ne!(apply[0].bytes, checked[0].bytes);
    }

    #[test]
    fn the_copy_stub_refuses_what_it_cannot_prove_or_place() {
        let (data, s, ram) = redirect_fixture();
        let go = |data: &[u8], s: &Setup, ram: Vec<u8>, destination: u64| {
            package_copy_stub(data, s, ram, 0, destination, CheckedRedirect::Apply)
        };
        assert!(go(&data, &s, ram.clone(), 0x3000_0000).is_ok());
        for bad in [0x2000_0000, 0x3000_0004, READ_ONLY_CEILING, 0x2445_9000 - 0x1000] {
            assert!(go(&data, &s, ram.clone(), bad).is_err(), "{bad:#x}");
        }
        let mut bad = ram.clone();
        bad[0x8_1000 + 5] ^= 1;
        assert!(go(&data, &s, bad, 0x3000_0000).is_err());
        // A stage whose padding is not nops, or which refers to it.
        for (at, value) in [(0x3c0usize, 0xaa1f_03e1u32), (0x3000, 0x9400_0000 | (((0x400i64 - 0x3000) / 4) as u32 & 0x03ff_ffff))] {
            let mut other = data.clone();
            other[0x40000 + at..0x40000 + at + 4].copy_from_slice(&value.to_le_bytes());
            let mut ram_other = ram.clone();
            ram_other[at..at + 4].copy_from_slice(&value.to_le_bytes());
            assert!(go(&other, &s, ram_other, 0x3000_0000).is_err(), "{at:#x}");
        }
        // The plain fixture has no such stage at all.
        let (plain, plain_setup, plain_ram) = fixture();
        assert!(go(&plain, &plain_setup, plain_ram, 0x3000_0000).is_err());
    }

    #[test]
    fn the_copy_stub_loader_asset_keeps_its_three_binding_templates_and_no_other_build_has_the_stub_one() {
        use copy_stub_template as t;
        assert_eq!((t::SETUP_SD_REDIRECT % 8, t::SETUP_SD_READ_ONLY % 8, t::SETUP_SD_COPY_STUB % 8), (0, 0, 0));
        let mut template = vec![0u8; 48 + 1024 + 32];
        template[..8].copy_from_slice(&1u64.to_le_bytes());
        assert_eq!(&t::IMAGE[t::SETUP_SD_COPY_STUB..t::SETUP_SD_COPY_STUB + template.len()], template);
        assert_eq!(COPY_STUB.copy_stub_binding, Some(t::SETUP_SD_COPY_STUB));
        assert_eq!(COPY_STUB.redirect_binding, Some(t::SETUP_SD_REDIRECT));
        assert_eq!(
            (CARD.copy_stub_binding, INTERNAL_FLASH.copy_stub_binding, READ_ONLY.copy_stub_binding, CHECKED_REDIRECT.copy_stub_binding),
            (None, None, None, None)
        );
        for other in [
            include_str!("../assets/sd-boot-v1-xp5-loader.generated.json"),
            include_str!("../assets/sd-boot-v1-xp5-internal-flash-loader.generated.json"),
            include_str!("../assets/sd-boot-v1-xp5-read-only-loader.generated.json"),
            include_str!("../assets/sd-boot-v1-xp5-checked-redirect-loader.generated.json"),
        ] {
            assert!(!other.contains("setup_sd_copy_stub"));
        }
        // The stub image is position independent, a multiple of 8 bytes, with its parameter block last.
        let stub = stub_template::IMAGE;
        assert_eq!((stub.len() % 8, stub_template::XP5_STUB_PARAMS + 176, stub_template::XP5_STUB_ENTRY), (0, stub.len(), 0));
        assert!(stub[stub_template::XP5_STUB_PARAMS..].iter().all(|b| *b == 0));
        // None of the stub's parameter offsets overlaps another.
        let mut offsets = vec![
            stub_template::XP5_STUB_P_SELF, stub_template::XP5_STUB_P_DST, stub_template::XP5_STUB_P_FLASH,
            stub_template::XP5_STUB_P_BYTES, stub_template::XP5_STUB_P_DRAM, stub_template::XP5_STUB_P_CRC,
            stub_template::XP5_STUB_P_COPY, stub_template::XP5_STUB_P_GREEN, stub_template::XP5_STUB_P_RED,
            stub_template::XP5_STUB_P_PIN, stub_template::XP5_STUB_P_PULSE, stub_template::XP5_STUB_P_GAP,
            stub_template::XP5_STUB_P_ENABLE, stub_template::XP5_STUB_P_ENABLE_GREEN,
            stub_template::XP5_STUB_P_ENABLE_RED, stub_template::XP5_STUB_P_MUX, stub_template::XP5_STUB_P_RETURN,
            stub_template::XP5_STUB_P_RESTORE,
            stub_template::XP5_STUB_OUT_STATUS, stub_template::XP5_STUB_OUT_SRC_CRC,
            stub_template::XP5_STUB_OUT_DST_CRC, stub_template::XP5_STUB_OUT_RESULT,
        ];
        offsets.sort_unstable();
        assert!(offsets.windows(2).all(|w| w[1] == w[0] + 8));
        assert_eq!(offsets[0], stub_template::XP5_STUB_PARAMS);
    }

    /// `redirect_fixture` with a section 0 that has a header, an entry point and zero padding before a vector
    /// table, and tables that declare its size.
    fn marker_fixture() -> (Vec<u8>, Setup, Vec<u8>) {
        let (fixture_data, mut s, _) = fixture();
        let (reset, mut update, ram_stage) = redirect::tests::marker_stage(0);
        let ram_size = 0x8_1000 + redirect::tests::E2E_SECTION_BYTES;
        update[..0x40000].copy_from_slice(&fixture_data[..0x40000]);
        let mut stock = reset.clone();
        put64(&mut stock, s.selector, 1).unwrap();
        update[0x40000..0x40000 + stock.len()].copy_from_slice(&stock);
        s.reset_copy_size = 0xb000;
        let mut ram = vec![0u8; ram_size];
        ram[..0xb000].copy_from_slice(&stock);
        put64(&mut ram, s.selector, 0).unwrap();
        ram[0x4_0000..0x4_0800].copy_from_slice(&update[0x64_0000..0x64_0800]);
        ram[0x8_1000..].copy_from_slice(&update[0xc_0000..0xc_0000 + redirect::tests::E2E_SECTION_BYTES]);
        assert_eq!(ram[0x4_0000..0x4_0800], ram_stage[0x4_0000..0x4_0800]);
        (update, s, ram)
    }

    #[test]
    fn the_marker_packages_the_copy_stubs_file_and_binds_the_names_the_stub_crc_and_every_proof() {
        let (data, s, ram) = marker_fixture();
        let at = 0xc000 + marker_loader_template::SETUP_SD_REDIRECT;
        let stub_at = 0xc000 + marker_loader_template::SETUP_SD_COPY_STUB;
        let marker_at = 0xc000 + marker_loader_template::SETUP_SD_MARKER;
        let stub = package_copy_stub(&data, &s, ram.clone(), 0, 0x3000_0000, CheckedRedirect::Apply).unwrap();
        for (mode, apply) in [(CheckedRedirect::Apply, 1u64), (CheckedRedirect::VerifyOnly, 0)] {
            let files = package_marker(&data, &s, ram.clone(), 0, 0x3000_0000, mode).unwrap();
            assert_eq!(files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["marble_init.bin", "ffboot.bin"]);
            // The card's file is the stock-derived cut image of the copy stub, byte for byte: the loader makes the change.
            assert_eq!(files[1].bytes, stub[1].bytes);
            assert_eq!(files[1].bytes[0x8_1000..0x8_3000], data[0xc_0000..0xc_2000]);
            let marble = &files[0].bytes;
            let q = |at: usize, n: usize| u64::from_le_bytes(marble[at + 8 * n..at + 8 * n + 8].try_into().unwrap());
            let d = |at: usize| u32::from_le_bytes(marble[at..at + 4].try_into().unwrap());
            // The redirect binding is the copy stub's (the copy call, the flash range's CRC-32: the STOCK section's).
            let section = &data[0xc_0000..0xc_2000];
            assert_eq!((q(at, 0), q(at, 1), q(at, 2)), (1, apply, 0x2d04));
            assert_eq!((q(at, 7), q(at, 8), q(at, 9), q(at, 10)), (0x8_1000, 0x2000, 0xf00c_0000, u64::from(crate::xp4::crc32(section))));
            // The marker binding: the modified CRC-32, the declared size, the names.
            let modified_crc = q(marker_at, 1);
            assert_ne!(modified_crc, q(at, 10));
            assert_eq!((q(marker_at, 0), q(marker_at, 2), q(marker_at, 3)), (1, 0x1800, 65.min(q(marker_at, 3))));
            let count = q(marker_at, 3) as usize;
            assert!(count > 1 && count <= 96);
            let mut changed = section.to_vec();
            for n in 0..count {
                let w = marker_at + 40 + 12 * n;
                let (offset, old, new) = (d(w) as usize, d(w + 4), d(w + 8));
                assert_eq!(u32::from_le_bytes(section[offset..offset + 4].try_into().unwrap()), old);
                changed[offset..offset + 4].copy_from_slice(&new.to_le_bytes());
            }
            assert_eq!(u64::from(crate::xp4::crc32(&changed)), modified_crc);
            // Past the last name the binding is zero, and so are its outputs.
            assert!(marble[marker_at + 40 + 12 * count..marker_at + 40 + 12 * 96 + 32].iter().all(|b| *b == 0));
            // The stub checks the MODIFIED section: its bound CRC-32 is the marker's, and the stub bytes carry it.
            let len = stub_template::IMAGE.len();
            let image = &marble[stub_at + 48..stub_at + 48 + len];
            assert_eq!(u64::from_le_bytes(image[stub_template::XP5_STUB_P_CRC..][..8].try_into().unwrap()), modified_crc);
            assert_eq!(q(stub_at, 4), u64::from(crate::xp4::crc32(image)));
        }
        // The two modes differ in the apply word alone.
        let apply = package_marker(&data, &s, ram.clone(), 0, 0x3000_0000, CheckedRedirect::Apply).unwrap();
        let verify = package_marker(&data, &s, ram.clone(), 0, 0x3000_0000, CheckedRedirect::VerifyOnly).unwrap();
        let differing: Vec<usize> =
            (0..apply[0].bytes.len()).filter(|&n| apply[0].bytes[n] != verify[0].bytes[n]).collect();
        assert_eq!(differing, [at + 8]);
        // The marker loader is a build of its own.
        assert_ne!(apply[0].bytes, stub[0].bytes);
        // The stub's own CRC-32 differs from the copy stub's card: only the bound CRC-32 of the stub's block changed.
        assert_ne!(apply[0].bytes[stub_at + 48..stub_at + 48 + stub_template::IMAGE.len()], stub[0].bytes[0xc000 + copy_stub_template::SETUP_SD_COPY_STUB + 48..0xc000 + copy_stub_template::SETUP_SD_COPY_STUB + 48 + stub_template::IMAGE.len()]);
    }

    /// `redirect_fixture` for the OS section: a stage with the OS load function and its three callers, tables that
    /// name the section compressed, and the update's compressed bytes.
    fn os_fixture() -> (Vec<u8>, Setup, Vec<u8>) {
        let (fixture_data, mut s, _) = fixture();
        let (reset, mut update, ram_stage) = redirect::tests::os_stage(0);
        update[..0x40000].copy_from_slice(&fixture_data[..0x40000]);
        let mut stock = reset.clone();
        put64(&mut stock, s.selector, 1).unwrap();
        update[0x40000..0x40000 + stock.len()].copy_from_slice(&stock);
        s.reset_copy_size = 0x1_4000;
        let mut ram = ram_stage.clone();
        ram[..0x1_4000].copy_from_slice(&stock);
        put64(&mut ram, s.selector, 0).unwrap();
        (update, s, ram)
    }

    #[test]
    fn the_os_startup_packages_the_decoded_section_alone_and_binds_the_crcs_the_stub_and_the_call() {
        let (data, s, ram) = os_fixture();
        let at = 0xc000 + os_loader_template::SETUP_SD_REDIRECT;
        let os_at = 0xc000 + os_loader_template::SETUP_SD_OS;
        let stub_at = 0xc000 + os_loader_template::SETUP_SD_COPY_STUB;
        let head = ram[redirect::tests::OS_DST as usize..].to_vec();
        for (mode, apply) in [(CheckedRedirect::Apply, 1u64), (CheckedRedirect::VerifyOnly, 0)] {
            let files = package_os(&data, &s, ram.clone(), 0, 0x3000_0000, mode).unwrap();
            assert_eq!(files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["marble_init.bin", "ffboot.bin"]);
            // The card's file is the decoded section's initialized bytes and nothing else.
            assert_eq!(files[1].bytes, head);
            let marble = &files[0].bytes;
            let q = |at: usize, n: usize| u64::from_le_bytes(marble[at + 8 * n..at + 8 * n + 8].try_into().unwrap());
            assert_eq!((q(at, 0), q(at, 1), q(at, 2)), (1, apply, 0x28ac));
            assert_eq!((q(at, 7), q(at, 8), q(at, 10)), (0, head.len() as u64, u64::from(crate::xp4::crc32(&head))));
            assert_eq!(q(at, 9), 0xf000_0000 + 0x20_0000);
            // The OS words: the compressed range's length and CRC-32, the id, the destination, the copy function.
            let comp = &data[0x20_0000..0x20_0000 + q(os_at, 1) as usize];
            assert_eq!((q(os_at, 0), q(os_at, 2), q(os_at, 3), q(os_at, 4), q(os_at, 6)), (1, u64::from(crate::xp4::crc32(comp)), 6, redirect::tests::OS_DST, 0x2_0000));
            assert!(marble[os_at + 64..os_at + 96].iter().all(|b| *b == 0));
            // The stub's size and place, and the request's length and CRC-32 are the file's.
            assert_eq!(q(stub_at, 1), 0x200);
            let request = 0xc000 + os_loader_template::SETUP_SD_REQUEST;
            assert_eq!((q(request, 1), q(request, 2)), (head.len() as u64, u64::from(crate::xp4::crc32(&head))));
        }
        let apply = package_os(&data, &s, ram.clone(), 0, 0x3000_0000, CheckedRedirect::Apply).unwrap();
        let verify = package_os(&data, &s, ram.clone(), 0, 0x3000_0000, CheckedRedirect::VerifyOnly).unwrap();
        assert_eq!(apply[1].bytes, verify[1].bytes);
        // Apply and verify-only differ in the apply word alone.
        let differing: Vec<usize> = (0..apply[0].bytes.len()).filter(|&n| apply[0].bytes[n] != verify[0].bytes[n]).collect();
        assert_eq!(differing, [at + 8]);
    }

    #[test]
    fn the_os_startup_refuses_what_it_cannot_prove_or_place() {
        let (data, s, ram) = os_fixture();
        let go = |data: &[u8], s: &Setup, ram: Vec<u8>, destination: u64| {
            package_os(data, s, ram, 0, destination, CheckedRedirect::Apply)
        };
        assert!(go(&data, &s, ram.clone(), 0x3000_0000).is_ok());
        assert!(go(&data, &s, ram.clone(), 0x3000_0010).is_err());
        assert!(go(&data, &s, ram.clone(), 0x1000_0000).is_err());
        assert!(go(&data, &s, ram.clone(), READ_ONLY_CEILING).is_err());
        // The card's OS section is not the decode of the flash's compressed bytes.
        let mut changed = ram.clone();
        changed[redirect::tests::OS_DST as usize + 3] ^= 1;
        assert!(go(&data, &s, changed, 0x3000_0000).is_err());
        // The stage's OS call is not where the load function's callers are.
        let mut changed = data.clone();
        changed[0x40000 + 0x28a8..0x40000 + 0x28ac].copy_from_slice(&0x5280_0020u32.to_le_bytes());
        assert!(go(&changed, &s, ram.clone(), 0x3000_0000).is_err());
    }

    /// `os_fixture` for the Exif string: the OS stage with the string and its two sites in the decode.
    fn exif_fixture() -> (Vec<u8>, Setup, Vec<u8>) {
        let (fixture_data, mut s, _) = fixture();
        let (reset, mut update, ram_stage) = redirect::tests::exif_stage(0);
        update[..0x40000].copy_from_slice(&fixture_data[..0x40000]);
        let mut stock = reset.clone();
        put64(&mut stock, s.selector, 1).unwrap();
        update[0x40000..0x40000 + stock.len()].copy_from_slice(&stock);
        s.reset_copy_size = 0x1_4000;
        let mut ram = ram_stage.clone();
        ram[..0x1_4000].copy_from_slice(&stock);
        put64(&mut ram, s.selector, 0).unwrap();
        (update, s, ram)
    }

    #[test]
    fn the_exif_startup_packages_a_placeholder_file_and_binds_the_crcs_the_table_the_stub_and_the_call() {
        let (data, s, ram) = exif_fixture();
        let at = 0xc000 + exif_loader_template::SETUP_SD_REDIRECT;
        let exif_at = 0xc000 + exif_loader_template::SETUP_SD_EXIF;
        let stub_at = 0xc000 + exif_loader_template::SETUP_SD_COPY_STUB;
        for (mode, apply) in [(CheckedRedirect::Apply, 1u64), (CheckedRedirect::VerifyOnly, 0)] {
            let files = package_exif(&data, &s, ram.clone(), 0, 0x3000_0000, mode).unwrap();
            assert_eq!(files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["marble_init.bin", "ffboot.bin"]);
            // the card's file is a short note and zeros: 4 KiB and no OS bytes
            let file = &files[1].bytes;
            assert_eq!(file.len(), EXIF_PLACEHOLDER_BYTES);
            assert!(file.starts_with(b"XP5 startup placeholder"));
            let note = file.iter().position(|b| *b == b'\n').unwrap();
            assert!(file[note + 1..].iter().all(|b| *b == 0));
            let marble = &files[0].bytes;
            let q = |at: usize, n: usize| u64::from_le_bytes(marble[at + 8 * n..at + 8 * n + 8].try_into().unwrap());
            assert_eq!((q(at, 0), q(at, 1), q(at, 2)), (1, apply, 0x28ac));
            assert_eq!((q(at, 7), q(at, 8), q(at, 10)), (0, 4096, u64::from(crate::xp4::crc32(file))));
            assert_eq!(q(at, 9), 0xf000_0000 + 0x20_0000);
            // the Exif words: the compressed range's length and CRC-32, the table's address, the id, the section, the window, the count
            let comp = &data[0x20_0000..0x20_0000 + q(exif_at, 2) as usize];
            assert_eq!(
                (q(exif_at, 0), q(exif_at, 1), q(exif_at, 3), q(exif_at, 4), q(exif_at, 5), q(exif_at, 6), q(exif_at, 7)),
                (1, 0xf000_0000, u64::from(crate::xp4::crc32(comp)), 0x4_0000, 6, redirect::tests::OS_DST, redirect::tests::OS_HEAD as u64)
            );
            let window = redirect::tests::OS_DST + redirect::tests::EXIF_AT as u64;
            assert_eq!((q(exif_at, 8), q(exif_at, 9), q(exif_at, 10)), (window, 16, 8));
            assert_eq!(q(exif_at, 11), u64::from(crate::xp4::crc32(b"Digital Camera \0")));
            assert_eq!(q(exif_at, 12), u64::from(crate::xp4::crc32(b"FUJIMOD Camera \0")));
            assert_eq!(q(exif_at, 13), window - 8);
            assert!(marble[exif_at + 8 * 29..exif_at + 296].iter().all(|b| *b == 0));
            assert_eq!(q(stub_at, 1), 0x200);
            let request = 0xc000 + exif_loader_template::SETUP_SD_REQUEST;
            assert_eq!((q(request, 1), q(request, 2)), (4096, u64::from(crate::xp4::crc32(file))));
        }
        let apply = package_exif(&data, &s, ram.clone(), 0, 0x3000_0000, CheckedRedirect::Apply).unwrap();
        let verify = package_exif(&data, &s, ram.clone(), 0, 0x3000_0000, CheckedRedirect::VerifyOnly).unwrap();
        assert_eq!(apply[1].bytes, verify[1].bytes);
        let differing: Vec<usize> = (0..apply[0].bytes.len()).filter(|&n| apply[0].bytes[n] != verify[0].bytes[n]).collect();
        assert_eq!(differing, [at + 8]);
        // the Exif loader is a build of its own
        let os = package_os(&data, &s, ram.clone(), 0, 0x3000_0000, CheckedRedirect::Apply);
        assert!(os.is_err() || os.unwrap()[0].bytes != apply[0].bytes);
    }

    #[test]
    fn the_exif_startup_refuses_what_it_cannot_prove_or_place() {
        let (data, s, ram) = exif_fixture();
        let go = |data: &[u8], s: &Setup, ram: Vec<u8>, destination: u64| {
            package_exif(data, s, ram, 0, destination, CheckedRedirect::Apply)
        };
        assert!(go(&data, &s, ram.clone(), 0x3000_0000).is_ok());
        assert!(go(&data, &s, ram.clone(), 0x3000_0010).is_err());
        assert!(go(&data, &s, ram.clone(), 0x1000_0000).is_err());
        assert!(go(&data, &s, ram.clone(), READ_ONLY_CEILING).is_err());
        // the card's OS section is not the decode of the flash's compressed bytes
        let mut changed = ram.clone();
        changed[redirect::tests::OS_DST as usize + 3] ^= 1;
        assert!(go(&data, &s, changed, 0x3000_0000).is_err());
        // the stage's OS call is not where the load function's callers are
        let mut changed = data.clone();
        changed[0x40000 + 0x28a8..0x40000 + 0x28ac].copy_from_slice(&0x5280_0020u32.to_le_bytes());
        assert!(go(&changed, &s, ram.clone(), 0x3000_0000).is_err());
        // a decode without the string (the plain OS fixture) cannot be named
        let (plain, plain_setup, plain_ram) = os_fixture();
        assert!(go(&plain, &plain_setup, plain_ram, 0x3000_0000).is_err());
    }

    #[test]
    fn the_exif_loader_asset_keeps_its_four_binding_templates_and_no_other_build_has_the_exif_one() {
        use exif_loader_template as t;
        assert_eq!((t::SETUP_SD_REDIRECT % 8, t::SETUP_SD_READ_ONLY % 8, t::SETUP_SD_COPY_STUB % 8, t::SETUP_SD_EXIF % 8), (0, 0, 0, 0));
        let mut template = vec![0u8; redirect::EXIF_BINDING];
        template[..8].copy_from_slice(&1u64.to_le_bytes());
        assert_eq!(&t::IMAGE[t::SETUP_SD_EXIF..t::SETUP_SD_EXIF + template.len()], template);
        assert_eq!(EXIF.exif_binding, Some(t::SETUP_SD_EXIF));
        assert_eq!(
            (CARD.exif_binding, INTERNAL_FLASH.exif_binding, READ_ONLY.exif_binding, CHECKED_REDIRECT.exif_binding, COPY_STUB.exif_binding, MARKER.exif_binding, OS.exif_binding),
            (None, None, None, None, None, None, None)
        );
        for other in [
            include_str!("../assets/sd-boot-v1-xp5-loader.generated.json"),
            include_str!("../assets/sd-boot-v1-xp5-internal-flash-loader.generated.json"),
            include_str!("../assets/sd-boot-v1-xp5-read-only-loader.generated.json"),
            include_str!("../assets/sd-boot-v1-xp5-checked-redirect-loader.generated.json"),
            include_str!("../assets/sd-boot-v1-xp5-copy-stub-loader.generated.json"),
            include_str!("../assets/sd-boot-v1-xp5-marker-loader.generated.json"),
            include_str!("../assets/sd-boot-v1-xp5-os-loader.generated.json"),
        ] {
            assert!(!other.contains("setup_sd_exif"));
        }
        assert_ne!(t::IMAGE, os_loader_template::IMAGE);
        assert_eq!(t::XP5_SETUP_RETURN_BRANCH, os_loader_template::XP5_SETUP_RETURN_BRANCH);
        // the stub fits the copy stub's binding array
        assert!(exif_stub_template::IMAGE.len() <= 1024 && exif_stub_template::XP5_EXIF_PARAMS + redirect::EXIF_STUB_PARAMS == exif_stub_template::IMAGE.len());
    }

    /// `os_fixture` for the hook: the OS stage with the hooked function, its calls, the code map and the padding.
    fn hook_fixture() -> (Vec<u8>, Setup, Vec<u8>) {
        let (fixture_data, mut s, _) = fixture();
        let (reset, mut update, ram_stage) = redirect::tests::hook_stage(0);
        update[..0x40000].copy_from_slice(&fixture_data[..0x40000]);
        let mut stock = reset.clone();
        put64(&mut stock, s.selector, 1).unwrap();
        update[0x40000..0x40000 + stock.len()].copy_from_slice(&stock);
        s.reset_copy_size = 0x1_4000;
        let mut ram = ram_stage.clone();
        ram[..0x1_4000].copy_from_slice(&stock);
        put64(&mut ram, s.selector, 0).unwrap();
        (update, s, ram)
    }

    #[test]
    fn the_hook_startup_packages_a_placeholder_file_and_binds_the_crcs_the_table_the_stub_and_the_call() {
        let (data, s, ram) = hook_fixture();
        let at = 0xc000 + hook_loader_template::SETUP_SD_REDIRECT;
        let hook_at = 0xc000 + hook_loader_template::SETUP_SD_HOOK;
        let stub_at = 0xc000 + hook_loader_template::SETUP_SD_COPY_STUB;
        for (mode, apply) in [(CheckedRedirect::Apply, 1u64), (CheckedRedirect::VerifyOnly, 0)] {
            let files = package_hook(&data, &s, ram.clone(), 0, 0x3000_0000, mode).unwrap();
            assert_eq!(files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["marble_init.bin", "ffboot.bin"]);
            let file = &files[1].bytes;
            assert_eq!(file.len(), EXIF_PLACEHOLDER_BYTES);
            assert!(file.starts_with(b"XP5 startup placeholder"));
            let marble = &files[0].bytes;
            let q = |at: usize, n: usize| u64::from_le_bytes(marble[at + 8 * n..at + 8 * n + 8].try_into().unwrap());
            assert_eq!((q(at, 0), q(at, 1), q(at, 2)), (1, apply, 0x28ac));
            assert_eq!((q(at, 7), q(at, 8), q(at, 10)), (0, 4096, u64::from(crate::xp4::crc32(file))));
            assert_eq!(q(at, 9), 0xf000_0000 + 0x20_0000);
            let comp = &data[0x20_0000..0x20_0000 + q(hook_at, 2) as usize];
            assert_eq!(
                (q(hook_at, 0), q(hook_at, 1), q(hook_at, 3), q(hook_at, 4), q(hook_at, 5), q(hook_at, 6), q(hook_at, 7)),
                (1, 0xf000_0000, u64::from(crate::xp4::crc32(comp)), 0x4_0000, 6, redirect::tests::OS_DST, redirect::tests::OS_HEAD as u64)
            );
            let hook = redirect::tests::OS_DST + redirect::tests::HOOK_AT as u64;
            // the hook is the function, the program starts at the next 8-aligned word of the padding after its first word
            assert_eq!(q(hook_at, 8), hook);
            let cave = q(hook_at, 9);
            assert_eq!((cave % 8, cave > hook, q(hook_at, 10)), (0, true, 13));
            assert_eq!(q(hook_at, 13), hook);
            assert_eq!(q(hook_at, 14), u64::from(0xa9bf_7bfdu32) | (u64::from(0x1400_0000u32 | (((cave + 16 - hook) >> 2) as u32 & 0x03ff_ffff)) << 32));
            assert!(marble[hook_at + 8 * 13 + 16 * 13..hook_at + 424].iter().all(|b| *b == 0));
            assert_eq!(q(stub_at, 1), 0x200);
            // the stub is larger than the older builds' and the binding array is too
            assert_eq!(q(stub_at, 2) as usize, hook_stub_template::IMAGE.len());
            let request = 0xc000 + hook_loader_template::SETUP_SD_REQUEST;
            assert_eq!((q(request, 1), q(request, 2)), (4096, u64::from(crate::xp4::crc32(file))));
        }
        let apply = package_hook(&data, &s, ram.clone(), 0, 0x3000_0000, CheckedRedirect::Apply).unwrap();
        let verify = package_hook(&data, &s, ram.clone(), 0, 0x3000_0000, CheckedRedirect::VerifyOnly).unwrap();
        assert_eq!(apply[1].bytes, verify[1].bytes);
        let differing: Vec<usize> = (0..apply[0].bytes.len()).filter(|&n| apply[0].bytes[n] != verify[0].bytes[n]).collect();
        assert_eq!(differing, [at + 8]);
        // the hook loader is a build of its own
        let os = package_os(&data, &s, ram.clone(), 0, 0x3000_0000, CheckedRedirect::Apply);
        assert!(os.is_err() || os.unwrap()[0].bytes != apply[0].bytes);
    }

    #[test]
    fn the_hook_startup_refuses_what_it_cannot_prove_or_place() {
        let (data, s, ram) = hook_fixture();
        let go = |data: &[u8], s: &Setup, ram: Vec<u8>, destination: u64| {
            package_hook(data, s, ram, 0, destination, CheckedRedirect::Apply)
        };
        assert!(go(&data, &s, ram.clone(), 0x3000_0000).is_ok());
        assert!(go(&data, &s, ram.clone(), 0x3000_0010).is_err());
        assert!(go(&data, &s, ram.clone(), 0x1000_0000).is_err());
        assert!(go(&data, &s, ram.clone(), READ_ONLY_CEILING).is_err());
        // the stage's OS call is not where the load function's callers are
        let mut changed = data.clone();
        changed[0x40000 + 0x28a8..0x40000 + 0x28ac].copy_from_slice(&0x5280_0020u32.to_le_bytes());
        assert!(go(&changed, &s, ram.clone(), 0x3000_0000).is_err());
        // a decode without the hooked function (the plain OS fixture, or the Exif one) cannot be hooked
        let (plain, plain_setup, plain_ram) = os_fixture();
        assert!(go(&plain, &plain_setup, plain_ram, 0x3000_0000).is_err());
        let (exif, exif_setup, exif_ram) = exif_fixture();
        assert!(go(&exif, &exif_setup, exif_ram, 0x3000_0000).is_err());
        // and a decode whose padding was used up
        let mut spent = ram.clone();
        let pad = redirect::tests::OS_DST as usize + 0xc00 + 0x20;
        spent[pad..pad + 4].copy_from_slice(&0x1234_5678u32.to_le_bytes());
        assert!(go(&data, &s, spent, 0x3000_0000).is_err());
    }

    #[test]
    fn the_hook_loader_asset_keeps_its_four_binding_templates_a_larger_stub_room_and_no_other_build_has_the_hook_one() {
        use hook_loader_template as t;
        assert_eq!((t::SETUP_SD_REDIRECT % 8, t::SETUP_SD_READ_ONLY % 8, t::SETUP_SD_COPY_STUB % 8, t::SETUP_SD_HOOK % 8), (0, 0, 0, 0));
        let mut template = vec![0u8; redirect::HOOK_BINDING];
        template[..8].copy_from_slice(&1u64.to_le_bytes());
        assert_eq!(&t::IMAGE[t::SETUP_SD_HOOK..t::SETUP_SD_HOOK + template.len()], template);
        assert_eq!(HOOK.hook_binding, Some(t::SETUP_SD_HOOK));
        assert_eq!(
            (CARD.hook_binding, INTERNAL_FLASH.hook_binding, READ_ONLY.hook_binding, CHECKED_REDIRECT.hook_binding, COPY_STUB.hook_binding, MARKER.hook_binding, OS.hook_binding, EXIF.hook_binding),
            (None, None, None, None, None, None, None, None)
        );
        for other in [
            include_str!("../assets/sd-boot-v1-xp5-loader.generated.json"),
            include_str!("../assets/sd-boot-v1-xp5-internal-flash-loader.generated.json"),
            include_str!("../assets/sd-boot-v1-xp5-read-only-loader.generated.json"),
            include_str!("../assets/sd-boot-v1-xp5-checked-redirect-loader.generated.json"),
            include_str!("../assets/sd-boot-v1-xp5-copy-stub-loader.generated.json"),
            include_str!("../assets/sd-boot-v1-xp5-marker-loader.generated.json"),
            include_str!("../assets/sd-boot-v1-xp5-os-loader.generated.json"),
            include_str!("../assets/sd-boot-v1-xp5-exif-loader.generated.json"),
        ] {
            assert!(!other.contains("setup_sd_hook"));
        }
        assert_ne!(t::IMAGE, exif_loader_template::IMAGE);
        assert_eq!(t::XP5_SETUP_RETURN_BRANCH, exif_loader_template::XP5_SETUP_RETURN_BRANCH);
        // the stub's room is the hook build's own (1280); the older stub arrays are 1024, and the hook stub does not fit them
        assert!(hook_stub_template::IMAGE.len() <= redirect::HOOK_STUB_MAX && hook_stub_template::IMAGE.len() > redirect::STUB_MAX);
        assert_eq!(hook_stub_template::XP5_HOOK_PARAMS + redirect::HOOK_STUB_PARAMS, hook_stub_template::IMAGE.len());
    }

    #[test]
    fn the_marker_refuses_what_it_cannot_prove_or_place() {
        let (data, s, ram) = marker_fixture();
        let go = |data: &[u8], s: &Setup, ram: Vec<u8>, destination: u64| {
            package_marker(data, s, ram, 0, destination, CheckedRedirect::Apply)
        };
        assert!(go(&data, &s, ram.clone(), 0x3000_0000).is_ok());
        for bad in [0x2000_0000, 0x3000_0004, READ_ONLY_CEILING, 0x2445_9000 - 0x1000] {
            assert!(go(&data, &s, ram.clone(), bad).is_err(), "{bad:#x}");
        }
        // The card's section differs from the flash's: the file has to be the stock-derived one (the loader changes it).
        let mut bad = ram.clone();
        bad[0x8_1000 + 0x40] ^= 1;
        assert!(go(&data, &s, bad, 0x3000_0000).is_err());
        // The entry instruction is not the one the program displaces (flash and card agree).
        let mut other = data.clone();
        other[0xc_0000 + 0x40] ^= 0x20;
        let mut ram_other = ram.clone();
        ram_other[0x8_1000 + 0x40] ^= 0x20;
        assert!(go(&other, &s, ram_other, 0x3000_0000).is_err());
        // Tables that declare no size (the copy stub's fixture) have no section to change.
        let (plain, plain_setup, plain_ram) = redirect_fixture();
        assert!(go(&plain, &plain_setup, plain_ram, 0x3000_0000).is_err());
        let (plain, plain_setup, plain_ram) = fixture();
        assert!(go(&plain, &plain_setup, plain_ram, 0x3000_0000).is_err());
    }

    #[test]
    fn the_marker_loader_asset_keeps_its_four_binding_templates_and_no_other_build_has_the_marker_one() {
        use marker_loader_template as t;
        assert_eq!(
            (t::SETUP_SD_REDIRECT % 8, t::SETUP_SD_READ_ONLY % 8, t::SETUP_SD_COPY_STUB % 8, t::SETUP_SD_MARKER % 8),
            (0, 0, 0, 0)
        );
        let mut template = vec![0u8; redirect::MARKER_BINDING];
        template[..8].copy_from_slice(&1u64.to_le_bytes());
        assert_eq!(&t::IMAGE[t::SETUP_SD_MARKER..t::SETUP_SD_MARKER + template.len()], template);
        assert_eq!(MARKER.marker_binding, Some(t::SETUP_SD_MARKER));
        assert_eq!(MARKER.copy_stub_binding, Some(t::SETUP_SD_COPY_STUB));
        assert_eq!(MARKER.redirect_binding, Some(t::SETUP_SD_REDIRECT));
        assert_eq!(
            (CARD.marker_binding, INTERNAL_FLASH.marker_binding, READ_ONLY.marker_binding, CHECKED_REDIRECT.marker_binding, COPY_STUB.marker_binding),
            (None, None, None, None, None)
        );
        for other in [
            include_str!("../assets/sd-boot-v1-xp5-loader.generated.json"),
            include_str!("../assets/sd-boot-v1-xp5-internal-flash-loader.generated.json"),
            include_str!("../assets/sd-boot-v1-xp5-read-only-loader.generated.json"),
            include_str!("../assets/sd-boot-v1-xp5-checked-redirect-loader.generated.json"),
            include_str!("../assets/sd-boot-v1-xp5-copy-stub-loader.generated.json"),
        ] {
            assert!(!other.contains("setup_sd_marker"));
        }
        // The marker build is its own loader, and the stub it binds is the copy stub's asset, unchanged.
        assert_ne!(t::IMAGE, copy_stub_template::IMAGE);
        assert_eq!(t::XP5_SETUP_RETURN_BRANCH, copy_stub_template::XP5_SETUP_RETURN_BRANCH);
        assert_eq!(t::PRELUDE_STACK_MAX, copy_stub_template::PRELUDE_STACK_MAX);
    }

    #[test]
    fn the_read_only_startup_is_its_own_startup() {
        use crate::StartupOptions;
        let (data, s, _) = fixture();
        let update = fujimod_firmware::invert(&data);
        let mut recipe = crate::Recipe::bundled("x100vi-131").unwrap().unwrap();
        recipe.source_size = update.len();
        recipe.source_sha256 = fujimod_firmware::sha256(&update);
        recipe.updater = None;
        recipe.plan = Some(image::Plan {
            size: 0x1000,
            entry: 0,
            regions: vec![image::Region {
                name: "reset".into(),
                address: 0,
                source: Source {
                    dat_offset: 0x40000,
                    size: 0x1000,
                    compression: image::Compression::Raw,
                    decoded_size: 0x1000,
                    sha256: fujimod_firmware::sha256(&data[0x40000..0x41000]),
                    initialized_prefix: None,
                },
            }],
            bss: vec![],
            edits: vec![image::Edit {
                address: 0x40,
                old: 1,
                new: 0,
                what: "preloaded selector".into(),
            }],
        });
        recipe.xp5 = Some(s);
        let options = StartupOptions {
            read_only_destination: Some(0x3000_0000),
            ..StartupOptions::default()
        };
        let names = |files: &[File]| files.iter().map(|f| f.name.clone()).collect::<Vec<_>>();
        let everything = recipe.build_startup_with_options(&update, options).unwrap();
        assert_eq!(names(&everything), ["marble_init.bin", "ffboot.bin", "FPUPDATE.DAT"]);
        // `build_boot_files` is an allowlist: the update is dropped, nothing else is returned.
        let boot = recipe.build_boot_files(&update, options).unwrap();
        assert_eq!(names(&boot), ["marble_init.bin", "ffboot.bin"]);
        assert_eq!(boot[0].bytes, everything[0].bytes);
        assert_eq!(boot[1].bytes, everything[1].bytes);
        // It excludes the internal-flash build, the fallback and a storage image, takes only XP5
        // and refuses a destination the loader would refuse.
        for other in [
            StartupOptions { internal_flash: true, ..options },
            StartupOptions { fallback_wait_seconds: Some(1), ..options },
            StartupOptions { read_only_destination: Some(0x1000), ..options },
            StartupOptions { read_only_destination: Some(0x3000_0004), ..options },
            StartupOptions { read_only_destination: Some(READ_ONLY_CEILING), ..options },
        ] {
            assert!(recipe.build_boot_files(&update, other).is_err(), "{other:?}");
        }
        assert!(recipe.build_startup_with_storage(&update, &[0; 8], options).is_err());
        // Its RAM image is read and never run, so a RAM overlay is refused.
        let refused = recipe
            .build_startup_with_overlay(&update, options, &image::Overlay::default())
            .unwrap_err();
        assert!(refused.to_string().contains("takes no RAM overlay"), "{refused}");
        let mut other_generation = recipe.clone();
        other_generation.generation = crate::Generation::Xp4;
        assert!(other_generation.build_boot_files(&update, options).is_err());
        // The default options still build the card-resident startup, not the read-only one.
        assert_eq!(StartupOptions::default().read_only_destination, None);
    }
}
