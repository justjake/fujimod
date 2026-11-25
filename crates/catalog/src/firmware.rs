//! Firmware source identities, boot layouts and persistent-storage formats.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// An explicitly empty device-reference set.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "FirmwareEmpty"))]
pub struct Empty {}

/// Declared flash slices facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "FirmwareFlashSlices"))]
pub struct FlashSlices {
    /// Declared address.
    pub address: u64,
    /// Declared name.
    pub name: String,
    /// Declared offset.
    pub offset: u64,
    /// Declared sha256.
    pub sha256: String,
    /// Declared size.
    pub size: u64,
}

/// Declared units facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "FirmwareUnits"))]
pub struct Units {
    /// Declared unit.
    pub unit: String,
}

/// Declared boot layout facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "FirmwareBootLayout"))]
pub struct BootLayout {
    /// Declared flash slices.
    pub flash_slices: Vec<FlashSlices>,
    /// Declared units.
    pub units: Vec<Units>,
}

/// Declared regs facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "FirmwareRegs"))]
pub struct Regs {
    /// Declared x19.
    pub x19: u64,
    /// Declared x20.
    pub x20: u64,
    /// Declared x21.
    pub x21: u64,
}

/// Declared sysregs facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "FirmwareSysregs"))]
pub struct Sysregs {
    #[serde(rename = "CPACR_EL1")]
    /// Declared CPACR EL1.
    pub cpacr_el1: u64,
    #[serde(rename = "ICC_IGRPEN0_EL1")]
    /// Declared ICC IGRPEN0 EL1.
    pub icc_igrpen0_el1: u64,
    #[serde(rename = "ICC_IGRPEN1_EL1")]
    /// Declared ICC IGRPEN1 EL1.
    pub icc_igrpen1_el1: u64,
    #[serde(rename = "ICC_PMR_EL1")]
    /// Declared ICC PMR EL1.
    pub icc_pmr_el1: u64,
    #[serde(rename = "SCTLR_EL1")]
    /// Declared SCTLR EL1.
    pub sctlr_el1: u64,
    #[serde(rename = "SP_EL0")]
    /// Declared SP EL0.
    pub sp_el0: u64,
    #[serde(rename = "SP_EL1")]
    /// Declared SP EL1.
    pub sp_el1: u64,
    #[serde(rename = "VBAR_EL1")]
    /// Declared VBAR EL1.
    pub vbar_el1: u64,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "ELR_EL3")]
    /// Declared ELR EL3.
    pub elr_el3: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "SCR_EL3")]
    /// Declared SCR EL3.
    pub scr_el3: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "SP_EL3")]
    /// Declared SP EL3.
    pub sp_el3: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "SPSR_EL3")]
    /// Declared SPSR EL3.
    pub spsr_el3: Option<u64>,
}

/// Declared entry level.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", schemars(rename = "FirmwareEntryLevel"))]
pub enum EntryLevel {
    /// EL1 handoff entry.
    El1h,
    /// EL2 entry.
    El2h,
    /// EL3 entry.
    El3h,
    /// EL3 entry with the SPSel-selected stack pointer (SP_EL0).
    El3t,
}

/// Declared entry descriptor facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "FirmwareEntry"))]
pub struct Entry {
    /// Declared entry level.
    pub level: EntryLevel,
    /// Declared entry pstate.
    pub pstate: u64,
    /// Declared entry registers keyed by x0 through x30.
    pub regs: BTreeMap<String, u64>,
}

/// Declared cpu facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "FirmwareCpu"))]
pub struct Cpu {
    /// Declared cpsr for a legacy EL1 handoff.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpsr: Option<u64>,
    /// Declared id.
    pub id: u64,
    /// Declared pc.
    pub pc: u64,
    /// Declared legacy handoff registers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regs: Option<Regs>,
    /// Declared running.
    pub running: bool,
    /// Declared sysregs.
    pub sysregs: Sysregs,
    /// Declared explicit entry descriptor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<Entry>,
}

