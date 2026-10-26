//! ARMv7-M basic exception frames, independent of the execution backend.
//! NVIC state supplies IPSR because Unicorn exposes the architectural MRS/MSR
//! view rather than a host setter for the processor's active exception.
pub mod nvic;
use fujimod_emulator_cpu::{Cpu, Register, arm};
use nvic::Nvic;
fn r(n: u8) -> Register {
    Register::Arm32(arm::Register::R(n))
}
fn special(r: arm::Register) -> Register {
    Register::Arm32(r)
}
fn get(cpu: &dyn Cpu, register: Register) -> Result<u32, String> {
    let mut bytes = [0; 4];
    if cpu.read_register(register, &mut bytes)? != 4 {
        return Err("M-profile register is not 32 bits".into());
    }
    Ok(u32::from_le_bytes(bytes))
}
fn set(cpu: &dyn Cpu, register: Register, value: u32) -> Result<(), String> {
    cpu.write_register(register, &value.to_le_bytes())
}
pub fn exception_status(cpu: &dyn Cpu, nvic: &Nvic) -> Result<u32, String> {
    Ok((get(cpu, special(arm::Register::Cpsr))? & 0xfe0ffc00)
        | (1 << 24)
        | nvic.active.last().copied().unwrap_or(0) as u32)
}
pub fn enter(cpu: &dyn Cpu, nvic: &mut Nvic, exception: u16, pc: u32) -> Result<(), String> {
    if !(2..272).contains(&exception) {
        return Err("M-profile exception outside supported NVIC".into());
    }
    let control = get(cpu, special(arm::Register::Control))?;
    if control & 4 != 0 {
        return Err("M-profile extended FP exception frames are unsupported".into());
    }
    let vector_address = nvic
        .vtor
        .checked_add(exception as u32 * 4)
        .ok_or("exception vector address overflow")?;
    let mut vector = [0; 4];
    cpu.read_memory(vector_address as u64, &mut vector)?;
    let vector = u32::from_le_bytes(vector);
    if vector & 1 == 0 {
        return Err(format!(
            "non-Thumb exception {exception} vector {vector:#x}"
        ));
    }
    let thread = nvic.active.is_empty();
    let psp = thread && control & 2 != 0;
    let stack = special(if psp {
        arm::Register::Psp
    } else {
        arm::Register::Msp
    });
    let sp = get(cpu, stack)?;
    let align = sp & 4 != 0 && nvic.regs.get(&0xd14).copied().unwrap_or(0) & 0x200 != 0;
    let base = sp.checked_sub(32).ok_or("exception stack underflow")? & if align { !7 } else { !3 };
    let frame = [
        get(cpu, r(0))?,
        get(cpu, r(1))?,
        get(cpu, r(2))?,
        get(cpu, r(3))?,
        get(cpu, r(12))?,
        get(cpu, r(14))?,
        pc,
        exception_status(cpu, nvic)? | ((align as u32) << 9),
    ];
    let bytes: Vec<_> = frame.into_iter().flat_map(u32::to_le_bytes).collect();
    cpu.write_memory(base as u64, &bytes)?;
    set(cpu, stack, base)?;
    set(
        cpu,
        special(arm::Register::Cpsr),
        get(cpu, special(arm::Register::Cpsr))? & !0x0600fc00,
    )?;
    set(cpu, special(arm::Register::Control), control & !2)?;
    set(cpu, r(13), get(cpu, special(arm::Register::Msp))?)?;
    set(
        cpu,
        r(14),
        if !thread {
            0xfffffff1
        } else if psp {
            0xfffffffd
        } else {
            0xfffffff9
        },
    )?;
    set(cpu, Register::Pc, vector)?;
    nvic.enter(exception);
    Ok(())
}
pub fn exception_return(cpu: &dyn Cpu, nvic: &mut Nvic, ret: u32) -> Result<(), String> {
    if !matches!(ret, 0xfffffff1 | 0xfffffff9 | 0xfffffffd) {
        return Err(format!(
            "unsupported EXC_RETURN {ret:#x}; only basic frames are implemented"
        ));
    }
    if nvic.active.is_empty() {
        return Err("EXC_RETURN without an active exception".into());
    }
    let stack = special(if ret & 4 != 0 {
        arm::Register::Psp
    } else {
        arm::Register::Msp
    });
    let sp = get(cpu, stack)?;
    let mut bytes = [0; 32];
    cpu.read_memory(sp as u64, &mut bytes)?;
    let frame: [u32; 8] =
        std::array::from_fn(|i| u32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap()));
    if frame[7] & (1 << 24) == 0 {
        return Err("EXC_RETURN requires a Thumb frame".into());
    }
    let thread = ret & 8 != 0;
    let previous = nvic.active.iter().rev().nth(1).copied().unwrap_or(0) as u32;
    if (thread && nvic.active.len() != 1)
        || ((frame[7] & 0x1ff) != if thread { 0 } else { previous })
        || (!thread && previous == 0)
    {
        return Err("EXC_RETURN frame does not match active exception nesting".into());
    }
    let next_sp = sp
        .checked_add(32 + if frame[7] & 0x200 != 0 { 4 } else { 0 })
        .ok_or("exception return stack overflow")?;
    for (reg, value) in [0, 1, 2, 3, 12, 14].into_iter().zip(frame) {
        set(cpu, r(reg), value)?;
    }
    let cpsr = get(cpu, special(arm::Register::Cpsr))?;
    set(
        cpu,
        special(arm::Register::Cpsr),
        (cpsr & !0xfe0ffc00) | (frame[7] & 0xfe0ffc00) | 0x20,
    )?;
    set(cpu, stack, next_sp)?;
    let control = get(cpu, special(arm::Register::Control))?;
    set(
        cpu,
        special(arm::Register::Control),
        (control & !2) | if ret & 4 != 0 { 2 } else { 0 },
    )?;
    set(cpu, Register::Pc, frame[6] | 1)?;
    nvic.active.pop();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_cpu::{Model, Run};
    use fujimod_emulator_cpu_unicorn::Library;
    fn library() -> Library {
        Library::linked().unwrap()
    }
    #[test]
    fn real_m4_executes_handler_and_restores_aligned_psp_and_nested_msp_frames() {
        let cpu = library().open(Model::CortexM4, 1024 * 1024).unwrap();
        cpu.map_zeroed(0, 0x4000, 7).unwrap();
        cpu.write_memory(71 * 4, &0x301u32.to_le_bytes()).unwrap();
        cpu.write_memory(74 * 4, &0x401u32.to_le_bytes()).unwrap();
        cpu.write_memory(0x300, &[1, 0x30]).unwrap(); // adds r0,#1
        let mut nvic = Nvic::new(600_000_000);
        nvic.regs.insert(0xd14, 0x200);
        for (reg, value) in [
            (r(0), 11),
            (r(1), 22),
            (r(2), 33),
            (r(3), 44),
            (r(12), 55),
            (r(14), 66),
            (special(arm::Register::Msp), 0x3000),
            (special(arm::Register::Psp), 0x2004),
            (special(arm::Register::Control), 2),
            (special(arm::Register::Cpsr), 0xa8000020),
        ] {
            set(&cpu, reg, value).unwrap();
        }
        enter(&cpu, &mut nvic, 71, 0x1234).unwrap();
        assert_eq!(get(&cpu, r(14)).unwrap(), 0xfffffffd);
        assert_eq!(get(&cpu, special(arm::Register::Psp)).unwrap(), 0x1fe0);
        assert_eq!(get(&cpu, Register::Sp).unwrap(), 0x3000);
        let mut frame = [0; 32];
        cpu.read_memory(0x1fe0, &mut frame).unwrap();
        assert_eq!(
            u32::from_le_bytes(frame[24..28].try_into().unwrap()),
            0x1234
        );
        assert_ne!(
            u32::from_le_bytes(frame[28..32].try_into().unwrap()) & (1 << 9),
            0
        );
        cpu.run(Run {
            begin: 0x301,
            until: None,
            watchdog_us: 0,
            instructions: Some(1),
        })
        .unwrap();
        assert_eq!(get(&cpu, r(0)).unwrap(), 12);
        enter(&cpu, &mut nvic, 74, 0x302).unwrap();
        assert_eq!(get(&cpu, r(14)).unwrap(), 0xfffffff1);
        assert_eq!(nvic.active, vec![71, 74]);
        assert_eq!(exception_status(&cpu, &nvic).unwrap() & 511, 74);
        exception_return(&cpu, &mut nvic, 0xfffffff1).unwrap();
        assert_eq!(nvic.active, vec![71]);
        assert_eq!(get(&cpu, Register::Pc).unwrap(), 0x302);
        assert_eq!(get(&cpu, special(arm::Register::Msp)).unwrap(), 0x3000);
        assert_eq!(get(&cpu, r(14)).unwrap(), 0xfffffffd);
        exception_return(&cpu, &mut nvic, 0xfffffffd).unwrap();
        assert!(nvic.active.is_empty());
        assert_eq!(get(&cpu, Register::Pc).unwrap(), 0x1234);
        assert_eq!(get(&cpu, r(0)).unwrap(), 11);
        assert_eq!(get(&cpu, special(arm::Register::Psp)).unwrap(), 0x2004);
        assert_eq!(get(&cpu, Register::Sp).unwrap(), 0x2004);
        assert_eq!(
            exception_status(&cpu, &nvic).unwrap() & 0xf8000000,
            0xa8000000
        );
    }
    #[test]
    fn invalid_vector_and_frame_refuse_before_changing_cpu_or_stack() {
        let cpu = library().open(Model::CortexM4, 1024 * 1024).unwrap();
        cpu.map_zeroed(0, 0x4000, 7).unwrap();
        set(&cpu, special(arm::Register::Msp), 0x3000).unwrap();
        set(&cpu, Register::Pc, 0x1001).unwrap();
        let mut nvic = Nvic::new(600_000_000);
        assert!(
            enter(&cpu, &mut nvic, 71, 0x1000)
                .unwrap_err()
                .contains("non-Thumb")
        );
        assert_eq!(get(&cpu, special(arm::Register::Msp)).unwrap(), 0x3000);
        assert_eq!(get(&cpu, Register::Pc).unwrap(), 0x1000);
        assert!(nvic.active.is_empty());
        assert!(exception_return(&cpu, &mut nvic, 0xffffffe9).is_err());
        assert!(
            exception_return(&cpu, &mut nvic, 0xfffffff9)
                .unwrap_err()
                .contains("without an active")
        );
        let mut stack = [0; 32];
        cpu.read_memory(0x2fe0, &mut stack).unwrap();
        assert_eq!(stack, [0; 32]);
    }
}
