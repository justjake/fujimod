//! Same-owner keyframe IDs and presentation state owned by this transport.
use super::{
    results::{Checkpoint as CheckpointResult, ErrorReport, InputLog, Point},
    session::Session,
};
use fujimod_emulator_session::{
    checkpoint::Snapshot,
    history::{Boundary, Entry, History as CoreHistory, Input},
};
use fujimod_emulator_devices::graphics::scanout::Frame;
use serde_json::json;
use std::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
};

const HISTORY_LIMIT: usize = 32;
const CHECKPOINT_LIMIT: usize = 8;
// Separate device-state budgets leave room for RAM versions, live media and cold construction.
const DEVICE_BYTE_LIMIT: usize = 128 << 20;

#[derive(serde::Serialize)]
struct LogView<'a> {
    schema: u32,
    owner: u32,
    generation: u64,
    entries: &'a [Entry],
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Log {
    schema: u32,
    owner: u32,
    generation: u64,
    entries: Vec<Entry>,
}

#[derive(Clone)]
pub(super) struct Presentation {
    pub order: u64,
    pub boundary: Boundary,
    pub frames: BTreeMap<usize, Arc<Frame>>,
    pub powered_on: bool,
}
struct Checkpoint {
    id: String,
    core: Snapshot,
    presentation: Presentation,
}
pub(super) struct History {
    core: CoreHistory,
    marks: VecDeque<Presentation>,
    checkpoints: VecDeque<Checkpoint>,
    next: u32,
}
impl History {
    fn new() -> Result<Self, String> {
        Ok(Self {
            core: CoreHistory::new(HISTORY_LIMIT, DEVICE_BYTE_LIMIT)?,
            marks: VecDeque::new(),
            checkpoints: VecDeque::new(),
            next: 0,
        })
    }
    fn branch(&mut self) -> Result<(), String> {
        self.core = CoreHistory::new(HISTORY_LIMIT, DEVICE_BYTE_LIMIT)?;
        self.marks.clear();
        Ok(())
    }
}
impl Session {
    pub fn error_report(&mut self) -> Result<ErrorReport, String> {
        let mut report = self.core.board.state();
        let mut rejected = Vec::new();
        let live = if self.failed {
            None
        } else {
            match self.core.checkpoint(None) {
                Ok(saved) => Some(saved),
                Err(error) => {
                    rejected.push(json!({"source":"current","tick":self.core.board.now().to_string(),"error":error}));
                    None
                }
            }
        };
        let mut candidates = Vec::new();
        if let Some(live) = &live {
            candidates.push(("current", live, self.presentation()));
        }
        if let Some(history) = &self.history {
            for (saved, presentation) in history.core.snapshots().zip(history.marks.iter().rev()) {
                candidates.push(("history", saved, presentation.clone()));
            }
            for saved in history.checkpoints.iter().rev() {
                candidates.push(("checkpoint", &saved.core, saved.presentation.clone()));
            }
        }
        if let Some(reset) = &self.reset_image {
            candidates.push((
                "reset",
                reset.as_ref(),
                Presentation {
                    boundary: Boundary {
                        tick: reset.now(),
                        instructions: reset.instruction_counts().to_vec(),
                    },
                    frames: Default::default(),
                    powered_on: true,
                    order: 0,
                },
            ));
        }
        // Capture order also distinguishes input-only points within one tick,
        // and a new branch after rewinding past a retained explicit checkpoint.
        candidates.sort_by_key(|(_, _, presentation)| std::cmp::Reverse(presentation.order));
        for (source, saved, presentation) in candidates {
            let encoded = match saved.failure() {
                Some(error) => Err(format!("saved execution failed: {error}")),
                None => self.encode_point(saved, &presentation),
            };
            match encoded {
                Ok(bytes) => {
                    report["snapshot"] = json!({"source":source,"tick":saved.now().to_string(),"instructions":saved.instruction_counts().iter().map(u64::to_string).collect::<Vec<_>>(),"rejected":rejected});
                    return Ok(ErrorReport {
                        report,
                        snapshot: Some(bytes),
                    });
                }
                Err(error) => rejected
                    .push(json!({"source":source,"tick":saved.now().to_string(),"error":error})),
            }
        }
        report["snapshot"] = json!({"unavailable":true,"rejected":rejected});
        Ok(ErrorReport {
            report,
            snapshot: None,
        })
    }
    pub(super) fn presentation(&self) -> Presentation {
        let order = self.next_capture_order();
        Presentation {
            order,
            boundary: Boundary::capture(&self.core),
            frames: self.frames.clone(),
            powered_on: self.powered_on,
        }
    }
    fn next_capture_order(&self) -> u64 {
        let order = self.capture_order.get();
        self.capture_order.set(order.saturating_add(1));
        order
    }
    pub(super) fn collect_frames(&mut self) {
        for frame in self.core.board.take_frames() {
            self.frames.insert(frame.output, Arc::new(frame));
        }
    }
    pub(super) fn restored(&mut self) {
        self.failed = self.core.board.state()["failure"].is_string();
        self.collect_frames();
    }
    pub fn checkpoint(&mut self) -> Result<CheckpointResult, String> {
        let next = self
            .history
            .as_ref()
            .map_or(0, |h| h.next)
            .checked_add(1)
            .ok_or("checkpoint ID space exhausted")?;
        let previous = self
            .history
            .as_ref()
            .and_then(|h| h.checkpoints.back())
            .map(|c| &c.core);
        let saved = self.core.checkpoint(previous)?;
        let presentation = self.presentation();
        let id = format!("{}:{}:{next}", self.owner, self.generation);
        let point = Point::new(saved.now(), saved.instruction_counts());
        if self.history.is_none() {
            self.history = Some(History::new()?);
        }
        let history = self.history.as_mut().expect("initialized history");
        history.checkpoints.push_back(Checkpoint {
            id: id.clone(),
            core: saved,
            presentation,
        });
        history.next = next;
        while history.checkpoints.len() > 1
            && (history.checkpoints.len() > CHECKPOINT_LIMIT
                || history
                    .checkpoints
                    .iter()
                    .map(|c| c.core.encoded_device_bytes())
                    .fold(0usize, usize::saturating_add)
                    > DEVICE_BYTE_LIMIT)
        {
            history.checkpoints.pop_front();
        }
        Ok(CheckpointResult {
            id,
            tick: point.tick,
            instructions: point.instructions,
            retained_ids: history.checkpoints.iter().map(|c| c.id.clone()).collect(),
        })
    }
    pub(super) fn mark_input(&mut self, input: &Input) -> Result<(), String> {
        let order = self.next_capture_order();
        if let Some(history) = self.history.as_mut() {
            if history.core.mark_automatic(&self.core, input)? {
                history.marks.push_back(Presentation {
                    order,
                    boundary: Boundary::capture(&self.core),
                    frames: self.frames.clone(),
                    powered_on: self.powered_on,
                });
                while history.marks.len() > history.core.retained_marks() {
                    history.marks.pop_front();
                }
            }
        }
        Ok(())
    }
    pub(super) fn apply_marked_input(&mut self, input: Input) -> Result<(), String> {
        self.mark_input(&input)?;
        self.apply_input(input)
    }
    pub(super) fn apply_input(&mut self, input: Input) -> Result<(), String> {
        match &mut self.history {
            Some(history) => history.core.apply(&mut self.core, input),
            // No machine capture or retained log before the first checkpoint.
            None => CoreHistory::new(1, DEVICE_BYTE_LIMIT)?.apply(&mut self.core, input),
        }
    }
    pub fn restore_checkpoint(&mut self, id: &str) -> Result<Point, String> {
        let history = self
            .history
            .as_mut()
            .ok_or("unknown or expired checkpoint ID")?;
        let saved = history
            .checkpoints
            .iter()
            .find(|c| c.id == id)
            .ok_or("unknown or expired checkpoint ID")?;
        self.core.restore(&saved.core)?;
        self.frames = saved.presentation.frames.clone();
        self.powered_on = saved.presentation.powered_on;
        history.branch()?;
        self.restored();
        Ok(Boundary::capture(&self.core).into())
    }
    pub fn undo(&mut self) -> Result<Point, String> {
        let history = self.history.as_mut().ok_or("no retained Undo keyframe")?;
        let presentation = history
            .marks
            .back()
            .ok_or("no retained Undo keyframe")?
            .clone();
        let restored = history.core.undo(&mut self.core)?;
        history.marks.pop_back();
        self.frames = presentation.frames;
        self.powered_on = presentation.powered_on;
        self.restored();
        Ok(restored.into())
    }
    pub fn rollback(&mut self, tick: u64) -> Result<Point, String> {
        let history = self
            .history
            .as_mut()
            .ok_or("rollback target precedes retained history")?;
        let index = history
            .marks
            .iter()
            .rposition(|p| p.boundary.tick <= tick)
            .ok_or("rollback target precedes retained history")?;
        let restored = history.core.rollback(&mut self.core, tick)?;
        self.frames = history.marks[index].frames.clone();
        self.powered_on = history.marks[index].powered_on;
        history.marks.truncate(index + 1);
        self.restored();
        Ok(restored.into())
    }
    pub fn replay(&mut self, bytes: &[u8]) -> Result<Point, String> {
        if bytes.is_empty() || bytes.len() > 64 << 20 {
            return Err("input log must contain CBOR and fit within 64 MiB".into());
        }
        let mut input = bytes;
        let log: Log =
            ciborium::from_reader(&mut input).map_err(|e| format!("input log decode: {e}"))?;
        if log.schema != 1 || !input.is_empty() {
            return Err("invalid input log schema or trailing bytes".into());
        }
        if log.owner != self.owner || log.generation != self.generation {
            return Err("input log belongs to another session".into());
        }
        if log.entries.len() > 4096 {
            return Err("replay exceeds 4096 entries".into());
        }
        let history = self
            .history
            .as_mut()
            .ok_or("checkpoint is required before replay")?;
        let previous = history.checkpoints.back().map(|c| &c.core);
        let live = self.core.checkpoint(previous)?;
        if let Err(error) = CoreHistory::replay(&mut self.core, &log.entries) {
            self.core
                .restore(&live)
                .map_err(|restore| format!("{error}; replay recovery failed: {restore}"))?;
            return Err(error);
        }
        history.branch()?;
        self.restored();
        Ok(Boundary::capture(&self.core).into())
    }
    pub fn history_log(&self) -> Result<InputLog, String> {
        let entries = self.history.as_ref().map_or(&[][..], |h| h.core.entries());
        let mut log = Vec::new();
        ciborium::into_writer(
            &LogView {
                schema: 1,
                owner: self.owner,
                generation: self.generation,
                entries,
            },
            &mut log,
        )
        .map_err(|e| format!("input log encode: {e}"))?;
        Ok(InputLog {
            log,
            entries: entries.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_cpu::ExecutionPolicy;

    fn session(policy: ExecutionPolicy) -> Session {
        let code = 0xeafffffeu32.to_le_bytes();
        let config = json!({"schema":1,"firmwareId":"loop","bootSource":"internal","graph":{
            "schema":1,"firmwareId":"loop","bootSource":"internal","quantum":17,
            "execution":{"processors":[{"model":"cortex-r5f","registers":[["Pc",65536],["Pstate",211]],"released":true,"mprofile":null,"rprofile":{"cp14_identity":16387,"tcm":{"atcm":{"bytes":0,"reset":0},"btcm":{"bytes":0,"reset":0}}}}],
                "ram":[{"base":65536,"size":4096}],"dma_mappings":[{"address":65536,"physical":65536,"size":4096,"permissions":3}],
                "mappings":[{"core":0,"address":65536,"physical":65536,"size":4096,"permissions":7}],
                "peripherals":[{"core":0,"address":0xfffb3000u64,"physical":0xfffb3000u64,"size":4096}],"decoders":[],"instructions_per_tick":[24,1]},
            "devices":{"schema":1,"boot_entry":"loader","wires":{},"nodes":[{"name":"irq","boot_entries":["loader"],"windows":[{"base":0xfffb3000u64,"size":4096}],"dependencies":[],"model":{"kind":"exr2-interrupt-controller","options":{"arbitration":"unique"},"core":0,"base":0xfffb3000u64,"routes":null,"software":[]}}]},
            "firmware":{"size":4,"sha256":fujimod_firmware::sha256(&code),"format":"raw",
                "mappings":[{"name":"loop","offset":0,"size":4,"base":65536,"encoding":"raw","executable":true}]}
        }});
        Session::create_camera(&serde_json::to_vec(&config).unwrap(), &code, policy).unwrap()
    }
    fn frame(session: &mut Session, sequence: u64) {
        session.frames.insert(
            0,
            Arc::new(Frame {
                output: 0,
                sequence,
                width: 1,
                height: 1,
                rgba: vec![sequence as u8, 0, 0, 255],
            }),
        );
    }
    #[test]
    fn cold_power_keeps_shared_mosaic_input_without_raf_bytes() {
        use fujimod_emulator_imaging::scene::{Frame as SensorFrame, FramePixels, SensorFrames, mosaic::Radiance};
        use std::sync::Mutex;
        let mut session = session(ExecutionPolicy::Interactive);
        let sensor = Arc::new(Mutex::new(SensorFrames::new(
            SensorFrame::new(1, 1, FramePixels::Rgb24(vec![0; 3].into())).unwrap(),
        )));
        fujimod_emulator_exr2_images::images_mut(&mut session.core.resources)
            .unwrap()
            .sensors
            .insert("sensor".into(), sensor.clone());
        session.reset_image = Some(Arc::new(session.core.checkpoint(None).unwrap()));
        let frame: Arc<SensorFrame> = Arc::new(
            SensorFrame::new(
                7,
                3,
                FramePixels::Mosaic {
                    cfa: [[1; 6]; 6],
                    radiance: Radiance::from_counts(0..21, 2.),
                },
            )
            .unwrap(),
        );
        session
            .apply_marked_input(Input::SensorRaf {
                source: "sensor".into(),
                frame: fujimod_emulator_exr2_resources::SensorFrame {
                    frame: frame.clone(),
                    bytes: 0,
                },
            })
            .unwrap();
        let fingerprint = frame.fingerprint();
        session.power(false).unwrap();
        assert!(Arc::ptr_eq(&sensor.lock().unwrap().current, &frame));
        session.power(true).unwrap();
        session.power_cycle(&[]).unwrap();
        let current = sensor.lock().unwrap().current.clone();
        assert!(Arc::ptr_eq(&current, &frame));
        assert_eq!(current.fingerprint(), fingerprint);
    }

    #[test]
    fn coalesced_marks_keep_frames_aligned_through_eviction_undo_and_restore() {
        let mut session = session(ExecutionPolicy::Interactive);
        frame(&mut session, 1);
        let checkpoint = session.checkpoint().unwrap().id;
        let first_tick = session.core.board.now();
        for i in 0..36 {
            session.run(first_tick + (i + 1) * 10).unwrap();
            frame(&mut session, i + 2);
        }
        let history = session.history.as_ref().unwrap();
        assert_eq!(history.core.retained_marks(), 3);
        assert_eq!(history.marks.len(), 3);
        session.rollback(first_tick + 325).unwrap();
        assert!(session.core.board.now() <= first_tick + 325);
        assert_eq!(session.frames[&0].sequence, 33);
        session.undo().unwrap();
        assert_eq!(session.frames[&0].sequence, 33);
        session.undo().unwrap();
        assert_eq!(session.frames[&0].sequence, 17);
        session.undo().unwrap();
        assert_eq!(session.frames[&0].sequence, 1);
        session.restore_checkpoint(&checkpoint).unwrap();
        assert_eq!(session.core.board.now(), first_tick);
        assert_eq!(session.frames[&0].sequence, 1);
        assert!(session.history.as_ref().unwrap().marks.is_empty());
        // Force core retention to evict old automatic marks while the frame
        // queue follows the same surviving machine states.
        session.history.as_mut().unwrap().core = CoreHistory::new(1, usize::MAX).unwrap();
        for i in 0..20 {
            session.run(first_tick + (i + 1) * 10).unwrap();
            frame(&mut session, i + 2);
        }
        let history = session.history.as_ref().unwrap();
        assert_eq!(history.core.retained_marks(), 1);
        assert_eq!(history.marks.len(), 1);
        session.undo().unwrap();
        assert_eq!(session.frames[&0].sequence, 17);
    }
    #[test]
    fn exact_automatic_marks_keep_each_presented_boundary() {
        let mut session = session(ExecutionPolicy::Exact);
        frame(&mut session, 1);
        session.checkpoint().unwrap();
        for i in 0..3 {
            session.run((i + 1) * 10).unwrap();
            frame(&mut session, i + 2);
        }
        assert_eq!(session.history.as_ref().unwrap().marks.len(), 3);
        for sequence in (1..=3).rev() {
            session.undo().unwrap();
            assert_eq!(session.frames[&0].sequence, sequence);
        }
    }
    #[test]
    fn report_selects_the_latest_capture_when_inputs_do_not_advance_the_guest() {
        let mut session = session(ExecutionPolicy::Interactive);
        frame(&mut session, 1);
        session.checkpoint().unwrap();
        session
            .mark_input(&Input::Run {
                instructions: 0,
                quantum: 17,
                pause_after: None,
                until: None,
            })
            .unwrap();
        frame(&mut session, 2);
        session.checkpoint().unwrap();
        session.failed = true;
        let report = session.error_report().unwrap();
        assert_eq!(report.report["snapshot"]["source"], "checkpoint");
        let restored = Session::load_snapshot(report.snapshot.as_ref().unwrap()).unwrap();
        assert_eq!(restored.frames[&0].sequence, 2);
    }
    #[test]
    fn report_selection_follows_the_current_branch_after_rewind() {
        let mut session = session(ExecutionPolicy::Interactive);
        let cold = session.checkpoint().unwrap().id;
        session.run(20).unwrap();
        session.checkpoint().unwrap();
        session.restore_checkpoint(&cold).unwrap();
        session.run(10).unwrap();
        session.failed = true;
        let report = session.error_report().unwrap();
        assert_eq!(report.report["snapshot"]["source"], "history");
        assert_eq!(report.report["snapshot"]["tick"], "0");
    }
}