impl Cpu {
    /// Validate that the cpu declares exactly one entry shape with known registers.
    pub fn validate(&self) -> Result<(), String> {
        let legacy = self.cpsr.is_some() || self.regs.is_some();
        let entry = self.entry.is_some();
        match (legacy, entry) {
            (false, false) => {
                Err("cpu must declare a legacy handoff or an explicit entry descriptor".into())
            }
            (true, true) => Err(
                "cpu cannot declare both a legacy handoff and an explicit entry descriptor".into(),
            ),
            (true, false) => {
                if self.cpsr.is_none() || self.regs.is_none() {
                    return Err("a legacy handoff requires both cpsr and regs".into());
                }
                Ok(())
            }
            (false, true) => {
                let entry = self.entry.as_ref().expect("entry is present");
                let expected_mode = match &entry.level {
                    EntryLevel::El1h => 0x5,
                    EntryLevel::El2h => 0x9,
                    EntryLevel::El3h => 0xd,
                    EntryLevel::El3t => 0xc,
                };
                let mode = entry.pstate & 0x1f;
                if mode != expected_mode {
                    return Err(format!(
                        "entry pstate mode {mode:#x} does not match declared level {:?}",
                        entry.level
                    ));
                }
                for name in entry.regs.keys() {
                    if !is_entry_register_name(name) {
                        return Err(format!("entry register {name:?} is not x0 through x30"));
                    }
                }
                Ok(())
            }
        }
    }
}

/// Report whether a name is one of the entry registers x0 through x30.
fn is_entry_register_name(name: &str) -> bool {
    let Some(index) = name
        .strip_prefix('x')
        .and_then(|suffix| suffix.parse::<u8>().ok())
    else {
        return false;
    };
    name == format!("x{index}") && index <= 30
}

/// Declared descriptor facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "FirmwareDescriptor"))]
pub struct Descriptor {
    /// Declared address.
    pub address: u64,
    /// Declared sampling.
    pub sampling: String,
    /// Declared sha256.
    pub sha256: String,
    /// Declared size.
    pub size: u64,
}

/// Declared fe80 programs facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "FirmwareFe80Programs"))]
pub struct Fe80Programs {
    /// Declared basis.
    pub basis: String,
    /// Declared descriptor.
    pub descriptor: Descriptor,
    /// Declared period.
    pub period: u64,
    /// Declared program.
    pub program: Descriptor,
    /// Declared setup phase.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup_phase: Option<String>,
}

/// Declared fixture layouts facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "FirmwareFixtureLayouts"))]
pub struct FixtureLayouts {
    /// Declared status.
    pub status: String,
}

/// Declared flash image facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "FirmwareFlashImage"))]
pub struct FlashImage {
    /// Declared dat origin.
    pub dat_origin: u64,
    /// Declared xor.
    pub xor: u64,
}

/// Security states the firmware runs its GICv3 with (GICD_CTLR.DS).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", schemars(rename = "FirmwareGicSecurityStates"))]
pub enum GicSecurityStates {
    /// A single Security state (DS=1).
    One,
    /// Secure and Non-secure states (DS=0).
    Two,
}

/// Declared gic redistributor initial facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    feature = "schema",
    schemars(rename = "FirmwareGicRedistributorInitial")
)]
pub struct GicRedistributorInitial {
    /// Declared core.
    pub core: u64,
    /// Declared offset.
    pub offset: u64,
    /// Declared size.
    pub size: u64,
    /// Declared value.
    pub value: u64,
}

/// Declared persistent inputs geometry facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    feature = "schema",
    schemars(rename = "FirmwarePersistentInputsGeometry")
)]
pub struct PersistentInputsGeometry {
    /// Declared bytes.
    pub bytes: u64,
    /// Declared page bytes.
    pub page_bytes: u64,
    /// Declared pages per block.
    pub pages_per_block: u64,
    /// Declared spare bytes.
    pub spare_bytes: u64,
}

/// Declared persistent inputs facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "FirmwarePersistentInputs"))]
pub struct PersistentInputs {
    /// Declared assets.
    pub assets: BTreeMap<String, PersistentAssetLayout>,
    /// Declared geometry.
    pub geometry: PersistentInputsGeometry,
    /// Declared sets.
    pub sets: BTreeMap<String, fujimod_catalog_settings::persistent_inputs::PersistentSet>,
}

/// Declared preload facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "FirmwarePreload"))]
pub struct Preload {
    /// Declared address.
    pub address: u64,
    /// Declared dat offset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dat_offset: Option<u64>,
    /// Declared length.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<u64>,
    /// Declared sha256.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// Declared size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// Declared value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<u64>,
    /// Declared word size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub word_size: Option<u64>,
    /// Declared words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub words: Option<Vec<u64>>,
}

