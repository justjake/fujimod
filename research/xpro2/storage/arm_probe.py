"""Minimal ARM runner for the offline storage probe."""

import struct
import unicorn as u
from unicorn.arm_const import (
    UC_ARM_REG_CPSR, UC_ARM_REG_LR, UC_ARM_REG_PC, UC_ARM_REG_SP,
    UC_ARM_REG_R0, UC_ARM_REG_R1, UC_ARM_REG_R2, UC_ARM_REG_R3,
    UC_ARM_REG_R4, UC_ARM_REG_R5, UC_ARM_REG_R6, UC_ARM_REG_R7,
    UC_ARM_REG_R8, UC_ARM_REG_R9, UC_ARM_REG_R10, UC_ARM_REG_R11, UC_ARM_REG_R12,
)

STACK, STOP = 0x4000000, 0x7f000000
REGS = (
    UC_ARM_REG_R0, UC_ARM_REG_R1, UC_ARM_REG_R2, UC_ARM_REG_R3,
    UC_ARM_REG_R4, UC_ARM_REG_R5, UC_ARM_REG_R6, UC_ARM_REG_R7,
    UC_ARM_REG_R8, UC_ARM_REG_R9, UC_ARM_REG_R10, UC_ARM_REG_R11,
    UC_ARM_REG_R12, UC_ARM_REG_LR,
)


def word(cpu, address, value):
    cpu.mem_write(address, struct.pack('<I', value))


def machine(stock):
    cpu = u.Uc(u.UC_ARCH_ARM, u.UC_MODE_ARM)
    cpu.mem_map(0, (len(stock) + 4095) & ~4095)
    cpu.mem_write(0, bytes(stock))
    cpu.mem_map(STACK, 0x10000)
    cpu.mem_map(STOP, 0x1000)
    return cpu


def call(cpu, entry, args):
    seeds = [0x55660000 + index for index in range(len(REGS))]
    seeds[:4] = list(args) + [0] * (4 - len(args))
    seeds[-1] = STOP
    for reg, seed in zip(REGS, seeds):
        cpu.reg_write(reg, seed)
    stack_top = STACK + 0xf000
    cpu.reg_write(UC_ARM_REG_CPSR, 0x1d3)
    cpu.reg_write(UC_ARM_REG_SP, stack_top)
    cpu.emu_start(entry, STOP, count=30_000)
    assert cpu.reg_read(UC_ARM_REG_PC) == STOP, f'{entry:#x} did not return'
    assert cpu.reg_read(UC_ARM_REG_SP) == stack_top, f'{entry:#x} changed the stack'
    assert [cpu.reg_read(reg) for reg in REGS[4:12]] == seeds[4:12], f'{entry:#x} changed a callee register'
    return [cpu.reg_read(reg) for reg in REGS[:4]]
