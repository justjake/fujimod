//! Detach Python while retaining a single owner for thread-affine CPUs.
use fujimod_emulator::board::Board;
use pyo3::Python;
use std::{
    marker::PhantomData,
    ptr::NonNull,
    thread::{self, ThreadId},
};

enum Limit {
    Tick(u64),
    Budget {
        count: u64,
        pause_after: Option<u64>,
        until: Option<u64>,
    },
    Window {
        end: u64,
        pause_after: Option<u64>,
        until: Option<u64>,
    },
}
struct Owner<'a> {
    board: NonNull<Board>,
    thread: ThreadId,
    _loan: PhantomData<&'a mut Board>,
}
// Send transfers only an opaque, lifetime-bound capability. The only method
// that dereferences it checks its thread first; Drop never touches the board.
// This lets stable PyO3's Send-based Ungil bound express a synchronous detach
// without declaring the CPUs, their Rc handles, or the runtime itself Send.
unsafe impl Send for Owner<'_> {}
impl<'a> Owner<'a> {
    fn new(board: &'a mut Board) -> Self {
        Self {
            board: NonNull::from(board),
            thread: thread::current().id(),
            _loan: PhantomData,
        }
    }
    fn run(mut self, limit: Limit, quantum: usize) -> Result<(), String> {
        if self.thread != thread::current().id() {
            return Err("runtime execution moved from its owner thread".into());
        }
        let board = unsafe { self.board.as_mut() };
        match limit {
            Limit::Tick(tick) => board.run_until(tick, quantum),
            Limit::Budget {
                count,
                pause_after,
                until,
            } => board.run_with_limits(count, quantum, pause_after, until),
            Limit::Window {
                end,
                pause_after,
                until,
            } => board.run_window(end, quantum, pause_after, until),
        }
    }
}
fn run(py: Python<'_>, board: &mut Board, limit: Limit, quantum: usize) -> Result<(), String> {
    let owner = Owner::new(board);
    py.detach(move || owner.run(limit, quantum))
}
pub fn until(py: Python<'_>, board: &mut Board, tick: u64, quantum: usize) -> Result<(), String> {
    run(py, board, Limit::Tick(tick), quantum)
}
pub fn instructions(
    py: Python<'_>,
    board: &mut Board,
    count: u64,
    quantum: usize,
) -> Result<(), String> {
    budget(py, board, count, quantum, None, None)
}

pub fn budget(
    py: Python<'_>,
    board: &mut Board,
    count: u64,
    quantum: usize,
    pause_after: Option<u64>,
    until: Option<u64>,
) -> Result<(), String> {
    run(
        py,
        board,
        Limit::Budget {
            count,
            pause_after,
            until,
        },
        quantum,
    )
}

pub fn window(
    py: Python<'_>,
    board: &mut Board,
    end: u64,
    quantum: usize,
    pause_after: Option<u64>,
    until: Option<u64>,
) -> Result<(), String> {
    run(
        py,
        board,
        Limit::Window {
            end,
            pause_after,
            until,
        },
        quantum,
    )
}