/// Declared sdio inputs facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "FirmwareSdioInputs"))]
pub struct SdioInputs {
    /// Declared firmware bytes.
    pub firmware_bytes: u64,
    /// Declared mac address.
    pub mac_address: Vec<u64>,
    /// Declared ssid.
    pub ssid: String,
}

/// Declared usb interrupt tables facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "FirmwareUsbInterruptTables"))]
pub struct UsbInterruptTables {
    /// Declared address.
    pub address: u64,
    /// Declared name.
    pub name: String,
    /// Declared sampling.
    pub sampling: String,
    /// Declared sha256.
    pub sha256: String,
    /// Declared size.
    pub size: u64,
}

/// Declared firmware profile facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct FirmwareProfile {
    /// Firmware-qualified secondary-core release mechanism.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_release: Option<crate::profiles::ResetRelease>,
    /// Declared boot layout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot_layout: Option<BootLayout>,
    /// Declared browser configuration id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser_configuration_id: Option<String>,
    /// Declared control bus tables id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_bus_tables_id: Option<String>,
    /// Declared cpu.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu: Option<Vec<Cpu>>,
    /// Declared execution scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_scope: Option<String>,
    /// Declared fe80 programs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fe80_programs: Option<Vec<Fe80Programs>>,
    /// Declared fe80 setup views.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fe80_setup_views: Option<Vec<Descriptor>>,
    /// Declared fixture layouts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixture_layouts: Option<FixtureLayouts>,
    /// Declared flash image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flash_image: Option<FlashImage>,
    /// Declared gic initial.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gic_initial: Option<BTreeMap<String, u64>>,
    /// Declared gic redistributor initial.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gic_redistributor_initial: Option<Vec<GicRedistributorInitial>>,
    /// GICv3 Security states; one when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gic_security_states: Option<GicSecurityStates>,
    /// Declared hdmi configuration id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hdmi_configuration_id: Option<String>,
    /// Declared limits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<Vec<String>>,
    /// Declared persistent inputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persistent_inputs: Option<PersistentInputs>,
    /// Declared preload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preload: Option<Vec<Preload>>,
    /// Declared rom basis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rom_basis: Option<String>,
    /// Declared sdio inputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sdio_inputs: Option<SdioInputs>,
    /// Declared storage layout id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_layout_id: Option<String>,
    /// Declared usb interrupt tables.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usb_interrupt_tables: Option<Vec<UsbInterruptTables>>,
    /// Declared viewer abi.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewer_abi: Option<String>,
}

/// Declared final version bytes facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "FirmwareFinalVersionBytes"))]
pub struct FinalVersionBytes {
    /// Declared offset.
    pub offset: u64,
    /// Declared value.
    pub value: u64,
}

/// Declared configurationdefaults output facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    feature = "schema",
    schemars(rename = "FirmwareConfigurationdefaultsOutput")
)]
pub struct ConfigurationdefaultsOutput {
    /// Declared address.
    pub address: u64,
    /// Declared size.
    pub size: u64,
}

/// Declared table facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "FirmwareTable"))]
pub struct Table {
    /// Declared address.
    pub address: u64,
    /// Declared count.
    pub count: u64,
    /// Declared name.
    pub name: String,
    /// Declared sampling.
    pub sampling: String,
    /// Declared sha256.
    pub sha256: String,
    /// Declared size.
    pub size: u64,
}

/// Declared configuration defaults facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "FirmwareConfigurationDefaults"))]
pub struct ConfigurationDefaults {
    /// Declared builder entry.
    pub builder_entry: u64,
    /// Declared code checks.
    pub code_checks: Vec<UsbInterruptTables>,
    /// Declared final version bytes.
    pub final_version_bytes: Vec<FinalVersionBytes>,
    /// Declared limits.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub limits: Vec<String>,
    /// Declared max instructions.
    pub max_instructions: u64,
    /// Declared output.
    pub output: ConfigurationdefaultsOutput,
    /// Declared pointer address.
    pub pointer_address: u64,
    /// Declared sections.
    pub sections: Vec<String>,
    /// Declared static oracle sha256.
    pub static_oracle_sha256: String,
    /// Declared store limit.
    pub store_limit: u64,
    /// Declared table.
    pub table: Table,
}

