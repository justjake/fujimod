//! Buttons, dials, switches, touch and contact parts.
pub mod contact_actuator;
pub mod encoder;
pub mod gpio_contact_response;
pub mod i2c;
pub mod register_status;
pub mod absent;
pub mod key_input;
pub mod ladder_keys;
#[cfg(test)]
mod ladder_keys_tests;
pub mod resistor_keys;
#[cfg(test)]
mod resistor_keys_tests;
pub mod touch_controller;
