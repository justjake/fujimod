//! Versioned layouts supplied by the pinned engine, independent of host objects.
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug)]
pub struct MemoryLayout {
    pub(super) page_bits: u32,
    pub(super) tlb: u32,
    pub(super) stride: u32,
    pub(super) table: u32,
    pub(super) entry_bits: u32,
    pub(super) read: u32,
    pub(super) write: u32,
    pub(super) addend: u32,
    pub(super) uc: u32,
    pub(super) page_hooks: u32,
    pub(super) read_hook: u32,
    pub(super) write_hook: u32,
    pub(super) after_hook: u32,
    pub(super) size: u32,
    pub(super) sign: u32,
    pub(super) swap: u32,
    pub(super) align_shift: u32,
    pub(super) align_mask: u32,
    pub(super) unaligned: u32,
    pub(super) natural: u32,
    pub(super) modes: u32,
}
impl MemoryLayout {
    pub fn new(raw: &[u32]) -> Result<Self, String> {
        let &[
            1,
            page_bits,
            tlb,
            stride,
            table,
            entry_bits,
            read,
            write,
            addend,
            uc,
            page_hooks,
            read_hook,
            write_hook,
            after_hook,
            size,
            sign,
            swap,
            align_shift,
            align_mask,
            unaligned,
            natural,
            modes,
        ] = raw
        else {
            return Err("invalid TCI memory layout".into());
        };
        if !(10..=16).contains(&page_bits)
            || entry_bits > 31
            || size > 3
            || align_shift > 31
            || !(1..=16).contains(&modes)
        {
            return Err("invalid TCI memory layout fields".into());
        }
        Ok(Self {
            page_bits,
            tlb,
            stride,
            table,
            entry_bits,
            read,
            write,
            addend,
            uc,
            page_hooks,
            read_hook,
            write_hook,
            after_hook,
            size,
            sign,
            swap,
            align_shift,
            align_mask,
            unaligned,
            natural,
            modes,
        })
    }
}
#[derive(Clone, Copy, Debug)]
pub struct HelperLayout {
    pub(super) pc: u32,
    pub(super) uc: u32,
    pub(super) pending: u32,
    pub(super) carry: u32,
    pub(super) retry: u32,
    pub(super) cpu: u32,
    pub(super) decrement: u32,
}
impl HelperLayout {
    pub fn new(raw: &[u32]) -> Result<Self, String> {
        let &[1, pc, uc, pending, carry, retry, cpu, decrement] = raw else {
            return Err("invalid TCI helper layout".into());
        };
        Ok(Self {
            pc,
            uc,
            pending,
            carry,
            retry,
            cpu,
            decrement,
        })
    }
}
#[derive(Clone, Copy, Debug)]
pub enum HelperKind {
    Other,
    Trace,
    CheckExit,
    ShlCarry,
    ShrCarry,
    SarCarry,
    RorCarry,
}
impl HelperKind {
    pub fn named(name: &str) -> Option<Self> {
        Some(match name {
            "" => return None,
            "uc_tracecode" => Self::Trace,
            "check_exit_request_arm" => Self::CheckExit,
            "shl_cc" => Self::ShlCarry,
            "shr_cc" => Self::ShrCarry,
            "sar_cc" => Self::SarCarry,
            "ror_cc" => Self::RorCarry,
            _ => Self::Other,
        })
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Helper {
    pub kind: HelperKind,
    /// Original typed engine function, distinct from its uniform TCI wrapper.
    pub target: u32,
}
#[derive(Default, Debug)]
pub struct Environment {
    pub memory: Option<MemoryLayout>,
    pub helper: Option<HelperLayout>,
    pub helpers: BTreeMap<u32, Helper>,
}