/// Declared descriptors facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "FirmwareDescriptors"))]
pub struct Descriptors {
    /// Declared address.
    pub address: u64,
    /// Declared count.
    pub count: u64,
    /// Declared format.
    pub format: String,
    /// Declared sampling.
    pub sampling: String,
    /// Declared sha256.
    pub sha256: String,
    /// Declared size.
    pub size: u64,
}

/// Declared control bus tables facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "FirmwareControlBusTables"))]
pub struct ControlBusTables {
    /// Declared descriptors.
    pub descriptors: Descriptors,
    /// Declared limits.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub limits: Vec<String>,
    /// Declared polarity words.
    pub polarity_words: Descriptor,
}

/// Declared remapping facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "FirmwareRemapping"))]
pub struct Remapping {
    /// Declared delta.
    pub delta: u64,
    /// Declared selector.
    pub selector: u64,
    /// Declared size.
    pub size: u64,
    /// Declared start.
    pub start: u64,
}

/// Declared synthetic byte fields facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "FirmwareSyntheticByteFields"))]
pub struct SyntheticByteFields {
    /// Declared basis.
    pub basis: String,
    /// Declared field.
    pub field: u64,
    /// Declared row.
    pub row: u64,
    /// Declared view.
    pub view: String,
}

/// Declared synthetic scalar bytes facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "FirmwareSyntheticScalarBytes"))]
pub struct SyntheticScalarBytes {
    /// Declared basis.
    pub basis: String,
    /// Declared field.
    pub field: u64,
    /// Declared source offset.
    pub source_offset: u64,
    /// Declared view.
    pub view: String,
}

/// Declared synthetic signature facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "FirmwareSyntheticSignature"))]
pub struct SyntheticSignature {
    /// Declared offset.
    pub offset: u64,
}

/// Declared cfgdata facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "FirmwareCfgdata"))]
pub struct Cfgdata {
    /// Declared address.
    pub address: u64,
    /// Declared defaults sha256.
    pub defaults_sha256: String,
    /// Declared pointer address.
    pub pointer_address: u64,
    /// Declared remapping.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remapping: Vec<Remapping>,
    /// Declared selector address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector_address: Option<u64>,
    /// Declared size.
    pub size: u64,
    /// Declared synthetic byte fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub synthetic_byte_fields: Option<Vec<SyntheticByteFields>>,
    /// Declared synthetic scalar bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub synthetic_scalar_bytes: Option<Vec<SyntheticScalarBytes>>,
    /// Declared synthetic signature.
    pub synthetic_signature: SyntheticSignature,
}

/// Declared configuration banks facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "FirmwareConfigurationBanks"))]
pub struct ConfigurationBanks {
    /// Declared cfg offset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cfg_offset: Option<u64>,
    /// Declared fill.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<u64>,
    /// Declared row.
    pub row: u64,
}

/// Declared custom defaults facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "FirmwareCustomDefaults"))]
pub struct CustomDefaults {
    /// Declared entry view.
    pub entry_view: String,
    /// Declared output address.
    pub output_address: u64,
    /// Declared size.
    pub size: u64,
    /// Declared slot bytes.
    pub slot_bytes: u64,
    /// Declared slots.
    pub slots: u64,
}

/// Declared storagelayout geometry facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "FirmwareStoragelayoutGeometry"))]
pub struct StoragelayoutGeometry {
    /// Declared blocks.
    pub blocks: u64,
    /// Declared page bytes.
    pub page_bytes: u64,
    /// Declared pages per block.
    pub pages_per_block: u64,
    /// Declared spare bytes.
    pub spare_bytes: u64,
}

/// Declared header facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "FirmwareHeader"))]
pub struct Header {
    /// Declared checksum offset.
    pub checksum_offset: u64,
    /// Declared registers view.
    pub registers_view: String,
    /// Declared view.
    pub view: String,
}

/// Declared default copies facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "FirmwareDefaultCopies"))]
pub struct DefaultCopies {
    /// Declared offset.
    pub offset: u64,
    /// Declared view.
    pub view: String,
}

/// Declared nondefault edits facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "FirmwareNondefaultEdits"))]
pub struct NondefaultEdits {
    /// Declared mask.
    pub mask: u64,
    /// Declared offset.
    pub offset: u64,
    /// Declared values.
    pub values: Vec<u64>,
}

/// Declared record facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "FirmwareRecord"))]
pub struct Record {
    /// Declared protected bytes.
    pub protected_bytes: u64,
    /// Declared size.
    pub size: u64,
}

