//! XP4 power serial links, SPI, I2C, ADC, RTC, timers, requests, temperature, SD host, USB PHY, face detector and filter engine.
pub mod adc;
pub mod checksum_power_serial;
pub mod countdown_timers;
pub mod face_detector;
pub mod filter_engine;
pub mod interval_timers;
pub mod power_serial;
pub mod requests;
pub mod rtc;
pub mod register_bank;
pub mod sd;
pub mod spi;
pub mod temperature;
pub mod usb_condition;
pub mod usb_phy;

pub mod i2c;
