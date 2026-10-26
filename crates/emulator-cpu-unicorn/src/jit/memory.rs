use super::super::{MemoryLayout, ir::*};
use super::{ADDRESS, Body, DESCRIPTOR, ENTRY, WIDE};
impl Body {
    fn slow_memory(
        &mut self,
        store: bool,
        data: Data,
        address: Register,
        memop: u32,
        pc: u32,
    ) -> Result<(), String> {
        self.get(0);
        self.reg(address);
        if store {
            match data {
                Data::Word(r) => {
                    self.reg(r);
                    self.emit(&[0xad]);
                }
                Data::Double(pair) => self.pair(pair.map(Value::Register)),
            }
        }
        self.constant(memop);
        self.source_address(pc)?;
        self.emit(&[0x10, u8::from(store)]);
        Ok(())
    }
    fn reverse_bytes(&mut self, size: u32) {
        self.set(WIDE + 1);
        for n in 0..size {
            self.get(WIDE + 1);
            self.wide(u64::from(n * 8));
            self.emit(&[0x88]);
            self.wide(255);
            self.emit(&[0x83]);
            self.wide(u64::from((size - 1 - n) * 8));
            self.emit(&[0x86]);
            if n != 0 {
                self.emit(&[0x84]);
            }
        }
    }
    pub(super) fn memory(
        &mut self,
        store: bool,
        data: Data,
        address: Register,
        oi: u32,
        pc: u32,
        layout: Option<&MemoryLayout>,
    ) -> Result<(), String> {
        let Some(l) = layout else {
            self.slow_memory(store, data, address, oi, pc)?;
            return Ok(());
        };
        let (memop, mode) = (oi >> 4, oi & 15);
        if mode >= l.modes {
            return Err("invalid TCI MMU index".into());
        }
        let shift = memop & l.size;
        if shift > 3 {
            return Err("unsupported TCI memory width".into());
        }
        let size = 1u32 << shift;
        let swapped = memop & l.swap != 0;
        let signed = memop & l.sign != 0;
        let alignment = memop & l.align_mask;
        let alignment = if alignment == l.unaligned {
            0
        } else if alignment == l.natural {
            shift
        } else {
            alignment >> l.align_shift
        };
        if alignment > 31 {
            return Err("unsupported TCI memory alignment".into());
        }
        let page_mask = u32::MAX << (l.page_bits);
        self.reg(address);
        self.set(ADDRESS);
        self.get(0);
        self.constant(l.tlb.wrapping_add(mode.wrapping_mul(l.stride)));
        self.emit(&[0x6a]);
        self.set(DESCRIPTOR);
        // Slow accesses can resize or replace the TLB during this function.
        // Its current mask and table must therefore be read for each access.
        self.get(DESCRIPTOR);
        self.mem(0x28, 0, l.table);
        self.get(ADDRESS);
        self.constant(l.page_bits);
        self.emit(&[0x76]);
        self.constant(l.entry_bits);
        self.emit(&[0x74]);
        self.get(DESCRIPTOR);
        self.mem(0x28, 0, 0);
        self.emit(&[0x71, 0x6a]);
        self.set(ENTRY);
        self.get(ENTRY);
        self.mem(0x28, 0, if store { l.write } else { l.read });
        self.get(ADDRESS);
        self.constant(page_mask);
        self.emit(&[0x71, 0x46]);
        if alignment != 0 {
            self.get(ADDRESS);
            self.constant((1u32 << alignment) - 1);
            self.emit(&[0x71, 0x45, 0x71]);
        }
        if size > 1 {
            self.get(ADDRESS);
            self.constant(!page_mask);
            self.emit(&[0x71]);
            self.constant((1u32 << l.page_bits) - size);
            self.emit(&[0x4d, 0x71]);
        }
        self.get(0);
        self.mem(0x28, 0, l.uc);
        self.mem(0x2d, 0, l.page_hooks);
        self.emit(&[0x04, 0x7f]);
        self.constant(1);
        self.emit(&[0x05]);
        for (n, offset) in [l.read_hook, l.write_hook, l.after_hook]
            .into_iter()
            .enumerate()
        {
            self.get(0);
            self.mem(0x28, 0, l.uc);
            self.mem(0x28, 0, offset);
            if n != 0 {
                self.emit(&[0x72]);
            }
        }
        self.emit(&[0x45, 0x0b, 0x71, 0x04, if store { 0x40 } else { 0x7e }]);
        self.get(ADDRESS);
        self.get(ENTRY);
        self.mem(0x28, 0, l.addend);
        self.emit(&[0x6a]);
        if store {
            match data {
                Data::Word(r) => {
                    self.reg(r);
                    self.emit(&[0xad]);
                }
                Data::Double(pair) => self.pair(pair.map(Value::Register)),
            }
            if swapped && size > 1 {
                self.reverse_bytes(size);
            }
            self.mem([0x3c, 0x3d, 0x3e, 0x37][shift as usize], 0, 0);
        } else {
            self.mem(
                if signed && !swapped {
                    [0x30, 0x32, 0x34, 0x29][shift as usize]
                } else {
                    [0x31, 0x33, 0x35, 0x29][shift as usize]
                },
                0,
                0,
            );
            if swapped {
                if size > 1 {
                    self.reverse_bytes(size);
                }
                if signed && size < 8 {
                    self.wide(u64::from(64 - size * 8));
                    self.emit(&[0x86]);
                    self.wide(u64::from(64 - size * 8));
                    self.emit(&[0x87]);
                }
            }
        }
        self.emit(&[0x05]);
        self.slow_memory(store, data, address, oi, pc)?;
        self.emit(&[0x0b]);
        Ok(())
    }
}
