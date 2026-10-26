//! Ordered external inputs and bounded Undo on shared machine keyframes.
//! Instruction counts disambiguate inputs within one guest tick at high ratios.
use super::{Session, checkpoint::Snapshot};
use fujimod_emulator_cpu::ExecutionPolicy;
use fujimod_emulator_devices::parts::touch_input::ContactInput;
use fujimod_emulator_devices::capability::StatisticsCadence;
use fujimod_emulator_exr2_resources::SensorFrame;
use std::{collections::VecDeque, sync::Arc};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Boundary {
    pub tick: u64,
    pub instructions: Vec<u64>,
}
impl Boundary {
    pub fn capture(session: &Session) -> Self {
        Self {
            tick: session.board.now(),
            instructions: session.board.instruction_counts(),
        }
    }
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Input {
    Run {
        instructions: u64,
        quantum: usize,
        pause_after: Option<u64>,
        until: Option<u64>,
    },
    Position {
        control: String,
        position: String,
    },
    Press {
        control: String,
        stage: Option<String>,
    },
    Release {
        control: String,
    },
    Gpio {
        node: String,
        port: u8,
        pin: u8,
        high: bool,
    },
    Adc {
        node: String,
        samples: Vec<u16>,
    },
    Rotate {
        node: String,
        cycles: i32,
    },
    Touch {
        node: String,
        script: Vec<ContactInput>,
    },
    Logic {
        wire: String,
        high: bool,
    },
    Wake {
        node: String,
    },
    Frames {
        node: String,
        period: u64,
        cadence: StatisticsCadence,
    },
    Rgb {
        source: String,
        width: usize,
        height: usize,
        bytes: Vec<u8>,
    },
    SensorRgb {
        source: String,
        width: usize,
        height: usize,
        bytes: Vec<u8>,
        white_luminance: f64,
    },
    SensorRaf {
        source: String,
        frame: SensorFrame,
    },
    SensorRaw16 {
        source: String,
        width: usize,
        height: usize,
        bytes: Vec<u8>,
    },
    SensorImage {
        source: String,
    },
    Analytic {
        source: String,
    },
    CardFormat {
        node: String,
        options: fujimod_emulator_storage::fat::Options,
        files: Vec<(String, Vec<u8>)>,
    },
    CardInsert {
        node: String,
        medium: fujimod_emulator_devices::parts::sd_card::Card,
        present: bool,
        protected: bool,
    },
    CardContacts {
        node: String,
        present: bool,
        protected: bool,
    },
}
impl Input {
    fn retained_bytes(&self) -> usize {
        use std::mem::size_of;
        match self {
            Self::Run { .. } => 0,
            Self::Position { control, position } => control.capacity() + position.capacity(),
            Self::Press { control, stage } => {
                control.capacity() + stage.as_ref().map_or(0, String::capacity)
            }
            Self::Release { control } => control.capacity(),
            Self::Gpio { node, .. }
            | Self::Frames { node, .. }
            | Self::Wake { node }
            | Self::Rotate { node, .. }
            | Self::CardContacts { node, .. } => node.capacity(),
            Self::Adc { node, samples } => node.capacity() + samples.capacity() * size_of::<u16>(),
            Self::Touch { node, script } => {
                node.capacity() + script.capacity() * size_of::<ContactInput>()
            }
            Self::Logic { wire, .. } => wire.capacity(),
            Self::Rgb { source, bytes, .. }
            | Self::SensorRgb { source, bytes, .. }
            | Self::SensorRaw16 { source, bytes, .. } => source.capacity() + bytes.capacity(),
            Self::SensorRaf { source, frame } => source.capacity().saturating_add(frame.bytes),
            Self::SensorImage { source } | Self::Analytic { source } => source.capacity(),
            Self::CardFormat {
                node,
                options,
                files,
            } => files.iter().fold(
                options.read_only_files.iter().fold(
                    node.capacity()
                        + options.label.capacity()
                        + options.read_only_files.capacity() * size_of::<String>()
                        + files.capacity() * size_of::<(String, Vec<u8>)>(),
                    |total, path| total.saturating_add(path.capacity()),
                ),
                |total, (path, bytes)| {
                    total
                        .saturating_add(path.capacity())
                        .saturating_add(bytes.capacity())
                },
            ),
            // Include sparse sector data plus conservative map-node overhead.
            Self::CardInsert { node, medium, .. } => node
                .capacity()
                .saturating_add(medium.dirty_sectors().saturating_mul(512 + 128)),
        }
    }
    fn apply(&self, session: &mut Session) -> Result<(), String> {
        match self {
            Self::Run {
                instructions,
                quantum,
                pause_after,
                until,
            } => session.run_with_limits(*instructions, *quantum, *pause_after, *until),
            Self::Position { control, position } => session.set_position(control, position),
            Self::Press { control, stage } => {
                session.contact_input(control, stage.as_deref(), false)
            }
            Self::Release { control } => session.contact_input(control, None, true),
            Self::Gpio {
                node,
                port,
                pin,
                high,
            } => session.gpio(node, *port, *pin, *high),
            Self::Adc { node, samples } => session.set_adc_samples(node, samples),
            Self::Rotate { node, cycles } => session.rotate(node, *cycles),
            Self::Touch { node, script } => session.touch(node, script),
            Self::Logic { wire, high } => session.logic_input(wire, *high),
            Self::Wake { node } => session.board.inputs_changed(session.node(node)?),
            Self::Frames {
                node,
                period,
                cadence,
            } => session.configure_frames(node, *period, *cadence),
            Self::Rgb {
                source,
                width,
                height,
                bytes,
            } => session.rgb(source, *width, *height, bytes),
            Self::SensorRgb {
                source,
                width,
                height,
                bytes,
                white_luminance,
            } => session.sensor_rgb(source, *width, *height, bytes, *white_luminance),
            Self::SensorRaw16 {
                source,
                width,
                height,
                bytes,
            } => session.sensor_raw16(source, *width, *height, bytes),
            Self::SensorRaf { source, frame } => session.sensor_frame(source, frame),
            Self::SensorImage { source } => session.sensor_image(source),
            Self::Analytic { source } => session.analytic(source),
            Self::CardFormat {
                node,
                options,
                files,
            } => session.card_format(node, options, files),
            Self::CardInsert {
                node,
                medium,
                present,
                protected,
            } => session
                .insert_card(node, medium.clone(), *present, *protected)
                .map(|_| ()),
            Self::CardContacts {
                node,
                present,
                protected,
            } => session.card_contacts(node, *present, *protected),
        }
    }
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Entry {
    pub before: Boundary,
    pub input: Input,
    pub after: Boundary,
    pub failure: Option<String>,
}
impl Entry {
    fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            .saturating_add(self.input.retained_bytes())
            .saturating_add(
                (self.before.instructions.capacity() + self.after.instructions.capacity())
                    * std::mem::size_of::<u64>(),
            )
            .saturating_add(self.failure.as_ref().map_or(0, String::capacity))
    }
}

// Host Undo cadence, independent of the board clock and guest scheduling.
const AUTOMATIC_INPUT_INTERVAL: usize = 16;

struct Mark {
    saved: Arc<Snapshot>,
    cursor: usize,
}
pub struct History {
    limit: usize,
    device_byte_limit: usize,
    marks: VecDeque<Mark>,
    log: Vec<Entry>,
}
impl History {
    pub fn new(limit: usize, device_byte_limit: usize) -> Result<Self, String> {
        if !(1..=1024).contains(&limit) || device_byte_limit == 0 {
            return Err(
                "history requires 1..1024 keyframes and a positive device-byte limit".into(),
            );
        }
        Ok(Self {
            limit,
            device_byte_limit,
            marks: VecDeque::new(),
            log: Vec::new(),
        })
    }
    pub fn retained_marks(&self) -> usize {
        self.marks.len()
    }
    /// Retained machine points, newest first, without restoring or replaying.
    pub fn snapshots(&self) -> impl Iterator<Item = &Snapshot> {
        self.marks.iter().rev().map(|mark| mark.saved.as_ref())
    }
    pub fn entries(&self) -> &[Entry] {
        &self.log
    }
    /// Retain an automatic Undo boundary before an external transaction.
    /// Exact keeps every boundary. Interactive groups ordered inputs and trims
    /// their retained payloads to the device-byte budget. One oversized input,
    /// like one oversized keyframe, may exceed that budget until the next mark.
    /// Call once per transaction; explicit `mark` always captures immediately.
    /// Returns true only when captured. Presentation caches append on true and
    /// trim their oldest entries to `retained_marks()` after eviction.
    pub fn mark_automatic(&mut self, session: &Session, input: &Input) -> Result<bool, String> {
        if session.board.execution_policy() == ExecutionPolicy::Exact {
            self.mark(session)?;
            return Ok(true);
        }
        let next_bytes = std::mem::size_of::<Entry>()
            .saturating_add(input.retained_bytes())
            .saturating_add(
                session.board.instruction_counts().len() * 2 * std::mem::size_of::<u64>(),
            );
        let due = self
            .marks
            .back()
            .is_none_or(|mark| self.log.len() - mark.cursor >= AUTOMATIC_INPUT_INTERVAL);
        if !due && self.retained_input_bytes().saturating_add(next_bytes) <= self.device_byte_limit
        {
            return Ok(false);
        }
        self.mark(session)?;
        while self.marks.len() > 1
            && self.retained_input_bytes().saturating_add(next_bytes) > self.device_byte_limit
        {
            self.marks.pop_front();
            self.discard_obsolete_inputs();
        }
        Ok(true)
    }
    fn retained_input_bytes(&self) -> usize {
        self.log
            .iter()
            .map(Entry::retained_bytes)
            .fold(0usize, usize::saturating_add)
    }
    fn discard_obsolete_inputs(&mut self) {
        let obsolete = self.marks.front().map_or(0, |m| m.cursor);
        if obsolete != 0 {
            self.log.drain(..obsolete);
            for m in &mut self.marks {
                m.cursor -= obsolete;
            }
        }
    }
    /// Capture before an external transaction, not from a host read or getter.
    pub fn mark(&mut self, session: &Session) -> Result<(), String> {
        let previous = self.marks.back().map(|m| m.saved.as_ref());
        let saved = Arc::new(session.checkpoint(previous)?);
        self.marks.push_back(Mark {
            saved,
            cursor: self.log.len(),
        });
        while self.marks.len() > 1
            && (self.marks.len() > self.limit
                || self
                    .marks
                    .iter()
                    .map(|m| m.saved.encoded_device_bytes())
                    .fold(0usize, usize::saturating_add)
                    > self.device_byte_limit)
        {
            self.marks.pop_front();
        }
        // Drop inputs older than the oldest retained machine state.
        self.discard_obsolete_inputs();
        Ok(())
    }
    pub fn apply(&mut self, session: &mut Session, input: Input) -> Result<(), String> {
        let before = Boundary::capture(session);
        let result = input.apply(session);
        let after = Boundary::capture(session);
        self.log.push(Entry {
            before,
            input,
            after,
            failure: result.clone().err(),
        });
        result
    }
    pub fn undo(&mut self, session: &mut Session) -> Result<Boundary, String> {
        let mark = self.marks.back().ok_or("no retained Undo keyframe")?;
        session.restore(&mark.saved)?;
        let cursor = mark.cursor;
        self.marks.pop_back();
        self.log.truncate(cursor);
        Ok(Boundary::capture(session))
    }
    /// Restore and replay actual external inputs to an absolute guest tick.
    /// Inputs at one tick retain their original insertion/instruction order.
    pub fn rollback(&mut self, session: &mut Session, tick: u64) -> Result<Boundary, String> {
        if tick > session.board.now() {
            return Err("rollback target is in the future".into());
        }
        let live = session.checkpoint(self.marks.back().map(|m| m.saved.as_ref()))?;
        match self.rollback_inner(session, tick) {
            Ok(boundary) => Ok(boundary),
            Err(error) => {
                session
                    .restore(&live)
                    .map_err(|restore| format!("{error}; rollback recovery failed: {restore}"))?;
                Err(error)
            }
        }
    }
    fn rollback_inner(&mut self, session: &mut Session, tick: u64) -> Result<Boundary, String> {
        if tick > session.board.now() {
            return Err("rollback target is in the future".into());
        }
        let index = self
            .marks
            .iter()
            .rposition(|m| m.saved.now() <= tick)
            .ok_or("rollback target precedes retained history")?;
        let mark = &self.marks[index];
        let cursor = mark.cursor;
        let saved = mark.saved.clone();
        let replay = self.log[cursor..].to_vec();
        session.restore(&saved)?;
        let exact = session.board.execution_policy() == ExecutionPolicy::Exact;
        let mut continuation = Vec::new();
        for entry in replay {
            if entry.before.tick > tick {
                break;
            }
            if exact && Boundary::capture(session) != entry.before {
                return Err("input replay boundary differs".into());
            }
            let partial = entry.after.tick > tick;
            let input = match entry.input {
                Input::Run {
                    instructions,
                    quantum,
                    pause_after,
                    until,
                } if partial || !exact => Input::Run {
                    instructions,
                    quantum,
                    pause_after,
                    until: Some(until.map_or(tick, |until| until.min(tick))),
                },
                _ if partial => return Err("external input advanced guest time".into()),
                input => input,
            };
            let before = Boundary::capture(session);
            let result = input.apply(session);
            let failure = result.clone().err();
            if partial {
                result?;
                continuation.push(Entry {
                    before,
                    input,
                    after: Boundary::capture(session),
                    failure: None,
                });
                break;
            }
            let after = Boundary::capture(session);
            if failure != entry.failure || exact && after != entry.after {
                return Err("input replay result differs".into());
            }
            continuation.push(Entry {
                before,
                input,
                after,
                failure,
            });
        }
        if exact && session.board.now() != tick {
            return Err("input log does not reach rollback target".into());
        }
        if session.board.now() > tick {
            return Err("input replay exceeded rollback target".into());
        }
        self.marks.truncate(index + 1);
        self.log.truncate(cursor);
        self.log.extend(continuation);
        Ok(Boundary::capture(session))
    }
    pub fn replay(session: &mut Session, entries: &[Entry]) -> Result<(), String> {
        let exact = session.board.execution_policy() == ExecutionPolicy::Exact;
        for entry in entries {
            if exact && Boundary::capture(session) != entry.before {
                return Err("input replay boundary differs".into());
            }
            let failure = entry.input.apply(session).err();
            if failure != entry.failure || exact && Boundary::capture(session) != entry.after {
                return Err("input replay result differs".into());
            }
        }
        Ok(())
    }
}
