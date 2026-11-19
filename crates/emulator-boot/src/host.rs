//! Host timing and observation around one shared session scheduler window.
use fujimod_emulator_session::Session;
use std::time::{Duration, Instant};

/// Host inspection and verified writes to modeled physical memory.
pub trait Memory {
    fn read(&self, address: u64, data: &mut [u8]) -> Result<(), String>;
    fn write(&self, address: u64, data: &[u8]) -> Result<(), String>;
}

impl Memory for fujimod_emulator::board::Board {
    fn read(&self, address: u64, data: &mut [u8]) -> Result<(), String> {
        self.inspect_physical(address, data)
    }

    fn write(&self, address: u64, data: &[u8]) -> Result<(), String> {
        self.write_physical(address, data)
    }
}

pub fn read_scene(path: &std::path::Path) -> Result<serde_json::Value, String> {
    serde_json::from_slice(&crate::read(path, 64 << 20)?)
        .map_err(|error| format!("{}: {error}", path.display()))
}

pub fn set_exr2_scene(
    resolved: &mut fujimod_catalog_resolution::ResolvedConfiguration,
    path: &std::path::Path,
) -> Result<(), String> {
    use fujimod_catalog_configuration::RunScene;
    let scene: RunScene = serde_json::from_value(read_scene(path)?)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if !matches!(scene, RunScene::Exr2(_)) {
        return Err("selected board requires an EXR II scene".into());
    }
    scene.validate()?;
    resolved.configuration.scene = Some(scene);
    Ok(())
}

pub fn duration_ticks(duration: Duration, ticks_per_second: u64) -> u64 {
    (duration.as_nanos().saturating_mul(u128::from(ticks_per_second)) / 1_000_000_000)
        .min(u128::from(u64::MAX)) as u64
}

/// The scheduler boundary used by native host workflows.
pub(crate) trait Execution {
    fn progress(&self) -> (Vec<u64>, u64);
    fn run_window(&mut self, end: u64, quantum: usize, pause_after: Option<u64>, until: Option<u64>) -> Result<(), String>;
}

impl Execution for Session {
    fn progress(&self) -> (Vec<u64>, u64) {
        (self.board.instruction_counts(), self.board.now())
    }

    fn run_window(&mut self, end: u64, quantum: usize, pause_after: Option<u64>, until: Option<u64>) -> Result<(), String> {
        Session::run_window(self, end, quantum, pause_after, until)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Limits {
    pub instructions: u64,
    pub tick: Option<u64>,
    /// Host wall-clock deadline. The other budgets count guest time or progress, which can advance
    /// far slower than real time while a core polls a device, so only this one bounds a run's wait.
    pub wall: Option<Instant>,
}

/// Why a driven run ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StopReason {
    /// The guest-time budget (`--seconds`) elapsed.
    GuestTime,
    /// The instruction budget (`--instruction-limit`) was spent.
    Instructions,
    /// The host wall-clock budget (`--wall-seconds`) elapsed.
    WallClock,
    /// A caller stop (an interrupt) ended the run, or the board went quiescent.
    Other,
}

impl StopReason {
    pub fn name(self) -> &'static str {
        match self {
            Self::GuestTime => "guest-time-limit",
            Self::Instructions => "instruction-limit",
            Self::WallClock => "wall-clock-limit",
            Self::Other => "stopped",
        }
    }
}

impl Limits {
    pub fn new(
        session: &impl Execution,
        instructions: Option<u64>,
        duration: Option<Duration>,
        ticks_per_second: u64,
    ) -> Result<Self, String> {
        let (counts, now) = session.progress();
        Self::from_boundary(now, &counts, instructions, duration, ticks_per_second)
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
        Ok(Self { instructions, tick, wall: None })
    }

    /// Also end the run once `wall` of host time has passed.
    pub fn with_wall(mut self, wall: Option<Duration>) -> Self {
        self.wall = wall.and_then(|wall| Instant::now().checked_add(wall));
        self
    }

    pub fn reached(&self, session: &impl Execution) -> bool {
        self.stop_reason(session).is_some()
    }

    /// The budget that is spent, if any.
    pub fn stop_reason(&self, session: &impl Execution) -> Option<StopReason> {
        let (counts, now) = session.progress();
        if self.tick.is_some_and(|tick| now >= tick) {
            Some(StopReason::GuestTime)
        } else if counts.into_iter().sum::<u64>() >= self.instructions {
            Some(StopReason::Instructions)
        } else if self.wall.is_some_and(|wall| Instant::now() >= wall) {
            Some(StopReason::WallClock)
        } else {
            None
        }
    }

