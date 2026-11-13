//! An SDIO card in an SDHCI slot: the common register space with its CIS, the
//! per-function basic registers, the register window a function's device
//! answers through, and the card interrupt.
//!
//! The address is the card's 17-bit SDIO address space: `0x000..0x0ff` the
//! common registers (CCCR), `0x100..0x6ff` the basic registers of functions
//! one through seven, and `0x10000..0x6ffff` the register window of the
//! function the address names. A command names the function it addresses; the
//! common space answers the host whether the command names function zero or
//! the function whose basic registers or window the address reaches.
//!
//! The register window holds the bytes a function's device answers with, which
//! is where a chip model reads its frames and writes its backplane window. The
//! card does not interpret those bytes.
use serde::{Deserialize, Serialize};

/// The largest function number SDIO defines.
pub const MAX_FUNCTIONS: u8 = 7;
/// Bytes of the common register space: the CCCR and the common CIS.
pub const COMMON_BYTES: usize = 0x100;
/// Bytes of basic registers each function declares.
pub const BASIC_BYTES: usize = 0x100;
/// Bytes of the register window each function answers through.
pub const FUNCTION_BYTES: usize = 0x1000;
/// Offset of the common CIS inside the common register space.
pub const COMMON_CIS: usize = 0x20;
/// Offset of a function's CIS inside its register window.
pub const FUNCTION_CIS: usize = 0x800;
/// The block size function zero declares and function one is programmed with.
pub const BLOCK_SIZE: usize = 512;
/// The I/O OCR the card answers in its R4 response: the 3.3-3.4 V window. The
/// 1.8 V signaling bit stays clear, so initialization keeps the slot's 3.3 V
/// level instead of asking the host for a voltage switch.
pub const IO_OCR: u32 = 0x0020_0000;
/// Broadcom's SDIO vendor ID.
pub const VENDOR_BROADCOM: u16 = 0x02d0;
/// The CYW43455's SDIO device ID.
pub const DEVICE_CYW43455: u16 = 0xa9bf;
/// The SDIO standard interface class of a WLAN function.
pub const CLASS_WLAN: u8 = 0x07;

/// CIS tuple codes.
const TPL_MANFID: u8 = 0x20;
const TPL_FUNCID: u8 = 0x21;
const TPL_FUNCE: u8 = 0x22;
const TPL_END: u8 = 0xff;
/// Function extension tuple types.
const FUNCE_COMMON: u8 = 0x00;
const FUNCE_FUNCTION: u8 = 0x01;
/// Bytes of a function extension body for SDIO 1.10 and later.
const FUNCE_FUNCTION_BYTES: usize = 42;
/// Transport speed byte of the common function extension: 25 MHz in default
/// speed, which is the bus a 4-bit WLAN function starts on.
const SPEED_25MHZ: u8 = 0x32;
/// Common CIS function ID of an SDIO card.
const FUNCID_SDIO: u8 = 0x0c;
/// Common register bytes the host may not write.
const COMMON_READ_ONLY: [usize; 7] = [0x00, 0x01, 0x03, 0x08, 0x09, 0x0a, 0x0b];
/// Basic register bytes the host may not write.
const BASIC_READ_ONLY: [usize; 2] = [0x00, 0x01];

/// A declared SDIO card with function zero and one to seven function windows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SdioCard {
    present: bool,
    functions: u8,
    #[serde(with = "crate::checkpoint::bytes")]
    common: Box<[u8]>,
    #[serde(with = "crate::checkpoint::bytes")]
    basic: Box<[u8]>,
    #[serde(with = "crate::checkpoint::bytes")]
    registers: Box<[u8]>,
    /// Bit `f` is set while function `f` asserts an interrupt.
    pending: u8,
}

/// Which part of the card an access names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Space {
    Common(usize),
    Basic(u8, usize),
    Window(u8, usize),
}

impl SdioCard {
    /// A present CYW43455 WLAN card: one function behind a Broadcom
    /// manufacturer ID, a WLAN function class and a 512-byte block size.
    pub fn cyw43455() -> Result<Self, String> {
        Self::with_functions(1)
    }

    /// A present card declaring `functions` functions, one through seven.
    pub fn with_functions(functions: u8) -> Result<Self, String> {
        if !(1..=MAX_FUNCTIONS).contains(&functions) {
            return Err("SDIO card declares one to seven functions".into());
        }
        let mut card = Self {
            present: true,
            functions,
            common: vec![0; COMMON_BYTES].into_boxed_slice(),
            basic: vec![0; functions as usize * BASIC_BYTES].into_boxed_slice(),
            registers: vec![0; functions as usize * FUNCTION_BYTES].into_boxed_slice(),
            pending: 0,
        };
        card.declare()?;
        card.validate()?;
        Ok(card)
    }

