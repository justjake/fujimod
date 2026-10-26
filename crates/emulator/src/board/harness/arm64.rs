//! Isolated AArch64 routine calls on a caller-configured board.
//!
//! The caller supplies the routine, initial registers, return address, and
//! execution bound. This harness does not inspect firmware identity or alter
//! device behavior.

use super::super::{Board, observation::InstructionPoints};
use fujimod_emulator_cpu::{Model, Register};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    Returned,
    InstructionLimit,
}

#[derive(Clone, Debug)]
pub struct Options {
    pub core: usize,
    pub entry: u64,
    pub arguments: Vec<u64>,
    pub return_address: u64,
    pub stack_pointer: Option<u64>,
    pub pstate: Option<u64>,
    pub max_instructions: u64,
    pub quantum: usize,
}

#[derive(Clone, Debug)]
pub struct Result {
    pub stop: Stop,
    pub pc: u64,
    pub x0: u64,
    pub instructions: u64,
    pub tick: u64,
}

fn validate(options: &Options) -> std::result::Result<(), String> {
    if options.entry & 3 != 0
        || options.return_address & 3 != 0
        || options.arguments.len() > 8
        || options.max_instructions == 0
        || options.quantum == 0
    {
        return Err("invalid isolated AArch64 call options".into());
    }
    Ok(())
}

fn run(
    board: &mut Board,
    points: &mut InstructionPoints,
    options: &Options,
    start: u64,
) -> std::result::Result<Stop, String> {
    board.harness_select_core(options.core)?;
    for index in 0..30 {
        board.write_register(
            options.core,
            Register::X(index),
            options.arguments.get(index as usize).copied().unwrap_or(0),
        )?;
    }
    board.write_register(options.core, Register::X(30), options.return_address)?;
    if let Some(pstate) = options.pstate {
        board.write_register(options.core, Register::Pstate, pstate)?;
    }
    // SP is banked by PSTATE's EL0/EL1 stack selector. Select the caller's
    // exception level before writing SP so the requested value lands in the
    // bank the routine will actually use.
    if let Some(stack_pointer) = options.stack_pointer {
        board.write_register(options.core, Register::Sp, stack_pointer)?;
    }
    board.write_register(options.core, Register::Pc, options.entry)?;

    loop {
        if points.event().is_some() {
            return Ok(Stop::Returned);
        }
        let executed = board
            .instruction_counts()
            .get(options.core)
            .copied()
            .ok_or("isolated AArch64 call core disappeared")?
            .saturating_sub(start);
        if executed >= options.max_instructions {
            return Ok(Stop::InstructionLimit);
        }
        let remaining = options.max_instructions - executed;
        board.run_for_instructions(remaining, options.quantum)?;
    }
}

/// Call one caller-selected routine, stopping before its caller-selected
/// return address or after the caller-selected instruction budget.
///
/// Strict boards still refuse unsupported device accesses through their
/// normal bus. Discovery mode remains an explicit property of the board.
pub fn call(board: &mut Board, options: Options) -> std::result::Result<Result, String> {
    validate(&options)?;
    if board.cpu(options.core)?.model() != Model::ArmV8A {
        return Err("isolated AArch64 calls require an ArmV8A processor".into());
    }
    let start = board.instruction_counts()[options.core];
    let mut points = board.instruction_points_begin(options.core, vec![options.return_address])?;
    let execution = run(board, &mut points, &options, start);
    let cleanup = board.instruction_points_end(options.core, &mut points);
    let stop = match (execution, cleanup) {
        (Ok(stop), Ok(())) => stop,
        (Err(error), Ok(())) => return Err(error),
        (Ok(_), Err(error)) => return Err(error),
        (Err(error), Err(cleanup)) => return Err(format!("{error}; cleanup: {cleanup}")),
    };
    Ok(Result {
        stop,
        pc: board.read_register(options.core, Register::Pc)?,
        x0: board.read_register(options.core, Register::X(0))?,
        instructions: board.instruction_counts()[options.core].saturating_sub(start),
        tick: board.now(),
    })
}