    pub fn step(
        self,
        session: &mut impl Execution,
        quantum: usize,
        pause_after: Option<u64>,
        until: Option<u64>,
    ) -> Result<bool, String> {
        let until = self.tick.into_iter().chain(until).min();
        let before = session.progress();
        session.run_window(self.instructions, quantum, pause_after, until)?;
        Ok(before != session.progress())
    }

    /// Run until a budget, a caller stop, or quiescence. Before and after
    /// callbacks always see the same stopped machine boundary.
    pub fn drive<E: Execution>(
        self,
        session: &mut E,
        quantum: usize,
        mut before: impl FnMut(&mut E) -> Result<std::ops::ControlFlow<(), Option<u64>>, String>,
        mut after: impl FnMut(&mut E, &Result<bool, String>) -> Result<(), String>,
    ) -> Result<(), String> {
        while !self.reached(session) {
            let std::ops::ControlFlow::Continue(until) = before(session)? else { break; };
            let result = self.step(session, quantum, None, until);
            after(session, &result)?;
            if !result? {
                break;
            }
        }
        Ok(())
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

/// A dial or switch position change `MS` guest milliseconds after the run
/// starts (after a snapshot load, like `--press`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PositionChange {
    pub at_ms: u64,
    pub control: String,
    pub position: String,
}

/// Parse `--set CONTROL=POSITION[@MS][,...]`. Without `@MS` the change applies
/// when the run starts. Names are checked later against the body's dials.
pub fn parse_settings(spec: &str) -> Result<Vec<PositionChange>, String> {
    let mut changes = Vec::new();
    for item in spec.split(',').map(str::trim).filter(|item| !item.is_empty()) {
        let (setting, at_ms) = match item.rsplit_once('@') {
            Some((setting, at)) => (
                setting,
                at.trim()
                    .parse()
                    .map_err(|_| format!("--set {item}: the time after @ is whole milliseconds"))?,
            ),
            None => (item, 0),
        };
        let (control, position) = setting
            .split_once('=')
            .ok_or_else(|| format!("--set {item}: needs CONTROL=POSITION[@MS]"))?;
        let (control, position) = (control.trim(), position.trim());
        if control.is_empty() || position.is_empty() {
            return Err(format!("--set {item}: needs CONTROL=POSITION[@MS]"));
        }
        changes.push(PositionChange { at_ms, control: control.to_owned(), position: position.to_owned() });
    }
    // A stable sort keeps the written order of changes that share a time.
    changes.sort_by_key(|change| change.at_ms);
    Ok(changes)
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
    if spec.contains('@') {
        return Err(format!("--poke {spec}: the timed form ADDR=U32@MS needs an XP5 run"));
    }
    let (address, value) = spec.split_once('=').ok_or("--poke needs ADDR=U32")?;
    let address = parse_number(address)?;
    let value = u32::try_from(parse_number(value)?).map_err(|_| "--poke value exceeds u32")?;
    if address & 3 != 0 {
        return Err("--poke address needs 4-byte alignment".into());
    }
    Ok((address, value))
}

/// `ADDR=U32` (applied before the run) or `ADDR=U32@MS` (applied MS milliseconds of guest time
/// after the run starts, after a snapshot load). Returns the time, if any, and the write.
pub fn parse_timed_poke(spec: &str) -> Result<(Option<u64>, u64, u32), String> {
    match spec.split_once('@') {
        Some((write, at)) => {
            let at_ms = at.trim().parse()
                .map_err(|_| format!("--poke {spec}: the time after @ is whole milliseconds"))?;
            let (address, value) = parse_poke(write.trim())?;
            Ok((Some(at_ms), address, value))
        }
        None => {
            let (address, value) = parse_poke(spec.trim())?;
            Ok((None, address, value))
        }
    }
}

#[cfg(test)]
mod poke_tests {
    use super::*;