/// Declared record copies facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "FirmwareRecordCopies"))]
pub struct RecordCopies {
    /// Declared payload offset.
    pub payload_offset: u64,
    /// Declared record offset.
    pub record_offset: u64,
    /// Declared size.
    pub size: u64,
}

/// Declared journals facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "FirmwareJournals"))]
pub struct Journals {
    /// Declared bank rows.
    pub bank_rows: Vec<u64>,
    /// Declared check view.
    pub check_view: String,
    /// Declared default copies.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub default_copies: Vec<DefaultCopies>,
    /// Declared kind.
    pub kind: u64,
    /// Declared name.
    pub name: String,
    /// Declared nondefault edits.
    pub nondefault_edits: Vec<NondefaultEdits>,
    /// Declared parity context.
    pub parity_context: u64,
    /// Declared payload size.
    pub payload_size: u64,
    /// Declared prepare view.
    pub prepare_view: String,
    /// Declared query view.
    pub query_view: String,
    /// Declared record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record: Option<Record>,
    /// Declared record copies.
    pub record_copies: Vec<RecordCopies>,
    /// Declared record tags.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub record_tags: Vec<DefaultCopies>,
}

/// Declared managed facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "FirmwareManaged"))]
pub struct Managed {
    /// Declared banks view.
    pub banks_view: String,
    /// Declared logical capacity.
    pub logical_capacity: u64,
    /// Declared start.
    pub start: u64,
}

/// Declared raw records facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "FirmwareRawRecords"))]
pub struct RawRecords {
    /// Declared bank rows.
    pub bank_rows: Vec<u64>,
    /// Declared basis.
    pub basis: String,
    /// Declared consumer view.
    pub consumer_view: String,
    /// Declared name.
    pub name: String,
    /// Declared nondefault bytes.
    pub nondefault_bytes: Vec<SyntheticSignature>,
    /// Declared seed sequences.
    pub seed_sequences: Vec<u64>,
    /// Declared size.
    pub size: u64,
}

/// Declared storagelayout record facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "FirmwareStoragelayoutRecord"))]
pub struct StoragelayoutRecord {
    /// Declared empty fill.
    pub empty_fill: u64,
    /// Declared protected bytes.
    pub protected_bytes: u64,
    /// Declared seed sequences.
    pub seed_sequences: Vec<u64>,
    /// Declared sequence offset.
    pub sequence_offset: u64,
    /// Declared size.
    pub size: u64,
}

/// Declared storage layout facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "FirmwareStorageLayout"))]
pub struct StorageLayout {
    /// Declared cfgdata.
    pub cfgdata: Cfgdata,
    /// Declared configuration banks.
    pub configuration_banks: Vec<ConfigurationBanks>,
    /// Declared custom defaults.
    pub custom_defaults: CustomDefaults,
    /// Declared geometry.
    pub geometry: StoragelayoutGeometry,
    /// Declared header.
    pub header: Header,
    /// Declared journals.
    pub journals: Vec<Journals>,
    /// Declared limits.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub limits: Vec<String>,
    /// Declared managed.
    pub managed: Managed,
    /// Declared max instructions.
    pub max_instructions: u64,
    /// Declared raw records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_records: Option<Vec<RawRecords>>,
    /// Declared record.
    pub record: StoragelayoutRecord,
    /// Declared scratch record.
    pub scratch_record: u64,
    /// Declared sections.
    pub sections: Vec<String>,
    /// Declared views.
    pub views: Vec<UsbInterruptTables>,
}

/// Located firmware layouts grouped by stable profile and view identity.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct FirmwareLayouts {
    /// Boot and device layout profiles for this firmware.
    pub profiles: BTreeMap<String, FirmwareProfile>,
    /// Immutable tables and storage layouts addressed by semantic identity.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub views: BTreeMap<String, FirmwareView>,
}

/// Closed layout families retained by the firmware catalog.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
#[cfg_attr(feature = "schema", schemars(rename = "FirmwareFirmwareView"))]
// Catalog records are few and long-lived, so the variant size difference costs nothing.
#[allow(clippy::large_enum_variant)]
pub enum FirmwareView {
    /// Firmware configuration-default builder and its source identities.
    ConfigurationDefaults(ConfigurationDefaults),
    /// Firmware control-bus descriptor and polarity tables.
    ControlBusTables(ControlBusTables),
    /// Firmware-managed persistent records, journals and table identities.
    StorageLayout(StorageLayout),
}

