//! Explicit electrical levels at an ADC's external pins.
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
pub struct Samples(Arc<Mutex<Vec<u16>>>);
impl Samples {
    pub fn new(values: Vec<u16>) -> Result<Self, String> {
        if values.is_empty() || values.len() > 32 {
            return Err("analog input requires 1..32 channels".into());
        }
        Ok(Self(Arc::new(Mutex::new(values))))
    }
    pub fn values(&self) -> Vec<u16> {
        self.0.lock().unwrap().clone()
    }
    pub fn get(&self, channel: usize) -> Result<u16, String> {
        self.0
            .lock()
            .unwrap()
            .get(channel)
            .copied()
            .ok_or("analog channel is not connected".into())
    }
    pub fn set(&self, channel: usize, value: u16) -> Result<(), String> {
        *self
            .0
            .lock()
            .unwrap()
            .get_mut(channel)
            .ok_or("analog channel is not connected")? = value;
        Ok(())
    }
    pub fn replace(&self, values: &[u16]) -> Result<(), String> {
        let mut old = self.0.lock().unwrap();
        if old.len() != values.len() {
            return Err("analog input channel count differs".into());
        }
        old.copy_from_slice(values);
        Ok(())
    }
}
