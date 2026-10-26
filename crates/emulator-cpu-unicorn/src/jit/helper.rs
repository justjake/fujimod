use super::super::{Environment, HelperKind, HelperLayout, ir::*};
use super::{ADDRESS, ARGUMENTS, Body, DESCRIPTOR, ENTRY, WIDE};

impl Body {
    fn typed_call(
        &mut self,
        function: u32,
        words: u32,
        mask: u32,
        pc: u32,
        layout: &HelperLayout,
    ) -> Result<(), String> {
        self.constant(layout.pc);
        self.source_address(pc)?;
        self.mem(0x36, 2, 0);
        let mut word = 0;
        let mut argument = 0;
        while word < words {
            if argument >= 6 {
                return Err("TCI helper argument count exceeds scratch ABI".into());
            }
            self.get(2);
            self.reg(Register::new(ARGUMENTS[word as usize])?);
            self.emit(&[0xad]);
            word += 1;
            if (mask >> (2 * (argument + 1))) & 1 != 0 {
                if word >= words {
                    return Err("truncated TCI helper argument".into());
                }
                self.reg(Register::new(ARGUMENTS[word as usize])?);
                self.emit(&[0xad]);
                self.wide(32);
                self.emit(&[0x86, 0x84]);
                word += 1;
            }
            self.mem(0x37, 3, argument * 8);
            argument += 1;
        }
        self.get(2);
        self.constant(function);
        self.emit(&[0x11, 4, 0]);
        Ok(())
    }
    pub(super) fn typed_helper(
        &mut self,
        function: Value,
        words: u32,
        mask: u32,
        pc: u32,
        environment: &Environment,
    ) -> Result<bool, String> {
        let (Value::Constant(function), Some(layout)) = (function, environment.helper.as_ref())
        else {
            return Ok(false);
        };
        let Some(helper) = environment.helpers.get(&function) else {
            return Ok(false);
        };
        let kind = &helper.kind;
        if words > 12 {
            return Err("TCG helper argument count exceeds ABI".into());
        }
        let r = |n| Register::new(n).expect("fixed TCI register");
        let shifter = matches!(
            kind,
            HelperKind::ShlCarry
                | HelperKind::ShrCarry
                | HelperKind::SarCarry
                | HelperKind::RorCarry
        ) && words == 3
            && mask == 0;
        if matches!(kind, HelperKind::Trace) {
            if words != 5 || mask != 256 || helper.target == 0 {
                return Err("invalid typed trace helper signature or target".into());
            }
            // Call the same engine helper with its native Wasm signature.
            // Its hook iteration, budget/ring writes and stop handling stay in C.
            self.constant(layout.pc);
            self.source_address(pc)?;
            self.mem(0x36, 2, 0);
            for n in 0..3 {
                self.reg(r(n));
            }
            self.pair([Value::Register(r(3)), Value::Register(r(5))]);
            self.constant(helper.target);
            self.emit(&[0x11, 5, 0]);
            // The uniform TCI wrapper returns zero for a void helper.
            self.wide(0);
            self.set(WIDE);
        } else if matches!(kind, HelperKind::CheckExit) && words == 2 && mask == 0 {
            self.reg(r(0));
            self.mem(0x28, 0, layout.retry);
            self.emit(&[0x45]);
            self.reg(r(0));
            self.mem(0x28, 0, layout.cpu);
            self.mem(0x28, 0, layout.decrement);
            self.constant(0);
            self.emit(&[0x4e, 0x71, 0x04, 0x7e]);
            self.wide(0);
            self.emit(&[0x05]);
            self.typed_call(function, words, mask, pc, layout)?;
            self.emit(&[0x0b]);
            self.set(WIDE);
        } else if shifter {
            self.reg(r(0));
            self.set(DESCRIPTOR);
            self.reg(r(1));
            self.set(ENTRY);
            self.reg(r(2));
            self.constant(255);
            self.emit(&[0x71]);
            self.set(ADDRESS);
            self.get(ADDRESS);
            self.emit(&[0x04, 0x7f]);
            match kind {
                HelperKind::RorCarry => {
                    self.get(ENTRY);
                    self.get(ADDRESS);
                    self.emit(&[0x78]);
                    self.set(ENTRY);
                    self.get(DESCRIPTOR);
                    self.get(ENTRY);
                    self.constant(31);
                    self.emit(&[0x76]);
                    self.mem(0x36, 2, layout.carry);
                    self.get(ENTRY);
                }
                HelperKind::SarCarry => {
                    self.constant(32);
                    self.get(ADDRESS);
                    self.get(ADDRESS);
                    self.constant(32);
                    self.emit(&[0x4b, 0x1b]);
                    self.set(ADDRESS);
                    self.get(DESCRIPTOR);
                    self.get(ENTRY);
                    self.get(ADDRESS);
                    self.constant(1);
                    self.emit(&[0x6b, 0x76]);
                    self.constant(1);
                    self.emit(&[0x71]);
                    self.mem(0x36, 2, layout.carry);
                    self.get(ENTRY);
                    self.constant(31);
                    self.get(ADDRESS);
                    self.get(ADDRESS);
                    self.constant(31);
                    self.emit(&[0x4b, 0x1b, 0x75]);
                }
                HelperKind::ShlCarry | HelperKind::ShrCarry => {
                    self.get(ADDRESS);
                    self.constant(32);
                    self.emit(&[0x4d, 0x04, 0x7f]);
                    self.get(DESCRIPTOR);
                    self.get(ENTRY);
                    if matches!(kind, HelperKind::ShlCarry) {
                        self.constant(32);
                        self.get(ADDRESS);
                        self.emit(&[0x6b, 0x76]);
                    } else {
                        self.get(ADDRESS);
                        self.constant(1);
                        self.emit(&[0x6b, 0x76]);
                    }
                    self.constant(1);
                    self.emit(&[0x71]);
                    self.mem(0x36, 2, layout.carry);
                    self.get(ADDRESS);
                    self.constant(32);
                    self.emit(&[0x49, 0x04, 0x7f]);
                    self.get(ENTRY);
                    self.get(ADDRESS);
                    self.emit(&[
                        if matches!(kind, HelperKind::ShlCarry) {
                            0x74
                        } else {
                            0x76
                        },
                        0x05,
                    ]);
                    self.constant(0);
                    self.emit(&[0x0b, 0x05]);
                    self.get(DESCRIPTOR);
                    self.constant(0);
                    self.mem(0x36, 2, layout.carry);
                    self.constant(0);
                    self.emit(&[0x0b]);
                }
                _ => unreachable!(),
            }
            self.emit(&[0x05]);
            self.get(ENTRY);
            self.emit(&[0x0b, 0xad]);
            self.set(WIDE);
        } else {
            self.typed_call(function, words, mask, pc, layout)?;
            self.set(WIDE);
        }
        self.get(0);
        self.mem(0x28, 0, layout.uc);
        self.mem(0x2d, 0, layout.pending);
        self.emit(&[0x04, 0x40]);
        self.get(0);
        self.mem(0x28, 0, layout.uc);
        self.constant(0);
        self.mem(0x3a, 0, layout.pending);
        self.wide(0);
        self.emit(&[0x0f, 0x0b]);
        self.split([r(0), r(1)]);
        Ok(true)
    }
}
