//! The reference catalog of SoCs, parts, bodies and firmware and their hardware profiles.
pub mod assembly;
pub mod capture;
pub mod firmware_versions;
pub mod first_start;
pub mod generation;
pub mod progress;
pub mod reference;
pub mod soc;

pub use reference::{Body, Firmware, Part, ReferenceCatalog, ResolvedSelection, Soc};
pub use soc::SocHardware;

/// Reusable part assembly attachments.
pub mod parts;

/// Named hardware profiles and device graphs.
pub mod profiles;

/// Located firmware boot and storage layouts.
pub mod firmware;

/// Supplemental source evidence and caller input identities.
pub mod provenance;

/// Explicit emulator rehearsal qualification records.
pub mod rehearsal;
