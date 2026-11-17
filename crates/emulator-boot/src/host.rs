//! Host timing and observation around one shared session scheduler window.
use fujimod_emulator_session::Session;
use std::time::Duration;

pub(crate) fn duration_ticks(duration: Duration, ticks_per_second: u64) -> u64 {
    (duration.as_nanos().saturating_mul(u128::from(ticks_per_second)) / 1_000_000_000)
        .min(u128::from(u64::MAX)) as u64
}

#[derive(Clone, Copy)]
pub(crate) struct Limits {
    pub instructions: u64,
    pub tick: Option<u64>,
}

impl Limits {
    pub fn new(
        session: &Session,
        instructions: Option<u64>,
        duration: Option<Duration>,
        ticks_per_second: u64,
    ) -> Result<Self, String> {
        let instructions = match instructions {
            Some(0) => return Err("instruction limit must be positive".into()),
            Some(limit) => session.board.instruction_counts().into_iter()
                .try_fold(limit, u64::checked_add).ok_or("instruction limit overflows")?,
            None => u64::MAX,
        };
        let tick = duration.map(|duration| {
            session.board.now().saturating_add(duration_ticks(duration, ticks_per_second))
        });
        Ok(Self { instructions, tick })
    }

    pub fn reached(&self, session: &Session) -> bool {
        self.tick.is_some_and(|tick| session.board.now() >= tick)
            || session.board.instruction_counts().into_iter().sum::<u64>() >= self.instructions
    }

    pub fn step(
        self,
        session: &mut Session,
        quantum: usize,
        pause_after: Option<u64>,
        until: Option<u64>,
    ) -> Result<bool, String> {
        let until = self.tick.into_iter().chain(until).min();
        let before_tick = session.board.now();
        let before_instructions = session.board.instruction_counts();
        session.run_window(self.instructions, quantum, pause_after, until)?;
        Ok(before_tick != session.board.now()
            || before_instructions != session.board.instruction_counts())
    }
}