    #[test]
    fn position_settings_take_an_optional_time() {
        let change = |at_ms, control: &str, position: &str| PositionChange {
            at_ms,
            control: control.into(),
            position: position.into(),
        };
        assert_eq!(
            parse_settings("iso=800@1500, ss=1/250@500,ev=-2/3,power=OFF@9000").unwrap(),
            [
                change(0, "ev", "-2/3"),
                change(500, "ss", "1/250"),
                change(1500, "iso", "800"),
                change(9000, "power", "OFF"),
            ]
        );
        assert_eq!(parse_settings("").unwrap(), []);
        assert!(parse_settings("iso").is_err());
        assert!(parse_settings("iso=@10").is_err());
        assert!(parse_settings("=800").is_err());
        assert!(parse_settings("iso=800@1.5").is_err());
        assert!(parse_settings("iso=800@").is_err());
    }


    #[test]
    fn timed_poke_takes_milliseconds_after_the_at_sign() {
        assert_eq!(parse_timed_poke("0x257ccd8=0xc02"), Ok((None, 0x257_ccd8, 0xc02)));
        assert_eq!(parse_timed_poke("0x257ccd8=0xc02@1500"), Ok((Some(1500), 0x257_ccd8, 0xc02)));
        assert_eq!(parse_timed_poke("0x257ccd8=2@0"), Ok((Some(0), 0x257_ccd8, 2)));
        assert!(parse_timed_poke("0x257ccd8=2@1.5").is_err());
        assert!(parse_timed_poke("0x257ccd8=2@").is_err());
        assert!(parse_timed_poke("0x257ccd9=2@10").is_err(), "unaligned address");
        assert!(parse_poke("0x257ccd8=2@10").is_err(), "the untimed parser refuses a time");
        assert_eq!(parse_poke("0x257ccd8=2"), Ok((0x257_ccd8, 2)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ops::ControlFlow;

    /// A machine that advances a fixed amount of guest time and instructions per window.
    struct Slow {
        counts: Vec<u64>,
        now: u64,
        windows: u64,
    }

    impl Slow {
        fn new() -> Self {
            Self { counts: vec![0, 0], now: 0, windows: 0 }
        }
    }

    impl Execution for Slow {
        fn progress(&self) -> (Vec<u64>, u64) {
            (self.counts.clone(), self.now)
        }

        fn run_window(&mut self, _end: u64, _quantum: usize, _pause_after: Option<u64>, _until: Option<u64>) -> Result<(), String> {
            self.windows += 1;
            self.counts[0] += 1;
            self.now += 1;
            Ok(())
        }
    }

    fn drive(limits: Limits, session: &mut Slow) -> u64 {
        limits.drive(session, 1, |_| Ok(ControlFlow::Continue(None)), |_, _| Ok(())).unwrap();
        session.windows
    }

    #[test]
    fn a_wall_deadline_ends_a_run_whose_guest_budgets_never_arrive() {
        let mut session = Slow::new();
        let limits = Limits::new(&session, Some(u64::MAX / 4), Some(Duration::from_secs(3600)), 1_000_000_000)
            .unwrap()
            .with_wall(Some(Duration::from_millis(50)));
        let started = Instant::now();
        drive(limits, &mut session);
        assert_eq!(limits.stop_reason(&session), Some(StopReason::WallClock));
        assert!(started.elapsed() < Duration::from_secs(30));
    }

    #[test]
    fn an_elapsed_wall_deadline_stops_before_any_window() {
        let mut session = Slow::new();
        let limits = Limits::new(&session, None, None, 1_000_000_000).unwrap().with_wall(Some(Duration::ZERO));
        assert_eq!(drive(limits, &mut session), 0);
        assert_eq!(limits.stop_reason(&session), Some(StopReason::WallClock));
    }

    #[test]
    fn guest_budgets_report_their_own_reason() {
        let mut session = Slow::new();
        let limits = Limits::new(&session, Some(5), None, 1_000_000_000).unwrap();
        assert_eq!(drive(limits, &mut session), 5);
        assert_eq!(limits.stop_reason(&session).map(StopReason::name), Some("instruction-limit"));
        let mut session = Slow::new();
        let limits = Limits::new(&session, None, Some(Duration::from_nanos(7)), 1_000_000_000).unwrap();
        assert_eq!(drive(limits, &mut session), 7);
        assert_eq!(limits.stop_reason(&session).map(StopReason::name), Some("guest-time-limit"));
    }

    #[test]
    fn no_budget_never_reports_a_reason() {
        let session = Slow::new();
        let limits = Limits::new(&session, None, None, 1_000_000_000).unwrap();
        assert_eq!(limits.stop_reason(&session), None);
    }
}