    /// Write the CCCR identity, both CIS areas and every function's basic
    /// registers and CIS.
    fn declare(&mut self) -> Result<(), String> {
        // CCCR and FBR version 2.00; SD Physical 2.00; every function enabled
        // and ready; one common CIS.
        self.common[0x00] = 0x33;
        self.common[0x01] = 0x02;
        self.common[0x02] = self.present_mask();
        self.common[0x03] = self.present_mask();
        // Multi-block transfers; the card is a full-speed device, so the
        // low-speed and 4-bit-low-speed bits stay clear.
        self.common[0x08] = 0x02;
        self.common[0x09..0x0c].copy_from_slice(&(COMMON_CIS as u32).to_le_bytes()[..3]);
        self.common[0x10] = BLOCK_SIZE as u8;
        self.common[0x11] = (BLOCK_SIZE >> 8) as u8;
        // High-speed support; the host enables it by writing the enable bit
        // back to 0x13.
        self.common[0x13] = 0x01;
        let manfid = [
            (VENDOR_BROADCOM & 0xff) as u8,
            (VENDOR_BROADCOM >> 8) as u8,
            (DEVICE_CYW43455 & 0xff) as u8,
            (DEVICE_CYW43455 >> 8) as u8,
        ];
        let mut at = COMMON_CIS;
        at = put_tuple(&mut self.common, at, TPL_MANFID, &manfid)?;
        at = put_tuple(&mut self.common, at, TPL_FUNCID, &[FUNCID_SDIO, 0])?;
        at = put_tuple(
            &mut self.common,
            at,
            TPL_FUNCE,
            &[
                FUNCE_COMMON,
                BLOCK_SIZE as u8,
                (BLOCK_SIZE >> 8) as u8,
                SPEED_25MHZ,
            ],
        )?;
        self.common[at] = TPL_END;
        for function in 1..=self.functions {
            let basic = (function as usize - 1) * BASIC_BYTES;
            let window = (function as usize - 1) * FUNCTION_BYTES;
            let cis = u32::from(function) << 16 | FUNCTION_CIS as u32;
            let basic = &mut self.basic[basic..basic + BASIC_BYTES];
            basic[0x00] = CLASS_WLAN;
            basic[0x09..0x0c].copy_from_slice(&cis.to_le_bytes()[..3]);
            basic[0x10] = BLOCK_SIZE as u8;
            basic[0x11] = (BLOCK_SIZE >> 8) as u8;
            let window = &mut self.registers[window..window + FUNCTION_BYTES];
            let mut at = FUNCTION_CIS;
            at = put_tuple(window, at, TPL_MANFID, &manfid)?;
            let mut body = [0u8; FUNCE_FUNCTION_BYTES];
            body[0] = FUNCE_FUNCTION;
            body[1] = FUNCE_FUNCTION_BYTES as u8;
            body[12] = BLOCK_SIZE as u8;
            body[13] = (BLOCK_SIZE >> 8) as u8;
            // Enable timeout in 10 ms units.
            body[29] = 10;
            at = put_tuple(window, at, TPL_FUNCE, &body)?;
            window[at] = TPL_END;
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), String> {
        if !(1..=MAX_FUNCTIONS).contains(&self.functions) {
            return Err("SDIO card declares one to seven functions".into());
        }
        let functions = self.functions as usize;
        if self.common.len() != COMMON_BYTES
            || self.basic.len() != functions * BASIC_BYTES
            || self.registers.len() != functions * FUNCTION_BYTES
        {
            return Err("SDIO register extent does not match the function count".into());
        }
        if self.pending & !self.present_mask() != 0 {
            return Err("SDIO interrupt pending bit names an absent function".into());
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

    /// The I/O OCR this card answers in its R4 response.
    pub fn ocr(&self) -> u32 {
        IO_OCR
    }

    pub fn functions(&self) -> u8 {
        self.functions
    }

    /// Bits of the functions this card declares, function zero included.
    fn present_mask(&self) -> u8 {
        ((1u16 << (u16::from(self.functions) + 1)) - 1) as u8
    }

    /// The card asserts its interrupt while a function has a pending interrupt
    /// and the CCCR master interrupt enable allows it.
    pub fn interrupt(&self) -> bool {
        self.present && self.common[0x04] & 0x01 != 0 && self.pending != 0
    }

    /// Assert or release one function's interrupt.
    pub fn set_interrupt(&mut self, function: u8, asserted: bool) -> Result<(), String> {
        if function == 0 || function > self.functions {
            return Err(format!("SDIO function {function} is not present"));
        }
        if asserted {
            self.pending |= 1 << function;
        } else {
            self.pending &= !(1 << function);
        }
        Ok(())
    }

    /// The block size one function declares: function zero in the CCCR, any
    /// other function in its own basic registers.
    pub fn block_size(&self, function: u8) -> Result<usize, String> {
        let at = if function == 0 {
            0x10
        } else {
            if !(1..=self.functions).contains(&function) {
                return Err(format!("SDIO function {function} is not present"));
            }
            (function as usize - 1) * BASIC_BYTES + 0x10
        };
        let declared = if function == 0 {
            &self.common[at..at + 2]
        } else {
            &self.basic[at..at + 2]
        };
        Ok(usize::from(declared[0]) | usize::from(declared[1]) << 8)
    }

    /// Resolve one command's function and address into the space it names.
    fn space(&self, function: u8, address: u32) -> Result<Space, String> {
        if function > self.functions && function != 0 {
            return Err(format!("SDIO function {function} is not present"));
        }
        let functions = u32::from(self.functions);
        let owner = if address < COMMON_BYTES as u32 {
            0
        } else if address < COMMON_BYTES as u32 + functions * BASIC_BYTES as u32 {
            (address >> 8) as u8
        } else if address >> 16 >= 1 && address >> 16 <= functions {
            (address >> 16) as u8
        } else {
            return Err(format!("SDIO address {address:#x} is not on the card"));
        };
        if function != 0 && function != owner {
            return Err(format!(
                "SDIO function {function} does not answer address {address:#x}"
            ));
        }
        if owner == 0 {
            return Ok(Space::Common(address as usize));
        }
        if address >> 16 == u32::from(owner) {
            let offset = (address & 0xffff) as usize;
            if offset >= FUNCTION_BYTES {
                return Err(format!(
                    "SDIO function {owner} address {offset:#x} is outside its register window"
                ));
            }
            Ok(Space::Window(owner, offset))
        } else {
            Ok(Space::Basic(owner, (address & 0xff) as usize))
        }
    }

    /// CMD52: one byte of the card's address space. The value is read back
    /// after a write, which the host reports only for a read-after-write.
    pub fn direct(&mut self, function: u8, address: u32, write: Option<u8>) -> Result<u8, String> {
        match self.space(function, address)? {
            Space::Common(offset) => {
                if let Some(value) = write {
                    if COMMON_READ_ONLY.contains(&offset) {
                        return Err(format!("SDIO CCCR {offset:#x} is read-only"));
                    }
                    if offset == 0x05 {
                        // Interrupt pending bits clear by writing one; the
                        // function device asserts them when it has work.
                        self.pending &= !value;
                    } else if offset == 0x02 {
                        self.common[0x02] = value & self.present_mask();
                        self.common[0x03] = self.common[0x02];
                    } else {
                        self.common[offset] = value;
                    }
                }
                Ok(if offset == 0x05 {
                    self.pending
                } else {
                    self.common[offset]
                })
            }
            Space::Basic(owner, offset) => {
                let at = (owner as usize - 1) * BASIC_BYTES + offset;
                if let Some(value) = write {
                    if BASIC_READ_ONLY.contains(&offset) {
                        return Err(format!(
                            "SDIO function {owner} basic register {offset:#x} is read-only"
                        ));
                    }
                    self.basic[at] = value;
                }
                Ok(self.basic[at])
            }
            Space::Window(owner, offset) => {
                let at = (owner as usize - 1) * FUNCTION_BYTES + offset;
                if let Some(value) = write {
                    self.registers[at] = value;
                }
                Ok(self.registers[at])
            }
        }
    }

    /// CMD53 read from one function's register window.
    pub fn read(
        &mut self,
        function: u8,
        address: u32,
        increment: bool,
        out: &mut [u8],
    ) -> Result<(), String> {
        let offset = self.window(function, address, increment, out.len())?;
        let base = (function as usize - 1) * FUNCTION_BYTES;
        for (at, byte) in out.iter_mut().enumerate() {
            *byte = self.registers[base + if increment { offset + at } else { offset }];
        }
        Ok(())
    }

    /// CMD53 write into one function's register window.
    pub fn write_regs(
        &mut self,
        function: u8,
        address: u32,
        increment: bool,
        bytes: &[u8],
    ) -> Result<(), String> {
        let offset = self.window(function, address, increment, bytes.len())?;
        let base = (function as usize - 1) * FUNCTION_BYTES;
        for (at, byte) in bytes.iter().enumerate() {
            self.registers[base + if increment { offset + at } else { offset }] = *byte;
        }
        Ok(())
    }

    /// Check that a data transfer names a function window and fits in it.
    fn window(
        &self,
        function: u8,
        address: u32,
        increment: bool,
        len: usize,
    ) -> Result<usize, String> {
        if len == 0 {
            return Err("SDIO transfer has no bytes".into());
        }
        match self.space(function, address)? {
            Space::Window(_, offset) => {
                let end = if increment {
                    offset
                        .checked_add(len)
                        .ok_or("SDIO transfer address overflow")?
                } else {
                    offset + 1
                };
                if end > FUNCTION_BYTES {
                    return Err(format!(
                        "SDIO function {function} transfer of {len} bytes at {offset:#x} leaves its register window"
                    ));
                }
                Ok(offset)
            }
            _ => Err(format!(
                "SDIO data transfers reach a function register window, not address {address:#x}"
            )),
        }
    }

    /// Diagnostic state. The bytes stay in the typed checkpoint, so this is
    /// never a restore source; [`Self::restore`] reads the recorded form.
    pub fn state(&self) -> serde_json::Value {
        serde_json::json!({
            "present": self.present,
            "functions": self.functions,
            "common": crate::state::bytes(&self.common),
            "basic": crate::state::bytes(&self.basic),
            "registers": crate::state::bytes(&self.registers),
            "pending": self.pending,
        })
    }

    /// Whether a recorded value has this card's shape rather than a memory SD
    /// card's.
    pub fn is_state(state: &serde_json::Value) -> bool {
        ["present", "functions", "basic", "registers", "pending"]
            .iter()
            .all(|key| state.get(*key).is_some())
            && state.get("sectors").is_none()
    }

    /// Rebuild the card from the recorded form [`serde_json::to_value`] writes.
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
        Self::cyw43455().expect("a one-function SDIO card")
    }
}

/// Append one CIS tuple, returning the offset after it.
fn put_tuple(space: &mut [u8], at: usize, code: u8, body: &[u8]) -> Result<usize, String> {
    let end = at + 2 + body.len();
    if end > space.len() {
        return Err("SDIO CIS tuple leaves its register space".into());
    }
    space[at] = code;
    space[at + 1] = body.len() as u8;
    space[at + 2..end].copy_from_slice(body);
    Ok(end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_common_cis_and_function_registers_name_a_wlan_function() {
        let card = SdioCard::cyw43455().unwrap();
        assert_eq!(
            &card.common[COMMON_CIS..COMMON_CIS + 6],
            &[TPL_MANFID, 4, 0xd0, 0x02, 0xbf, 0xa9]
        );
        assert_eq!(card.block_size(0).unwrap(), BLOCK_SIZE);
        assert_eq!(card.block_size(1).unwrap(), BLOCK_SIZE);
        assert!(card.block_size(2).is_err());
        assert_eq!(card.ocr(), IO_OCR);
        assert_eq!(card.functions(), 1);
        for functions in [0, MAX_FUNCTIONS + 1] {
            assert!(SdioCard::with_functions(functions).is_err());
        }
        assert_eq!(
            SdioCard::with_functions(MAX_FUNCTIONS).unwrap().functions(),
            7
        );
    }

    #[test]
    fn the_function_cis_declares_a_512_byte_block_size() {
        let mut card = SdioCard::cyw43455().unwrap();
        // Linux reads the CIS pointer from the basic registers and parses the
        // tuples it points at through the common space.
        let pointer = u32::from(card.direct(0, 0x109, None).unwrap())
            | u32::from(card.direct(0, 0x10a, None).unwrap()) << 8
            | u32::from(card.direct(0, 0x10b, None).unwrap()) << 16;
        assert_eq!(pointer, 0x1_0800);
        assert_eq!(card.direct(0, pointer, None).unwrap(), TPL_MANFID);
        assert_eq!(card.direct(0, pointer + 1, None).unwrap(), 4);
        let funce = pointer + 6;
        assert_eq!(card.direct(0, funce, None).unwrap(), TPL_FUNCE);
        assert_eq!(card.direct(0, funce + 1, None).unwrap(), 42);
        let block = u16::from(card.direct(0, funce + 2 + 12, None).unwrap())
            | u16::from(card.direct(0, funce + 2 + 13, None).unwrap()) << 8;
        assert_eq!(usize::from(block), BLOCK_SIZE);
        assert_eq!(card.direct(0, funce + 2 + 42, None).unwrap(), TPL_END);
    }

    #[test]
    fn commands_are_resolved_by_address_and_named_function() {
        let mut card = SdioCard::cyw43455().unwrap();
        // The common space answers function zero.
        assert_eq!(card.direct(0, 0x00, None).unwrap(), 0x33);
        assert!(
            card.direct(1, 0x00, None)
                .unwrap_err()
                .contains("does not answer")
        );
        assert!(
            card.direct(2, 0x00, None)
                .unwrap_err()
                .contains("not present")
        );
        assert!(
            card.direct(0, 0x8000, None)
                .unwrap_err()
                .contains("not on the card")
        );
        // Function one's basic registers answer function one, and the common
        // space reaches them as the CIS reader does.
        assert_eq!(card.direct(1, 0x100, None).unwrap(), CLASS_WLAN);
        assert!(card.direct(2, 0x100, None).is_err());
        assert!(
            card.direct(0, 0x100, None).is_ok(),
            "the common space reaches the basic registers"
        );
        card.direct(1, 0x10004, Some(0x5a)).unwrap();
        assert_eq!(card.direct(1, 0x10004, None).unwrap(), 0x5a);
        assert!(card.direct(0, 0x104, Some(1)).is_ok());
        assert_eq!(card.direct(0, 0x104, None).unwrap(), 1);
        assert!(
            card.direct(0, 0x00, Some(1))
                .unwrap_err()
                .contains("read-only")
        );
        assert!(
            card.direct(0, 0x100, Some(1))
                .unwrap_err()
                .contains("read-only")
        );
        assert!(card.direct(1, 0x19000, None).is_err(), "outside the window");
    }

    #[test]
    fn a_block_transfer_reaches_only_a_function_window() {
        let mut card = SdioCard::cyw43455().unwrap();
        card.write_regs(1, 0x10000, true, &[1, 2, 3, 4]).unwrap();
        let mut out = [0; 4];
        card.read(1, 0x10000, true, &mut out).unwrap();
        assert_eq!(out, [1, 2, 3, 4]);
        // Without address increment every byte reaches the first address.
        card.read(1, 0x10002, false, &mut out).unwrap();
        assert_eq!(out, [3, 3, 3, 3]);
        card.write_regs(1, 0x10008, false, &[0x7f, 0x7f]).unwrap();
        let mut pair = [0; 2];
        card.read(1, 0x10008, true, &mut pair).unwrap();
        assert_eq!(pair, [0x7f, 0]);
        assert!(
            card.read(1, 0x10000, true, &mut [0; FUNCTION_BYTES + 1])
                .is_err()
        );
        assert!(card.read(1, 0x1f00, true, &mut [0; 0x101]).is_err());
        assert!(
            card.read(0, 0x00, true, &mut out).is_err(),
            "the common space is not a data target"
        );
        assert!(
            card.write_regs(1, 0x100, true, &[1]).is_err(),
            "basic registers are not a data target"
        );
    }

    #[test]
    fn the_card_interrupt_needs_a_pending_function_and_the_master_enable() {
        let mut card = SdioCard::cyw43455().unwrap();
        card.set_interrupt(1, true).unwrap();
        assert!(!card.interrupt(), "the master interrupt enable is clear");
        card.direct(0, 0x04, Some(0x01)).unwrap();
        assert!(card.interrupt());
        assert_eq!(card.direct(0, 0x05, None).unwrap(), 0x02);
        // Writing one to a pending bit clears it.
        card.direct(0, 0x05, Some(0x02)).unwrap();
        assert!(!card.interrupt());
        card.set_interrupt(1, true).unwrap();
        card.set_interrupt(1, false).unwrap();
        assert!(!card.interrupt());
        assert!(card.set_interrupt(2, true).is_err());
        assert!(card.set_interrupt(0, true).is_err());
    }

    #[test]
    fn declared_functions_and_their_registers_survive_a_recorded_round_trip() {
        let mut card = SdioCard::with_functions(3).unwrap();
        card.direct(2, 0x20010, Some(0x40)).unwrap();
        card.set_interrupt(3, true).unwrap();
        let recorded = serde_json::to_value(&card).unwrap();
        assert_eq!(SdioCard::restore(&recorded).unwrap(), card);
        assert!(SdioCard::is_state(&recorded));
        assert!(!SdioCard::is_state(
            &serde_json::json!({"sectors": 0x800000})
        ));
        let mut broken = recorded.clone();
        broken["functions"] = 0.into();
        assert!(SdioCard::restore(&broken).is_err());
        assert_eq!(SdioCard::default().functions(), 1);
    }
}
