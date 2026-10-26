//! Thumb conditional execution state shared by ARM profiles.
pub mod exclusive;

/// CPSR encodes ITSTATE in bits26:25 and15:10. Preserve flags and other state.
pub fn advance_it(cpsr: u32) -> u32 {
    let it = (((cpsr >> 25) & 3) | ((cpsr >> 8) & 0xfc)) as u8;
    let next = if it & 7 == 0 {
        0
    } else {
        (it & 0xe0) | ((it << 1) & 0x1f)
    };
    (cpsr & !0x0600_fc00) | ((next as u32 & 3) << 25) | ((next as u32 & 0xfc) << 8)
}
pub fn condition_passed(cpsr: u32) -> bool {
    let it = (((cpsr >> 25) & 3) | ((cpsr >> 8) & 0xfc)) as u8;
    if it == 0 {
        return true;
    }
    let (n, z, c, v) = (
        cpsr >> 31 != 0,
        cpsr & (1 << 30) != 0,
        cpsr & (1 << 29) != 0,
        cpsr & (1 << 28) != 0,
    );
    let cond = it >> 4;
    let yes = match cond >> 1 {
        0 => z,
        1 => c,
        2 => n,
        3 => v,
        4 => c && !z,
        5 => n == v,
        6 => n == v && !z,
        _ => true,
    };
    if cond & 1 != 0 && cond != 15 {
        !yes
    } else {
        yes
    }
}
