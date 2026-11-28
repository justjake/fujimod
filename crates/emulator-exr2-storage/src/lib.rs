//! EXR II SD host, SDIO and NAND controllers.
pub mod construct;
pub mod nand;

/// EXR II placement of the shared SDHCI Wi-Fi transport.
pub mod sdio {
    pub use fujimod_emulator_parts_wifi::sdhci::{Options, SdioHost, Wiring};

    pub const BASE: u64 = 0xfff51000;
}
