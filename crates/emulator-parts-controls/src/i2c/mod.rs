//! External parts attached to I2C buses, separated from the SoC register model.
//! Endpoint addresses, topology, protocol fields and reply inputs come from
//! checked construction data; the remaining fields hold transfer state.
pub mod command_registers;
pub mod touch_panel;

use fujimod_emulator_devices::{Value, WordMap};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct BusParts {
    pub touch: Option<touch_panel::Panel>,
    pub command_registers: Vec<command_registers::CommandRegisters>,
    pub prefixed_registers: Vec<(u16, Vec<u8>, u64)>,
    pub sensor_address: Option<u8>,
    pub prefix_address: Option<u8>,
    pub sensor_fields: Vec<u32>,
    pub prefix_high: u8,
    pub heartbeat_register: u16,
    pub topology: Option<Vec<u8>>,
    pub stationary: bool,
    pub status: Vec<crate::register_status::RegisterStatus>,
    pub payload: Vec<u8>,
    pub transactions: Vec<Value>,
    pub target: Option<u8>,
    pub target_present: bool,
    pub sensor_registers: WordMap,
    pub sensor_sample: Value,
    pub rx_metadata: Value,
    pub endpoint9_prefix: Option<u16>,
    pub heartbeat_counter: u32,
    pub heartbeat_reads: i64,
    pub target_registers: WordMap,
    pub pwic_registers: WordMap,
    pub pwic_prefix: Option<u32>,
    pub secondary_usb_connected: bool,
}
impl Default for BusParts {
    fn default() -> Self {
        Self {
            touch: None,
            command_registers: Vec::new(),
            prefixed_registers: Vec::new(),
            sensor_address: None,
            prefix_address: None,
            sensor_fields: Vec::new(),
            prefix_high: 0,
            heartbeat_register: 0,
            topology: Some(Vec::new()),
            stationary: false,
            status: Vec::new(),
            payload: Vec::new(),
            transactions: Vec::new(),
            target: None,
            target_present: true,
            sensor_registers: WordMap::default(),
            sensor_sample: Value::None,
            rx_metadata: Value::None,
            endpoint9_prefix: None,
            heartbeat_counter: 0,
            heartbeat_reads: 0,
            target_registers: WordMap::default(),
            pwic_registers: WordMap::default(),
            pwic_prefix: None,
            secondary_usb_connected: false,
        }
    }
}
impl BusParts {
    pub fn command_index(&self, address: Option<u8>) -> Option<usize> {
        self.command_registers
            .iter()
            .position(|part| Some(part.address) == address)
    }
    pub fn admitted(&self, address: Option<u8>) -> bool {
        address.is_some_and(|a| self.topology.as_ref().is_none_or(|rows| rows.contains(&a)))
    }
    pub fn status_index(&self, address: Option<u8>) -> Option<usize> {
        self.status
            .iter()
            .position(|row| Some(row.address) == address)
    }
    pub fn remember(&mut self, item: Value) {
        self.transactions.push(item);
        if self.transactions.len() > 512 {
            self.transactions.drain(..self.transactions.len() - 512);
        }
    }
}

pub fn dict(fields: Vec<(&str, Value)>) -> Value {
    Value::Dict(
        fields
            .into_iter()
            .map(|(key, value)| (Value::str(key), value))
            .collect(),
    )
}
pub fn add(dict: &mut Value, key: &str, value: Value) {
    if let Value::Dict(fields) = dict {
        fields.push((Value::str(key), value));
    }
}
pub fn get<'a>(dict: &'a Value, key: &str) -> Option<&'a Value> {
    if let Value::Dict(fields) = dict {
        fields.iter().find_map(|(name, value)| {
            if matches!(name, Value::Str(label) if label == key) {
                Some(value)
            } else {
                None
            }
        })
    } else {
        None
    }
}
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut out, "{byte:02x}").expect("String write");
    }
    out
}
