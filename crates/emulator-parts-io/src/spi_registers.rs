//! Attached register endpoints on SPI1, separate from the SoC controller.
//! Each endpoint has a declared bank and optional read-side status inputs.
//! Initial values and write replies are inputs; only declared registers exist.
//! Stable model-owned slots retain a selected bank across host replacement.
use fujimod_emulator_devices::WordMap;
pub type RegisterBank = usize;
#[derive(Clone, Debug, Default)]
pub struct RegisterEndpoints {
    pub entries: Vec<(u32, RegisterBank)>,
    pub banks: Vec<WordMap>,
    /// Read-side external status inputs. Writes are commands, not loopback.
    pub status_registers: Vec<(u32, u32)>,
}
impl RegisterEndpoints {
    pub fn complete_write(&mut self, endpoint: u32, bank: RegisterBank, register: u32, value: u32) {
        if !self.status_registers.contains(&(endpoint, register)) {
            self.banks[bank].insert(register, value);
        }
    }
    pub fn get(&self, endpoint: u32) -> Option<RegisterBank> {
        self.entries
            .iter()
            .find(|(id, _)| *id == endpoint)
            .map(|(_, bank)| *bank)
    }
    pub fn insert(&mut self, endpoint: u32, values: WordMap) {
        let bank = self.banks.len();
        self.banks.push(values);
        if let Some((_, old)) = self.entries.iter_mut().find(|(id, _)| *id == endpoint) {
            *old = bank;
        } else {
            self.entries.push((endpoint, bank));
        }
    }
    pub fn remove(&mut self, endpoint: u32) -> Option<RegisterBank> {
        let pos = self.entries.iter().position(|(id, _)| *id == endpoint)?;
        Some(self.entries.remove(pos).1)
    }
}
