//! What sessions and bindings do with devices, without naming their models. A model implements
//! a capability trait and returns itself from the matching `Device` accessor.
use crate::exr2::scanout_frame::Frame;
use crate::parts::sd_card::faults::{DataFault, DataFaultEvent};
use crate::parts::{nand::Nand, sd_card::Card, touch_input::ContactInput};
use crate::{Cx, Device};

/// Finds a capability on a device.
pub trait Capability: 'static {
    fn of(device: &dyn Device) -> Option<&Self>;
    fn of_mut(device: &mut dyn Device) -> Option<&mut Self>;
}

/// Analog input samples an ADC converts.
pub trait AnalogInputs {
    /// The shared external input levels the converter samples.
    fn external_inputs(&self) -> crate::parts::analog::Samples;
    fn samples(&self) -> Vec<u16>;
    fn set_sample(&mut self, channel: usize, value: u16) -> Result<(), String>;
}
impl Capability for dyn AnalogInputs {
    fn of(device: &dyn Device) -> Option<&Self> {
        device.analog_inputs()
    }
    fn of_mut(device: &mut dyn Device) -> Option<&mut Self> {
        device.analog_inputs_mut()
    }
}

/// Status registers of serial endpoints that external inputs drive.
pub trait SerialStatus {
    fn status_value(&self, route: usize, register: u8) -> Result<u8, String>;
    fn status_input(&mut self, route: usize, register: u8, value: u8) -> Result<(), String>;
}
impl Capability for dyn SerialStatus {
    fn of(device: &dyn Device) -> Option<&Self> {
        device.serial_status()
    }
    fn of_mut(device: &mut dyn Device) -> Option<&mut Self> {
        device.serial_status_mut()
    }
}

/// A touch panel that replays scripted contacts.
pub trait TouchPanel {
    fn touch_period(&self) -> Result<u64, String>;
    fn admit_touch(&self, now: u64, script: &[ContactInput]) -> Result<(), String>;
    fn touch(&mut self, now: u64, script: &[ContactInput]) -> Result<(), String>;
}
impl Capability for dyn TouchPanel {
    fn of(device: &dyn Device) -> Option<&Self> {
        device.touch_panel()
    }
    fn of_mut(device: &mut dyn Device) -> Option<&mut Self> {
        device.touch_panel_mut()
    }
}