/// Hash-qualified source asset before placement in persistent storage.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "FirmwarePersistentAssetLayout"))]
pub struct PersistentAssetLayout {
    /// Offset of the source window within the supplied file.
    pub file_offset: u64,
    /// Source window extent in bytes.
    pub bytes: u64,
    /// SHA-256 before declared byte edits.
    pub sha256: String,
    /// SHA-256 after declared byte edits.
    pub normalized_sha256: String,
    /// Explicit small edits used to normalize the input.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub patches: Vec<fujimod_catalog_settings::persistent_inputs::PersistentPatch>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cpu(value: serde_json::Value) -> Cpu {
        serde_json::from_value(value).expect("cpu parses")
    }

    fn legacy_sysregs() -> serde_json::Value {
        serde_json::json!({
            "CPACR_EL1": 0,
            "ICC_IGRPEN0_EL1": 0,
            "ICC_IGRPEN1_EL1": 0,
            "ICC_PMR_EL1": 0,
            "SCTLR_EL1": 0,
            "SP_EL0": 0,
            "SP_EL1": 0,
            "VBAR_EL1": 0
        })
    }

    fn legacy_cpu() -> serde_json::Value {
        serde_json::json!({
            "id": 0,
            "pc": 0x991cc,
            "cpsr": 0x1c5,
            "running": true,
            "regs": {"x19": 0, "x20": 0, "x21": 0},
            "sysregs": legacy_sysregs()
        })
    }

    fn entry_cpu(level: &str, regs: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "id": 0,
            "pc": 0x58420,
            "running": true,
            "sysregs": legacy_sysregs(),
            "entry": {"level": level, "pstate": 0x3cc, "regs": regs}
        })
    }

    #[test]
    fn legacy_handoff_validates() {
        cpu(legacy_cpu()).validate().unwrap();
    }

    #[test]
    fn el3t_entry_validates() {
        cpu(entry_cpu("el3t", serde_json::json!({"x0": 1, "x1": 2})))
            .validate()
            .unwrap();
    }

    #[test]
    fn entry_wrong_mode_refuses() {
        let mut value = entry_cpu("el3t", serde_json::json!({"x0": 1}));
        value["entry"]["pstate"] = serde_json::json!(0x3cd);
        let error = cpu(value).validate().unwrap_err();
        assert!(error.contains("does not match declared level"), "{error}");
    }

    #[test]
    fn neither_shape_refuses() {
        let mut value = legacy_cpu();
        value.as_object_mut().unwrap().remove("cpsr");
        value.as_object_mut().unwrap().remove("regs");
        let error = cpu(value).validate().unwrap_err();
        assert!(error.contains("must declare"), "{error}");
    }

    #[test]
    fn both_shapes_refuse() {
        let mut value = legacy_cpu();
        value.as_object_mut().unwrap().insert(
            "entry".into(),
            serde_json::json!({"level": "el3h", "pstate": 0x3cd, "regs": {"x0": 0}}),
        );
        let error = cpu(value).validate().unwrap_err();
        assert!(error.contains("cannot declare both"), "{error}");
    }

    #[test]
    fn legacy_missing_cpsr_refuses() {
        let mut value = legacy_cpu();
        value.as_object_mut().unwrap().remove("cpsr");
        let error = cpu(value).validate().unwrap_err();
        assert!(error.contains("requires both cpsr and regs"), "{error}");
    }

    #[test]
    fn entry_x31_refuses() {
        let error = cpu(entry_cpu("el3t", serde_json::json!({"x31": 0})))
            .validate()
            .unwrap_err();
        assert!(error.contains("not x0 through x30"), "{error}");
    }

    #[test]
    fn entry_x00_refuses() {
        let error = cpu(entry_cpu("el3t", serde_json::json!({"x00": 0})))
            .validate()
            .unwrap_err();
        assert!(error.contains("not x0 through x30"), "{error}");
    }

    #[test]
    fn entry_register_names() {
        assert!(is_entry_register_name("x0"));
        assert!(is_entry_register_name("x30"));
        assert!(!is_entry_register_name("x31"));
        assert!(!is_entry_register_name("x00"));
        assert!(!is_entry_register_name("x"));
        assert!(!is_entry_register_name("w0"));
    }
}
