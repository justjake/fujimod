//! Detach Python while retaining a single owner for thread-affine CPUs.
use fujimod_emulator::board::Board;
use fujimod_emulator_session::Session;
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
struct Owner<'a, T> {
    target: NonNull<T>,
    thread: ThreadId,
    _loan: PhantomData<&'a mut T>,
}
// Send transfers only an opaque, lifetime-bound capability. The only method
// that dereferences it checks its thread first; Drop never touches the board.
// This lets stable PyO3's Send-based Ungil bound express a synchronous detach
// without declaring the CPUs, their Rc handles, or the runtime itself Send.
unsafe impl<T> Send for Owner<'_, T> {}
impl<'a, T> Owner<'a, T> {
    fn new(target: &'a mut T) -> Self {
        Self {
            target: NonNull::from(target),
            thread: thread::current().id(),
            _loan: PhantomData,
        }
    }
    fn run(mut self, execute: impl FnOnce(&mut T) -> Result<(), String>) -> Result<(), String> {
        if self.thread != thread::current().id() {
            return Err("runtime execution moved from its owner thread".into());
        }
        execute(unsafe { self.target.as_mut() })
    }
}
fn run(py: Python<'_>, board: &mut Board, limit: Limit, quantum: usize) -> Result<(), String> {
    let owner = Owner::new(board);
    py.detach(move || {
        owner.run(|board| match limit {
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
        })
    })
}
fn run_session(
    py: Python<'_>,
    session: &mut Session,
    limit: Limit,
    quantum: usize,
) -> Result<(), String> {
    let owner = Owner::new(session);
    py.detach(move || {
        owner.run(|session| match limit {
            Limit::Tick(tick) => session.run_window(u64::MAX, quantum, None, Some(tick)),
            Limit::Budget {
                count,
                pause_after,
                until,
            } => session.run_with_limits(count, quantum, pause_after, until),
            Limit::Window {
                end,
                pause_after,
                until,
            } => session.run_window(end, quantum, pause_after, until),
        })
    })
}
pub fn session_until(
    py: Python<'_>,
    session: &mut Session,
    tick: u64,
    quantum: usize,
) -> Result<(), String> {
    run_session(py, session, Limit::Tick(tick), quantum)
}
pub fn board_until(
    py: Python<'_>,
    board: &mut Board,
    tick: u64,
    quantum: usize,
) -> Result<(), String> {
    run(py, board, Limit::Tick(tick), quantum)
}
pub fn session_budget(
    py: Python<'_>,
    session: &mut Session,
    count: u64,
    quantum: usize,
    pause_after: Option<u64>,
    until: Option<u64>,
) -> Result<(), String> {
    run_session(
        py,
        session,
        Limit::Budget {
            count,
            pause_after,
            until,
        },
        quantum,
    )
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
    session: &mut Session,
    end: u64,
    quantum: usize,
    pause_after: Option<u64>,
    until: Option<u64>,
) -> Result<(), String> {
    run_session(
        py,
        session,
        Limit::Window {
            end,
            pause_after,
            until,
        },
        quantum,
    )
}
