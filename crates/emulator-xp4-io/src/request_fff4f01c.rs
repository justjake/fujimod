//! Explicit XP4 0xfff4f01c request acknowledgment hypothesis.
//! The declared delay is unmeasured and carries no physical clock or reset effect.
use fujimod_emulator_devices::{Cx, Device, Record, Value as Field};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const ADDRESS: u64 = 0xfff4f01c;
pub const WIDTH: u8 = 4;

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Request {
    started_at: u64,
    deadline: u64,
    token: u64,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case", deny_unknown_fields)]
enum State {
    Unwritten {},
    Pending { request: Request },
    Completed { request: Request, completed_at: u64 },
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RequestWord {
    completion_ticks: u64,
    generation: u64,
    state: State,
}

impl RequestWord {
    pub fn new(completion_ticks: u64) -> Result<Self, String> {
        if !(1..=1_000_000).contains(&completion_ticks) {
            return Err("Invalid XP4 0xfff4f01c request delay".into());
        }
        Ok(Self {
            completion_ticks,
            generation: 0,
            state: State::Unwritten {},
        })
    }

    fn valid_request(&self, request: Request) -> bool {
        request.token == self.generation
            && request.token.checked_sub(1)
                .and_then(|prior| prior.checked_mul(self.completion_ticks))
                .is_some_and(|earliest_start| earliest_start <= request.started_at)
            && request.started_at.checked_add(self.completion_ticks) == Some(request.deadline)
    }
}

impl Device for RequestWord {
    fn name(&self) -> &'static str {
        "xp4-request-fff4f01c"
    }

    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        if address != ADDRESS || size != WIDTH {
            cx.unmodeled();
            return 0;
        }
        match self.state {
            State::Unwritten {} => {
                cx.fail("XP4 0xfff4f01c read before first request");
                0
            }
            State::Pending { .. } => 1,
            State::Completed { .. } => 0,
        }
    }

    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        if address != ADDRESS || size != WIDTH {
            cx.unmodeled();
            return;
        }
        if matches!(self.state, State::Pending { .. }) {
            cx.fail("XP4 0xfff4f01c write while request is pending");
            return;
        }
        if value != 1 {
            cx.fail("XP4 0xfff4f01c request requires a write of 1");
            return;
        }
        let started_at = cx.now();
        let (Some(deadline), Some(token)) = (
            started_at.checked_add(self.completion_ticks),
            self.generation.checked_add(1),
        ) else {
            cx.fail("XP4 0xfff4f01c request deadline or generation overflow");
            return;
        };
        self.state = State::Pending {
            request: Request { started_at, deadline, token },
        };
        self.generation = token;
        cx.record(Record::new(
            "XP4 experimental FFF4F01C request scheduled",
            [
                ("address", Field::int(ADDRESS)),
                ("tick", Field::int(started_at)),
                ("deadline", Field::int(deadline)),
                ("ticks_unmeasured", Field::int(self.completion_ticks)),
            ],
        ));
        cx.schedule(deadline, token);
    }

    fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
        let State::Pending { request } = self.state else { return; };
        let completed_at = cx.now();
        if request.token != token || completed_at < request.deadline {
            return;
        }
        self.state = State::Completed { request, completed_at };
        cx.record(Record::new(
            "XP4 experimental FFF4F01C request acknowledged",
            [
                ("address", Field::int(ADDRESS)),
                ("tick", Field::int(completed_at)),
                ("deadline", Field::int(request.deadline)),
            ],
        ));
    }

    fn next_deadline(&self, _: u64) -> Option<u64> {
        match self.state {
            State::Pending { request } => Some(request.deadline),
            _ => None,
        }
    }

    fn pollable(&self, address: u64, size: u8) -> bool {
        address == ADDRESS && size == WIDTH && !matches!(self.state, State::Unwritten {})
    }

    fn save(&self) -> Value {
        json!(self)
    }

    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: Self = serde_json::from_value(value.clone()).map_err(|error| error.to_string())?;
        let valid_history = match state.state {
            State::Unwritten {} => state.generation == 0,
            State::Pending { request } => state.valid_request(request),
            State::Completed { request, completed_at } => {
                state.valid_request(request) && completed_at >= request.deadline
            }
        };
        // The host owns event delivery. This validates only the device state.
        if state.completion_ticks != self.completion_ticks || !valid_history {
            return Err("Invalid XP4 0xfff4f01c request checkpoint or delay".into());
        }
        *self = state;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::recording::RecordingHost;

    fn read(model: &mut RequestWord, host: &mut RecordingHost, address: u64, width: u8) -> Result<u64, String> {
        let mut cx = Cx::new(host, 0, 0, 4);
        let value = model.read(&mut cx, address, width);
        cx.take_refusal().map_or(Ok(value), Err)
    }

    fn write(
        model: &mut RequestWord, host: &mut RecordingHost, address: u64, width: u8, value: u64,
    ) -> Result<(), String> {
        let mut cx = Cx::new(host, 3, 0x1000, 4);
        model.write(&mut cx, address, width, value);
        cx.take_refusal().map_or(Ok(()), Err)
    }

    fn event(model: &mut RequestWord, host: &mut RecordingHost, token: u64) {
        model.event(&mut Cx::new(host, 0, 0, 4), token);
    }

    #[test]
    fn only_the_scheduled_token_at_or_after_its_deadline_acknowledges() {
        let mut model = RequestWord::new(8).unwrap();
        let mut host = RecordingHost { now: 100, ..Default::default() };
        let unwritten = model.save();
        event(&mut model, &mut host, 1);
        assert!(read(&mut model, &mut host, ADDRESS, WIDTH).is_err());
        assert!(!model.pollable(ADDRESS, WIDTH));
        assert_eq!(model.save(), unwritten);

        for (token, start, completion) in [(1, 100, 108), (2, 108, 117)] {
            host.now = start;
            write(&mut model, &mut host, ADDRESS, WIDTH, 1).unwrap();
            let pending = model.save();
            assert_eq!(model.next_deadline(host.now), Some(start + 8));
            assert_eq!(host.events.last(), Some(&(4, start + 8, token)));
            assert!(model.pollable(ADDRESS, WIDTH));

            host.now = start + 7;
            event(&mut model, &mut host, token);
            host.now = completion;
            for ignored in [token - 1, token + 1] {
                event(&mut model, &mut host, ignored);
            }
            model.advance(&mut Cx::new(&mut host, 0, 0, 4));
            assert_eq!(read(&mut model, &mut host, ADDRESS, WIDTH).unwrap(), 1);
            assert_eq!(model.save(), pending);
            event(&mut model, &mut host, token);
            assert_eq!(read(&mut model, &mut host, ADDRESS, WIDTH).unwrap(), 0);
            assert_eq!(model.next_deadline(host.now), None);
            let completed = model.save();
            event(&mut model, &mut host, token);
            assert_eq!(model.save(), completed);
        }
        assert_eq!(host.events, [(4, 108, 1), (4, 116, 2)]);
        assert!(host.irqs.is_empty());
        assert!(host.core_irqs.is_empty());
        assert!(host.resets.is_empty());
    }

    #[test]
    fn other_accesses_and_every_write_while_pending_are_refused_without_effects() {
        let mut model = RequestWord::new(8).unwrap();
        let mut host = RecordingHost::default();
        for pending in [false, true] {
            if pending {
                write(&mut model, &mut host, ADDRESS, WIDTH, 1).unwrap();
            }
            let before = model.save();
            let events = host.events.clone();
            for (address, width) in [
                (ADDRESS - 4, 4), (ADDRESS + 4, 4), (ADDRESS + 1, 4),
                (ADDRESS, 1), (ADDRESS, 2), (ADDRESS, 8),
            ] {
                assert!(read(&mut model, &mut host, address, width).is_err());
                assert!(write(&mut model, &mut host, address, width, 1).is_err());
                assert!(!model.pollable(address, width));
            }
            for value in [0, 2, 3, u32::MAX as u64, (1 << 32) | 1] {
                assert!(write(&mut model, &mut host, ADDRESS, WIDTH, value).is_err());
            }
            if pending {
                assert!(write(&mut model, &mut host, ADDRESS, WIDTH, 1).is_err());
            }
            assert_eq!(model.save(), before);
            assert_eq!(host.events, events);
        }
    }

    #[test]
    fn delay_validation_and_overflow_preserve_state_and_scheduling() {
        for delay in [0, 1_000_001, u64::MAX] {
            assert!(RequestWord::new(delay).is_err());
        }
        assert!(RequestWord::new(1_000_000).is_ok());
        let mut model = RequestWord::new(1).unwrap();
        let mut host = RecordingHost { now: u64::MAX, ..Default::default() };
        let before = model.save();
        assert!(write(&mut model, &mut host, ADDRESS, WIDTH, 1).is_err());
        assert_eq!(model.save(), before);
        let exhausted = json!({
            "completion_ticks": 1, "generation": u64::MAX,
            "state": {"phase": "completed", "completed_at": u64::MAX, "request": {
                "started_at": u64::MAX - 1, "deadline": u64::MAX, "token": u64::MAX
            }}
        });
        model.restore(&exhausted).unwrap();
        host.now = 0;
        let error = write(&mut model, &mut host, ADDRESS, WIDTH, 1).unwrap_err();
        assert!(error.contains("overflow"));
        assert_eq!(model.save(), exhausted);
        assert!(host.events.is_empty());
    }

    #[test]
    fn checkpoint_validates_inputs_structure_timing_and_request_history() {
        let mut model = RequestWord::new(8).unwrap();
        let mut resumed = RequestWord::new(8).unwrap();
        let mut host = RecordingHost::default();
        let cold = model.save();
        resumed.restore(&cold).unwrap();
        let mut corrupt = cold.clone();
        corrupt["generation"] = json!(1);
        assert!(resumed.restore(&corrupt).is_err());
        let mut corrupt = cold.clone();
        corrupt["state"]["request"] = Value::Null;
        assert!(resumed.restore(&corrupt).is_err());

        write(&mut model, &mut host, ADDRESS, WIDTH, 1).unwrap();
        let pending = model.save();
        resumed.restore(&pending).unwrap();
        assert_eq!(resumed.next_deadline(0), Some(8));
        assert_eq!(host.events, [(4, 8, 1)]);
        assert!(RequestWord::new(9).unwrap().restore(&pending).is_err());
        for (field, value) in [("started_at", json!(1)), ("deadline", json!(7)), ("token", json!(0))] {
            let mut corrupt = pending.clone();
            corrupt["state"]["request"][field] = value;
            assert!(resumed.restore(&corrupt).is_err(), "{field}");
            assert_eq!(resumed.save(), pending);
        }
        for (field, value) in [("phase", json!("unknown")), ("completed_at", json!(8)), ("unexpected", json!(0))] {
            let mut corrupt = pending.clone();
            corrupt["state"][field] = value;
            assert!(resumed.restore(&corrupt).is_err(), "{field}");
            assert_eq!(resumed.save(), pending);
        }
        let mut corrupt = pending.clone();
        corrupt["state"]["request"]["token"] = json!(2);
        assert!(resumed.restore(&corrupt).is_err());
        corrupt["generation"] = json!(2);
        assert!(resumed.restore(&corrupt).is_err());

        host.now = 8;
        event(&mut model, &mut host, 1);
        event(&mut resumed, &mut host, 1);
        let completed = model.save();
        assert_eq!(resumed.save(), completed);
        resumed.restore(&completed).unwrap();
        let mut corrupt = completed.clone();
        corrupt["state"]["completed_at"] = json!(7);
        assert!(resumed.restore(&corrupt).is_err());
        assert_eq!(resumed.save(), completed);
        assert_eq!(host.events, [(4, 8, 1)]);
    }
}
