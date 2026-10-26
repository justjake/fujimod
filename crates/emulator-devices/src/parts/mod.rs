//! Handles that board parts, their controllers, plans and sessions share: pins, GPIO,
//! controls, analog samples, card contacts, encoder inputs, lens bridge registers and
//! HDMI sinks.
pub mod analog;
pub mod card_contacts;
pub mod controls;
pub mod encoder_input;
pub mod gpio;
pub mod hdmi_sink;
pub mod lens_bridge;
pub mod micron_nor;
pub mod nand;
pub mod optics;
pub mod pins;
pub mod read_fifo;
pub mod sd_card;
pub mod switch_panel;
pub mod touch_input;
