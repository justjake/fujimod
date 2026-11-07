//! Generated stock mechanisms for placing and executing a runtime payload.
//!
//! These records describe allocation, cache synchronization and callback
//! primitives. Features compose them with other mechanisms such as bulk upload
//! and deferred timers; no feature owns these target facts.
use fujimod_catalog_bulk::CodeCheck;
use serde::{Deserialize, Serialize};

pub const EXECUTION_SCHEMA_URI: &str =
    "https://fujimod.com/schemas/execution.generated.schema.json";

/// Located runtime-execution mechanism for one firmware.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct FirmwareExecution {
    #[serde(rename = "$schema")]
    pub schema_uri: String,
    pub schema: u32,
    pub id: String,
    pub firmware_id: String,
    pub product: String,
    pub internal_version: String,
    /// Uncached RAM alias for the runtime allocation and code checks.
    pub code_alias: u64,
    pub mechanism: ExecutionMechanism,
}

/// A mechanism that owns an allocation and can publish code as a diagnostic
/// callback after synchronizing its instruction view.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(missing_as_null)
)]
pub enum ExecutionMechanism {
    Exr2QueryHeap {
        allocation: HeapAllocation,
        cache: CacheSynchronization,
        code_checks: Vec<CodeCheck>,
    },
}

/// The stock allocator/release pair and its configuration-owned bootstrap
/// scratch. The bootstrap is borrowed only while allocating or releasing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
pub struct HeapAllocation {
    pub allocate: u64,
    pub release: u64,
    pub configuration: Configuration,
}

/// How the active configuration RAM base is obtained.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "mechanism", rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
pub enum Configuration {
    Pointer { address: u64 },
    Fixed { address: u64 },
}

/// Stock CPU0 cache and barrier routines invoked through the borrowed query.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
pub struct CacheSynchronization {
    pub data_clean: u64,
    pub instruction_invalidate: u64,
    pub data_barrier: u64,
    pub instruction_barrier: u64,
}

impl FirmwareExecution {
    /// Validate identity and the complete EXR II heap-execution contract.
    pub fn validate(&self) -> Result<(), String> {
        fujimod_catalog_identity::validate_id(&self.id)?;
        fujimod_catalog_identity::validate_id(&self.firmware_id)?;
        if self.schema_uri != EXECUTION_SCHEMA_URI || self.schema != 1 {
            return Err(format!("execution {}: unsupported schema", self.id));
        }
        if self.product.trim().is_empty() || self.internal_version.trim().is_empty() {
            return Err(format!("execution {}: empty camera identity", self.id));
        }
        match &self.mechanism {
            ExecutionMechanism::Exr2QueryHeap {
                allocation,
                cache,
                code_checks,
            } => {
                let configuration = match allocation.configuration {
                    Configuration::Pointer { address } | Configuration::Fixed { address } => address,
                };
                if [allocation.allocate, allocation.release, configuration, cache.data_clean,
                    cache.instruction_invalidate, cache.data_barrier, cache.instruction_barrier]
                    .into_iter().any(|address| address == 0 || !address.is_multiple_of(4))
                    || code_checks.is_empty()
                    || code_checks.iter().any(|check| check.size == 0 || check.sha256.len() != 64)
                {
                    return Err(format!("execution {}: incomplete EXR II mechanism", self.id));
                }
            }
        }
        Ok(())
    }
}
