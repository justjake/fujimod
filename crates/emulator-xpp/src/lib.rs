//! X-Processor Pro register models. The blocks differ from XP4 in layout and
//! address, so they are separate implementations selected by the SoC record.
//! Addresses and IRQ routing are explicit SoC inputs, never software-derived.
pub mod adc;
pub mod blocks;
pub mod board;
pub mod boot;
pub mod clocks;
pub mod development;
pub mod display;
pub mod dram;
pub mod face;
pub mod flash;
pub mod i2c;
pub mod imaging;
pub mod jpeg;
pub mod keys;
pub mod lens;
pub mod power;
pub mod resets;
pub mod sd;
pub mod serial;
pub mod shutter;
pub mod statistics;
pub mod sync;
pub mod timer;
