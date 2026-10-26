//! Immutable controller and attached-part construction data.
use super::{Channel, Profile, Registers, secondary};
use fujimod_emulator_devices::WordMap;
use fujimod_emulator_parts_controls::register_status;
use fujimod_emulator_parts_controls::i2c::{self, command_registers, touch_panel};
use serde::Deserialize;
use std::collections::BTreeSet;

// Constructor labels are policy input. Saved controller state uses the enum's
// symmetric typed representation, without parsing variant names as labels.
fn profile<'de, D: serde::Deserializer<'de>>(input: D) -> Result<Option<Profile>, D::Error> {
    Ok(Option::<String>::deserialize(input)?.map(Profile::from))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub base: u32,
    pub irq: u8,
    pub modes: Vec<u32>,
    pub transfer_ticks: u64,
    #[serde(default, deserialize_with = "profile")]
    pub profile: Option<Profile>,
    pub parts: Parts,
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Parts {
    Primary {
        touch: Option<touch_panel::Options>,
        sensor_address: Option<u8>,
        prefix_address: Option<u8>,
        sensor_fields: Vec<u32>,
        prefix_high: u8,
        heartbeat_register: u16,
        topology: Option<Vec<u8>>,
        stationary: bool,
        status: Vec<register_status::Options>,
        commands: Vec<command_registers::Options>,
    },
    Secondary(Box<Secondary>),
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Secondary {
    pub register_address: Option<u8>,
    pub pwic_address: Option<u8>,
    pub register_modes: Vec<u32>,
    pub pwic_modes: Vec<u32>,
    pub register_values: WordMap,
    pub register_masks: WordMap,
    pub pwic_values: WordMap,
    pub pwic_masks: WordMap,
    pub pwic_read_prefixes: Vec<(u32, Vec<u8>)>,
    pub topology: Option<Vec<u8>>,
    pub connected: Option<bool>,
}
fn topology_valid(rows: &Option<Vec<u8>>) -> bool {
    rows.as_ref().is_none_or(|r| {
        !r.is_empty()
            && r.iter().all(|&a| a < 128)
            && r.iter().copied().collect::<BTreeSet<_>>().len() == r.len()
    })
}
fn byte_map(words: &WordMap) -> bool {
    words.items().all(|(r, v)| r <= 255 && v <= 255)
}
impl Options {
    pub(super) fn build(self) -> Result<Registers, String> {
        if self.base & 0x3f != 0
            || self.base.checked_add(0x40).is_none()
            || self.irq >= 128
            || self.transfer_ticks == 0
            || self.modes.is_empty()
            || self.modes.len() > 16
            || self.modes.iter().any(|mode| !matches!(mode, 1 | 9))
            || self.modes.iter().copied().collect::<BTreeSet<_>>().len() != self.modes.len()
        {
            return Err("invalid I2C register, mode, IRQ or transfer-time inputs".into());
        }
        let primary = matches!(self.parts, Parts::Primary { .. });
        let channel = if primary {
            Channel::Primary
        } else {
            Channel::Secondary
        };
        let valid_profile = match &self.profile {
            None => true,
            Some(
                Profile::PrefixWrite
                | Profile::SensorWrite
                | Profile::Stationary
                | Profile::Heartbeat
                | Profile::LatchedHeartbeat
                | Profile::RegisterStatus,
            ) => primary,
            Some(
                Profile::UsbDisconnected
                | Profile::Pwic61
                | Profile::PwicStartup
                | Profile::DeclaredSecondary,
            ) => !primary,
            Some(Profile::Other(_)) => false,
        };
        if !valid_profile {
            return Err("I2C profile does not describe this channel".into());
        }
        let mut model = Registers::new(
            channel,
            self.base,
            self.modes,
            self.irq,
            self.transfer_ticks,
        );
        model.profile = self.profile;
        model.parts = match self.parts {
            Parts::Primary {
                touch,
                sensor_address,
                prefix_address,
                sensor_fields,
                prefix_high,
                heartbeat_register,
                topology,
                stationary,
                status,
                commands,
            } => {
                let addresses = sensor_address
                    .into_iter()
                    .chain(prefix_address)
                    .chain(status.iter().map(|s| s.address))
                    .chain(commands.iter().map(|s| s.address))
                    .collect::<Vec<_>>();
                if !topology_valid(&topology)
                    || addresses.iter().any(|&a| a >= 128)
                    || addresses.iter().copied().collect::<BTreeSet<_>>().len() != addresses.len()
                    || model.profile == Some(Profile::RegisterStatus)
                        && (status.is_empty() || topology.is_none())
                    || !commands.is_empty() && model.profile != Some(Profile::RegisterStatus)
                    || u16::from(prefix_high) != heartbeat_register >> 8
                    || sensor_fields.iter().any(|&f| f > 255)
                    || sensor_fields.iter().copied().collect::<BTreeSet<_>>().len()
                        != sensor_fields.len()
                    || stationary
                        && (model.profile != Some(Profile::RegisterStatus)
                            || sensor_address.is_none())
                {
                    return Err("invalid primary I2C attached-part inputs".into());
                }
                let status = status
                    .into_iter()
                    .map(|o| register_status::RegisterStatus::new(o, register_status::Wiring))
                    .collect::<Result<_, _>>()?;
                let command_registers = commands
                    .into_iter()
                    .map(command_registers::CommandRegisters::new)
                    .collect::<Result<_, _>>()?;
                let touch = touch.map(touch_panel::Panel::new).transpose()?;
                if touch.is_some() && prefix_address.is_none() {
                    return Err("touch requires a prefixed-register endpoint".into());
                }
                let prefixed_registers = touch
                    .as_ref()
                    .map(|t| vec![(t.options.register, vec![0; 11], 0)])
                    .unwrap_or_default();
                i2c::BusParts {
                    touch,
                    prefixed_registers,
                    sensor_address,
                    prefix_address,
                    sensor_fields,
                    prefix_high,
                    heartbeat_register,
                    topology,
                    stationary,
                    status,
                    command_registers,
                    ..Default::default()
                }
            }
            Parts::Secondary(options) => {
                let Secondary {
                    register_address,
                    pwic_address,
                    register_modes,
                    pwic_modes,
                    register_values,
                    register_masks,
                    pwic_values,
                    pwic_masks,
                    pwic_read_prefixes,
                    topology,
                    connected,
                } = *options;
                let mut prefixes = BTreeSet::new();
                if !topology_valid(&topology)
                    || register_address.is_some_and(|a| a >= 128)
                    || pwic_address.is_some_and(|a| a >= 128)
                    || register_address.is_some() && register_address == pwic_address
                    || [&register_values, &register_masks, &pwic_values, &pwic_masks]
                        .iter()
                        .any(|m| !byte_map(m))
                    || register_masks
                        .items()
                        .any(|(r, _)| register_values.get(r).is_none())
                    || pwic_read_prefixes
                        .iter()
                        .any(|(r, b)| *r > 255 || !matches!(b.len(), 0 | 3) || !prefixes.insert(*r))
                {
                    return Err("invalid secondary I2C attached-part inputs".into());
                }
                if connected == Some(true)
                    && matches!(
                        model.profile,
                        Some(Profile::UsbDisconnected | Profile::Pwic61 | Profile::PwicStartup)
                    )
                {
                    return Err("disconnected I2C input cannot model a connected USB cable".into());
                }
                model.secondary = secondary::Contract {
                    fixed_modes: true,
                    register_address,
                    pwic_address,
                    register_modes,
                    pwic_modes,
                    read_registers: register_values.items().map(|(r, _)| r).collect(),
                    register_masks,
                    pwic_masks,
                    pwic_read_prefixes,
                };
                i2c::BusParts {
                    topology,
                    target_registers: register_values,
                    pwic_registers: pwic_values,
                    secondary_usb_connected: connected.unwrap_or(false),
                    ..Default::default()
                }
            }
        };
        Ok(model)
    }
}
