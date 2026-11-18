//! Host timing and observation around one shared session scheduler window.
use fujimod_emulator_session::Session;
use std::time::Duration;

pub fn duration_ticks(duration: Duration, ticks_per_second: u64) -> u64 {
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
        Self::from_boundary(session.board.now(), &session.board.instruction_counts(),
            instructions, duration, ticks_per_second)
    }

    pub fn from_boundary(
        now: u64,
        counts: &[u64],
        instructions: Option<u64>,
        duration: Option<Duration>,
        ticks_per_second: u64,
    ) -> Result<Self, String> {
        let instructions = match instructions {
            Some(0) => return Err("instruction limit must be positive".into()),
            Some(limit) => counts.iter().copied().try_fold(limit, u64::checked_add)
                .ok_or("instruction limit overflows")?,
            None => u64::MAX,
        };
        let tick = duration.map(|duration| now.saturating_add(duration_ticks(duration, ticks_per_second)));
        Ok(Self { instructions, tick })
    }

    pub fn reached(&self, session: &Session) -> bool {
        self.reached_at(session.board.now(), session.board.instruction_counts().into_iter().sum())
    }

    pub fn reached_at(&self, now: u64, instructions: u64) -> bool {
        self.tick.is_some_and(|tick| now >= tick) || instructions >= self.instructions
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

pub(crate) fn parse_number(s: &str) -> Result<u64, String> {
    if let Some(s) = s.strip_prefix("0x") {
        u64::from_str_radix(s, 16).map_err(|e| e.to_string())
    } else {
        s.parse()
            .map_err(|e: std::num::ParseIntError| e.to_string())
    }
}
/// Instructions per scheduler turn for `--quantum`; replaces the run
/// configuration's `execution.quantum` for this run only.
pub fn parse_quantum(s: &str) -> Result<usize, String> {
    let value = usize::try_from(parse_number(s)?).map_err(|error| error.to_string())?;
    if value == 0 {
        return Err("--quantum needs a positive instruction count".into());
    }
    Ok(value)
}
pub fn parse_seconds(s: &str) -> Result<Duration, String> {
    let n = s.parse::<f64>().map_err(|e| e.to_string())?;
    Duration::try_from_secs_f64(n).map_err(|e| e.to_string())
}

pub fn parse_stub(specification: &str) -> Result<fujimod_emulator::board::discovery::CalleeStub, String> {
    let (name, rest) = specification
        .split_once('@')
        .ok_or("--stub needs NAME@ADDR=VALUE")?;
    let (address, value) = rest.split_once('=').ok_or("--stub needs NAME@ADDR=VALUE")?;
    if name.is_empty() {
        return Err("discovery callee stub name is empty".into());
    }
    Ok(fujimod_emulator::board::discovery::CalleeStub {
        name: name.to_owned(),
        core: 0,
        address: parse_number(address)?,
        value: parse_number(value)?,
    })
}

/// A physical key transition relative to the current run.
pub struct KeyEdge {
    pub delay: Duration,
    pub name: String,
    pub pressed: bool,
}

pub fn parse_keys(script: Option<&str>, presses: Option<&str>) -> Result<Vec<KeyEdge>, String> {
    let mut edges = Vec::new();
    for entry in script.unwrap_or_default().split(';').map(str::trim).filter(|entry| !entry.is_empty()) {
        let (time, name) = entry.split_once(':').ok_or("--keys needs t=SECONDS:KEY")?;
        let delay = parse_seconds(time.strip_prefix("t=").ok_or("--keys needs t=SECONDS:KEY")?)?;
        let (name, pressed) = name.strip_prefix('-').map_or((name, true), |name| (name, false));
        edges.push(KeyEdge { delay, name: key_name(name)?, pressed });
    }
    for entry in presses.into_iter().flat_map(|presses| presses.split(',')) {
        let (time, rest) = entry.trim().split_once(':').ok_or("--press needs TIME_MS:KEY[:HOLD_MS]")?;
        let (name, hold) = match rest.split_once(':') {
            Some((name, hold)) => (name, parse_number(hold)?),
            None => (rest, 150),
        };
        if hold == 0 {
            return Err("--press hold must be positive".into());
        }
        let name = key_name(name)?;
        let start = parse_number(time)?;
        let end = start.checked_add(hold).ok_or("--press time overflows")?;
        edges.push(KeyEdge { delay: Duration::from_millis(start), name: name.clone(), pressed: true });
        edges.push(KeyEdge { delay: Duration::from_millis(end), name, pressed: false });
    }
    edges.sort_by_key(|edge| (edge.delay, edge.pressed));
    Ok(edges)
}

fn key_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("empty input key name".into());
    }
    Ok(name.to_owned())
}

pub fn parse_gpio(spec: &str) -> Result<(u8, u8, bool), String> {
    let (pin, level) = spec.split_once('=').ok_or("--gpio needs PORT:PIN=0|1")?;
    let (port, pin) = pin.split_once(':').ok_or("--gpio needs PORT:PIN=0|1")?;
    let port = port.parse().map_err(|_| "invalid GPIO port")?;
    let pin = pin.parse().map_err(|_| "invalid GPIO pin")?;
    let high = match level {
        "0" => false,
        "1" => true,
        _ => return Err("GPIO level must be 0 or 1".into()),
    };
    Ok((port, pin, high))
}

pub fn parse_poke(spec: &str) -> Result<(u64, u32), String> {
    let (address, value) = spec.split_once('=').ok_or("--poke needs ADDR=U32")?;
    let address = parse_number(address)?;
    let value = u32::try_from(parse_number(value)?).map_err(|_| "--poke value exceeds u32")?;
    if address & 3 != 0 {
        return Err("--poke address needs 4-byte alignment".into());
    }
    Ok((address, value))
}
