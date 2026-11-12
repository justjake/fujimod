//! An SDIO card in an SDHCI slot. The card protocol (CMD5, CMD52, CMD53) and
//! the function device behind it are not modeled: a card records only whether
//! it occupies the slot, how many functions it declares and whether it asserts
//! its interrupt. A host refuses commands until the protocol exists.
use serde::{Deserialize, Serialize};

/// The largest function number SDIO defines.
pub const MAX_FUNCTIONS: u8 = 7;

/// A declared SDIO card: presence, function count and interrupt state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SdioCard {
    present: bool,
    interrupt: bool,
    functions: u8,
}

impl SdioCard {
    /// A present card with one function and no asserted interrupt.
    pub fn new_test() -> Self {
        Self {
            present: true,
            interrupt: false,
            functions: 1,
        }
    }
    /// A present card declaring `functions` functions, 1 through 7.
    pub fn with_functions(functions: u8) -> Result<Self, String> {
        let card = Self {
            present: true,
            interrupt: false,
            functions,
        };
        card.validate()?;
        Ok(card)
    }
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=MAX_FUNCTIONS).contains(&self.functions) {
            return Err("SDIO card declares one to seven functions".into());
        }
        Ok(())
    }
    pub fn present(&self) -> bool {
        self.present
    }
    /// Declare whether the modeled part occupies the slot.
    pub fn set_present(&mut self, present: bool) {
        self.present = present;
    }
    pub fn interrupt(&self) -> bool {
        self.interrupt
    }
    /// Assert or release the card's interrupt.
    pub fn set_interrupt(&mut self, asserted: bool) {
        self.interrupt = asserted;
    }
    pub fn functions(&self) -> u8 {
        self.functions
    }
    /// Diagnostic state; the value [`Self::restore`] accepts.
    pub fn state(&self) -> serde_json::Value {
        serde_json::json!({
            "present": self.present,
            "interrupt": self.interrupt,
            "functions": self.functions,
        })
    }
    /// Whether a recorded value has this card's shape rather than a memory
    /// SD card's.
    pub fn is_state(state: &serde_json::Value) -> bool {
        ["present", "interrupt", "functions"]
            .iter()
            .all(|key| state.get(*key).is_some())
            && state.get("sectors").is_none()
    }
    /// Rebuild the card from a value [`Self::state`] recorded.
    pub fn restore(state: &serde_json::Value) -> Result<Self, String> {
        if !Self::is_state(state) {
            return Err("SDIO card state has the wrong shape".into());
        }
        let card: Self =
            serde_json::from_value(state.clone()).map_err(|e| format!("SDIO card state: {e}"))?;
        card.validate()?;
        Ok(card)
    }
}

impl Default for SdioCard {
    fn default() -> Self {
        Self::new_test()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn declared_function_count_and_interrupt_survive_a_state_round_trip() {
        let mut card = SdioCard::with_functions(3).unwrap();
        card.set_interrupt(true);
        assert_eq!(SdioCard::restore(&card.state()).unwrap(), card);
        assert!(SdioCard::is_state(&card.state()));
        assert!(!SdioCard::is_state(
            &serde_json::json!({"sectors": 0x800000})
        ));
        assert_eq!(SdioCard::default().functions(), 1);
        for functions in [0, MAX_FUNCTIONS + 1] {
            assert!(SdioCard::with_functions(functions).is_err());
        }
    }
}
