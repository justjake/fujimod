//! X-Processor Pro register models. The blocks differ from XP4 in layout and
//! address, so they are separate implementations selected by the SoC record.
//! Addresses and IRQ routing are explicit SoC inputs, never software-derived.
pub mod board;
pub mod boot;
pub mod clocks;
pub mod flash;
pub mod sd;
pub mod serial;
pub mod timer;