/// An HDMI transmitter's hot-plug input.
pub trait HdmiOutput {
    fn set_connected(&mut self, cx: &mut Cx<'_>, connected: bool);
}
impl Capability for dyn HdmiOutput {
    fn of(device: &dyn Device) -> Option<&Self> {
        device.hdmi_output()
    }
    fn of_mut(device: &mut dyn Device) -> Option<&mut Self> {
        device.hdmi_output_mut()
    }
}

/// A USB device port the host drives.
pub trait UsbPort {
    fn cable_connected(&self) -> bool;
    fn bus_status(&self) -> Result<serde_json::Value, String>;
    fn host_reset(&mut self, cx: &mut Cx<'_>) -> Result<(), String>;
    fn host_setup(&mut self, cx: &mut Cx<'_>, packet: &[u8]) -> Result<(), String>;
    fn host_out(&mut self, cx: &mut Cx<'_>, endpoint: u8, packet: &[u8]) -> Result<(), String>;
    fn host_in(&mut self, cx: &mut Cx<'_>, endpoint: u8, size: usize) -> Result<Vec<u8>, String>;
    fn host_in_token(&mut self, cx: &mut Cx<'_>, endpoint: u8) -> Result<(), String>;
}
impl Capability for dyn UsbPort {
    fn of(device: &dyn Device) -> Option<&Self> {
        device.usb_port()
    }
    fn of_mut(device: &mut dyn Device) -> Option<&mut Self> {
        device.usb_port_mut()
    }
}

/// A NAND controller's medium.
pub trait NandStore {
    fn medium(&self) -> &Nand;
    fn retain(&mut self, medium: Nand) -> Result<(), String>;
}
impl Capability for dyn NandStore {
    fn of(device: &dyn Device) -> Option<&Self> {
        device.nand_store()
    }
    fn of_mut(device: &mut dyn Device) -> Option<&mut Self> {
        device.nand_store_mut()
    }
}

/// An SD host's card slot.
pub trait CardSlot {
    fn card(&self) -> &Card;
    fn busy(&self) -> bool;
    /// The inserted, idle card at the requested revision, for host file access.
    fn readable_card(&self, revision: Option<u64>) -> Result<&Card, String>;
    fn replace_card(&mut self, card: Card) -> Result<Card, String>;
    fn retain_card(&mut self, card: Card) -> Result<(), String>;
    fn initialize_revision(&mut self, revision: u64) -> Result<(), String>;
    fn inject_data_fault(&mut self, fault: DataFault) -> Result<(), String>;
    fn clear_data_faults(&mut self) -> Result<(), String>;
    fn data_faults(&self) -> &[DataFault];
    fn fault_history(&self) -> &[DataFaultEvent];
}
impl Capability for dyn CardSlot {
    fn of(device: &dyn Device) -> Option<&Self> {
        device.card_slot()
    }
    fn of_mut(device: &mut dyn Device) -> Option<&mut Self> {
        device.card_slot_mut()
    }
}

/// A display controller's scanout, read through board RAM.
pub trait ScanoutSource {
    fn metadata(&self) -> serde_json::Value;
    fn frame(
        &self,
        instance: usize,
        pixels: bool,
        read: &mut dyn FnMut(u64, usize) -> Result<Vec<u8>, String>,
        diagnostic_hashes: bool,
    ) -> Result<Option<Frame>, String>;
    fn main_frame(
        &self,
        instance: usize,
        pixels: bool,
        read: &mut dyn FnMut(u64, usize) -> Result<Vec<u8>, String>,
        diagnostic_hashes: bool,
    ) -> Result<Option<Frame>, String>;
}
impl Capability for dyn ScanoutSource {
    fn of(device: &dyn Device) -> Option<&Self> {
        device.scanout_source()
    }
    fn of_mut(device: &mut dyn Device) -> Option<&mut Self> {
        device.scanout_source_mut()
    }
}

/// A clocked frame source: its frame period and the cadence of its statistics frames.
pub trait FrameSource {
    fn configure(&mut self, period: u64, cadence: StatisticsCadence, now: u64) -> Result<(), String>;
    /// The source's clock, cadence and latched exposure, for diagnostics.
    fn frame_state(&self) -> serde_json::Value;
    /// Passive pre-ISP RGB U16LE samples on a uniform grid of the latched
    /// sensor view. Raster dimensions are bounded to 256 by 256.
    fn sensor_sample_rgb(&self, width: usize, height: usize) -> Result<Vec<u8>, String>;
}
impl Capability for dyn FrameSource {
    fn of(device: &dyn Device) -> Option<&Self> {
        device.frame_source()
    }
    fn of_mut(device: &mut dyn Device) -> Option<&mut Self> {
        device.frame_source_mut()
    }
}

/// The caller supplies physical S1 and declared sensor activity, never a
/// software mode variable. Dividers retain their absolute frame phase.
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum StatisticsCadence {
    EveryNth {
        frames: u64,
    },
    Activity {
        idle_frames: u64,
        active_frames: u64,
    },
}
impl StatisticsCadence {
    pub fn validate(self) -> Result<Self, String> {
        let valid = match self {
            Self::EveryNth { frames } => frames > 0,
            Self::Activity {
                idle_frames,
                active_frames,
            } => idle_frames > 0 && active_frames > 0,
        };
        if valid {
            Ok(self)
        } else {
            Err("statistics frame dividers must be positive".into())
        }
    }
    pub fn admits(self, index: u64, s1_pressed: bool, sensor_active: bool) -> Result<bool, String> {
        self.validate()?;
        let divisor = match self {
            Self::EveryNth { frames } => frames,
            Self::Activity {
                idle_frames,
                active_frames,
            } => {
                if s1_pressed || sensor_active {
                    active_frames
                } else {
                    idle_frames
                }
            }
        };
        Ok(index > 0 && index.is_multiple_of(divisor))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn statistics_cadence_divides_by_activity() {
        let s = StatisticsCadence::Activity {
            idle_frames: 4,
            active_frames: 1,
        };
        assert!(!s.admits(3, false, false).unwrap());
        assert!(s.admits(3, true, false).unwrap());
        assert!(s.admits(3, false, true).unwrap());
        assert!(
            StatisticsCadence::EveryNth { frames: 0 }
                .validate()
                .is_err()
        );
    }
}
