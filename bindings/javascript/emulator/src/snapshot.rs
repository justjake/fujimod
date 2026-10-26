//! Portable running sessions, including their constructor inputs and presentation.
use super::{
    history::Presentation,
    results::{Point, SessionSnapshot},
    session::Session,
};
use fujimod_emulator_session::checkpoint::Snapshot;
use fujimod_emulator_cpu::ExecutionPolicy;
use fujimod_emulator_devices::{checkpoint::Checkpoint as Encoded, graphics::scanout::Frame};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};

const KIND: &str = "wasm-session-v1";
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Resolved,
    Camera,
    Prepared,
}
pub(super) struct Construction {
    kind: Kind,
    // Exact document text preserves integral floating point constructor values.
    inputs: Vec<Arc<[u8]>>,
}
impl Construction {
    pub fn resolved(document: &[u8], firmware: &[u8]) -> Self {
        Self {
            kind: Kind::Resolved,
            inputs: vec![document.into(), firmware.into()],
        }
    }
    pub fn camera(document: &[u8], firmware: &[u8]) -> Self {
        Self {
            kind: Kind::Camera,
            inputs: vec![document.into(), firmware.into()],
        }
    }
    pub fn prepared(
        document: &[u8],
        update: &[u8],
        rom: &[u8],
        nand: Option<&[u8]>,
        settings: Option<&[u8]>,
        camera: &[u8],
    ) -> Self {
        Self {
            kind: Kind::Prepared,
            inputs: vec![
                document.into(),
                update.into(),
                rom.into(),
                nand.unwrap_or_default().into(),
                settings.unwrap_or_default().into(),
                camera.into(),
            ],
        }
    }
    fn encode(&self) -> Result<Encoded, String> {
        Ok(Encoded::encode("wasm-constructor-v1", &self.kind)?.with_blobs(self.inputs.clone()))
    }
    fn open(encoded: &Encoded, policy: ExecutionPolicy) -> Result<Session, String> {
        let kind: Kind = encoded.decode("wasm-constructor-v1")?;
        if !encoded.children().is_empty()
            || encoded.blobs().len()
                != match kind {
                    Kind::Prepared => 6,
                    _ => 2,
                }
        {
            return Err("snapshot constructor input count differs".into());
        }
        let b = encoded.blobs();
        let mut session = match kind {
            Kind::Resolved => Session::create(&b[0], &b[1], policy),
            Kind::Camera => Session::create_camera(&b[0], &b[1], policy),
            Kind::Prepared => Session::create_prepared(
                &b[0],
                b[1].to_vec(),
                b[2].to_vec(),
                (!b[3].is_empty()).then(|| b[3].to_vec()),
                (!b[4].is_empty()).then(|| b[4].to_vec()),
                &b[5],
                policy,
            ),
        }?;
        session.construction = Some(Self {
            kind,
            inputs: b.to_vec(),
        });
        Ok(session)
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    policy: String,
    powered_on: bool,
    generation: u64,
    quantum: usize,
    observations: Vec<(usize, Vec<u64>)>,
    frames: Vec<Image>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Image {
    output: usize,
    width: u32,
    height: u32,
    sequence: u64,
}
impl Session {
    pub(super) fn encode_point(
        &self,
        saved: &Snapshot,
        presentation: &Presentation,
    ) -> Result<Vec<u8>, String> {
        let construction = self
            .construction
            .as_ref()
            .ok_or("snapshot has no constructor inputs")?;
        let mut images = Vec::new();
        let mut pixels = Vec::new();
        for (output, frame) in &presentation.frames {
            if *output != frame.output {
                return Err("snapshot frame output differs".into());
            }
            images.push(Image {
                output: *output,
                width: frame.width,
                height: frame.height,
                sequence: frame.sequence,
            });
            pixels.push(Arc::from(frame.rgba.as_slice()));
        }
        let header = Header {
            policy: saved.execution_policy().name().into(),
            powered_on: presentation.powered_on,
            generation: self.generation,
            quantum: self.quantum,
            observations: saved.observations().to_vec(),
            frames: images,
        };
        Encoded::encode(KIND, &header)?
            .with_blobs(pixels)
            .with_children(vec![construction.encode()?, saved.export_checkpoint()?])
            .to_bytes()
    }
    pub fn export_snapshot(&self) -> Result<SessionSnapshot, String> {
        let saved = self.core.checkpoint(None)?;
        let snapshot = self.encode_point(&saved, &self.presentation())?;
        let point = Point::new(saved.now(), saved.instruction_counts());
        Ok(SessionSnapshot {
            snapshot,
            tick: point.tick,
            instructions: point.instructions,
        })
    }
    pub fn load_snapshot(bytes: &[u8]) -> Result<Self, String> {
        let encoded = Encoded::from_bytes(bytes)?;
        let header: Header = encoded.decode(KIND)?;
        if encoded.children().len() != 2
            || encoded.blobs().len() != header.frames.len()
            || header.quantum == 0
        {
            return Err("snapshot session child/blob count or quantum differs".into());
        }
        let policy = ExecutionPolicy::parse(&header.policy)?;
        let mut session = Construction::open(&encoded.children()[0], policy)?;
        // Passive hooks can be installed after construction. Preserve their
        // batch/core ordering, which is part of board checkpoint topology.
        let current = session.core.board.observation_topology();
        if !header.observations.starts_with(&current) {
            return Err("snapshot observation wiring differs".into());
        }
        let remaining = &header.observations[current.len()..];
        let cores = session.core.board.instruction_counts().len();
        if cores == 0 || remaining.len() % cores != 0 {
            return Err("snapshot observation core count differs".into());
        }
        for batch in remaining.chunks_exact(cores) {
            if batch
                .iter()
                .enumerate()
                .any(|(core, item)| item.0 != core || item.1 != batch[0].1)
            {
                return Err("snapshot observation batch differs".into());
            }
            session.core.board.watch_pcs(&batch[0].1)?;
        }
        let mut frames = BTreeMap::new();
        for (image, rgba) in header.frames.into_iter().zip(encoded.blobs()) {
            if (image.width as usize)
                .checked_mul(image.height as usize)
                .and_then(|n| n.checked_mul(4))
                != Some(rgba.len())
                || frames.contains_key(&image.output)
            {
                return Err("snapshot presentation frame extent/output differs".into());
            }
            frames.insert(
                image.output,
                Arc::new(Frame {
                    output: image.output,
                    width: image.width,
                    height: image.height,
                    sequence: image.sequence,
                    rgba: rgba.to_vec(),
                }),
            );
        }
        let point = session.core.import_checkpoint(&encoded.children()[1])?;
        session.core.restore(&point)?;
        session.frames = frames;
        session.powered_on = header.powered_on;
        session.generation = header.generation;
        session.quantum = header.quantum;
        session.restored();
        Ok(session)
    }
}
