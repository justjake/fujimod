//! Shared host session capabilities for native and browser clients.
//! Host observations never advance guest time or acknowledge device state.
pub mod card;
pub mod checkpoint;
pub mod contact_driver;
pub mod durable;
pub mod history;
pub mod inputs;
pub mod nand;
pub mod scanout;
#[cfg(test)]
mod tests;
pub mod usb;

use fujimod_emulator::board::Board;
use fujimod_emulator_devices::parts::card_contacts::Contacts;
use fujimod_emulator_exr2_resources::{Resources, SensorFrame};
use std::collections::BTreeMap;
use fujimod_emulator_devices::capability::{HdmiOutput, SerialStatus};

pub struct Session {
    pub definition: serde_json::Value,
    pub board: Board,
    pub controls: Option<fujimod_emulator_devices::parts::switch_panel::SwitchPanel>,
    pub contact_driver: Option<contact_driver::Driver>,
    pub resources: Resources,
    pub nodes: BTreeMap<String, usize>,
    pub contacts: BTreeMap<String, Contacts>,
    /// Host rendering memo; excluded from checkpoints, fingerprints and modeled state.
    pub recording_frames: std::cell::RefCell<
        BTreeMap<
            (String, usize, bool),
            fujimod_recording::cache::CaptureCache<
                Option<std::sync::Arc<fujimod_emulator_devices::exr2::scanout_frame::Frame>>,
            >,
        >,
    >,
}
impl Session {
    pub fn fingerprint(&self) -> Result<serde_json::Value, String> {
        let mut value = serde_json::json!({"schema":1,"definition":self.definition,
            "controls":self.controls.as_ref().map(|p| serde_json::json!({"spec":p.spec(),"positions":p.positions()})),
            "contact_driver":self.contact_driver.as_ref().map(|d|d.fingerprint()).transpose()?});
        value["board"] = self.board.fingerprint()?;
        value["resources"] = self.resources.fingerprint()?;
        Ok(value)
    }
    pub fn logic_input(&mut self, name: &str, high: bool) -> Result<(), String> {
        let level = self.resources.logic_level(name)?;
        let users = self.resources.wire_users(name)?;
        level.drive(high);
        for user in users {
            self.board.inputs_changed(user)?;
        }
        Ok(())
    }
    /// Change an explicitly wired HDMI sink input while processors are
    /// stopped, and deliver the corresponding HPD edge through the
    /// transmitter's native device context.
    pub fn hdmi_connection(
        &mut self,
        sink: &str,
        transmitter: &str,
        connected: bool,
    ) -> Result<(), String> {
        let sink_id = self.node(sink)?;
        let transmitter_id = self.node(transmitter)?;
        self.board
            .device_input::<fujimod_emulator_devices::parts::hdmi_sink::Input, _>(
                sink_id,
                |input, _| {
                    input.set_connected(connected);
                    Ok(())
                },
            )?;
        self.board
            .capability_input::<dyn HdmiOutput, _>(
                transmitter_id,
                |device, cx| {
                    device.set_connected(cx, connected);
                    Ok(())
                },
            )
    }
    pub fn configure_frames(
        &mut self,
        name: &str,
        period: u64,
        cadence: fujimod_emulator_devices::capability::StatisticsCadence,
    ) -> Result<(), String> {
        let now = self.board.now();
        self.board
            .edit_capability::<dyn fujimod_emulator_devices::capability::FrameSource, _>(
                self.node(name)?,
                |frames| frames.configure(period, cadence, now),
            )
    }
    pub fn frame_state(&self, name: &str) -> Result<serde_json::Value, String> {
        self.board
            .inspect_capability::<dyn fujimod_emulator_devices::capability::FrameSource, _>(
                self.node(name)?,
                |frames| Ok(frames.frame_state()),
            )
    }
    pub fn serial_state(&self, name: &str) -> Result<serde_json::Value, String> {
        self.board.device_save(self.node(name)?)
    }
    pub fn serial_status_input(
        &mut self,
        name: &str,
        route: usize,
        register: u8,
        value: u8,
    ) -> Result<(), String> {
        self.board
            .edit_capability::<dyn SerialStatus, _>(
                self.node(name)?,
                |serial| serial.status_input(route, register, value),
            )
    }
    pub fn node(&self, name: &str) -> Result<usize, String> {
        self.nodes
            .get(name)
            .copied()
            .ok_or_else(|| format!("no such session node {name}"))
    }
    pub fn gpio(&mut self, name: &str, port: u8, pin: u8, high: bool) -> Result<(), String> {
        self.resources.pads(name)?.request(
            fujimod_emulator_devices::parts::pins::Pin { port, pin },
            high,
        )?;
        self.board.inputs_changed(self.node(name)?)
    }
    pub fn gpio_input_levels(&self, name: &str) -> Result<[u32; 16], String> {
        Ok(self.resources.pads(name)?.input_levels())
    }
    pub fn gpio_level(&self, name: &str, port: u8, pin: u8) -> Result<bool, String> {
        self.resources
            .pads(name)?
            .input_level(fujimod_emulator_devices::parts::pins::Pin { port, pin })
    }
    pub fn gpio_pad_state(
        &self,
        name: &str,
        port: u8,
        pin: u8,
    ) -> Result<fujimod_emulator_devices::exr2::gpio::PadState, String> {
        self.resources
            .pads(name)?
            .pad_state(fujimod_emulator_devices::parts::pins::Pin { port, pin })
    }
    pub fn rotate(&mut self, name: &str, cycles: i32) -> Result<(), String> {
        self.resources
            .encoder(name)?
            .queue_cycles(self.board.now(), cycles)?;
        self.board.inputs_changed(self.node(name)?)
    }
    fn image_inputs_changed(&mut self, name: &str) -> Result<(), String> {
        for user in self.resources.image_users(name)? {
            self.board.inputs_changed(user)?;
        }
        Ok(())
    }
    pub fn rgb(
        &mut self,
        name: &str,
        width: usize,
        height: usize,
        bytes: &[u8],
    ) -> Result<(), String> {
        self.resources
            .images()?
            .prepare_rgb(name, width, height, bytes)?;
        self.image_inputs_changed(name)
    }
    pub fn analytic(&mut self, name: &str) -> Result<(), String> {
        self.resources.images()?.use_analytic(name)?;
        self.image_inputs_changed(name)
    }
    pub fn sensor_image(&mut self, name: &str) -> Result<(), String> {
        self.resources.images()?.use_sensor(name)?;
        self.image_inputs_changed(name)
    }
    pub fn sensor_rgb(
        &self,
        name: &str,
        width: usize,
        height: usize,
        bytes: &[u8],
        white_luminance: f64,
    ) -> Result<(), String> {
        self.resources
            .images()?
            .sensor_rgb(name, width, height, bytes, white_luminance)
    }
    /// Replace a sensor input with an external little-endian U16 RAW raster.
    /// The sensor's programmed window and transfer format still control DMA.
    pub fn sensor_raw16(
        &self,
        name: &str,
        width: usize,
        height: usize,
        bytes: &[u8],
    ) -> Result<(), String> {
        self.resources
            .images()?
            .sensor_raw16(name, width, height, bytes)
    }
    /// Decode a RAF's visible crop as mosaic radiance. Its recorded exposure
    /// is the normalization, so a later exposure scales the sensor counts.
    pub fn decode_raf(&self, bytes: &[u8]) -> Result<SensorFrame, String> {
        self.resources.images()?.decode_raf(bytes)
    }
    /// Install a RAF's visible crop as mosaic radiance.
    pub fn sensor_raf(&self, name: &str, bytes: &[u8]) -> Result<(), String> {
        let frame = self.decode_raf(bytes)?;
        self.sensor_frame(name, &frame)
    }
    /// Install shared immutable sensor pixels without retaining source file bytes.
    pub fn sensor_frame(&self, name: &str, frame: &SensorFrame) -> Result<(), String> {
        self.resources.images()?.sensor_frame(name, frame)
    }
}
