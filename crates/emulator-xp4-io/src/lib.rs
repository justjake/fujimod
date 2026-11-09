//! XP4 power serial links, SPI, ADC, RTC, timers, requests, temperature, SD host and USB PHY.
pub mod adc;
pub mod checksum_power_serial;
pub mod interval_timers;
pub mod power_serial;
pub mod requests;
pub mod rtc;
pub mod register_bank;
pub mod sd;
pub mod spi;
pub mod system_control;
pub mod temperature;
pub mod usb_condition;
pub mod usb_phy;

pub mod configuration_request;

pub mod selection_special;

pub mod base_parameter;

pub mod i2c;
