use crate::protocol::{Event, Request, command_args};
use crate::usb::UsbShare;
use fujimod_emulator_devices::graphics::scanout::Frame;
use fujimod_emulator_session::Session;
use fujimod_usb::share_owner::DeviceShare;
use fujimod_webrtc::{HostEvent, LoopbackRoom, PeerSocket, VideoFrame, VideoSender};
use openh264::encoder::{
    BitRate, Encoder, EncoderConfig, FrameRate, IntraFramePeriod, Profile, RateControlMode,
    UsageType,
};
use openh264::formats::{RgbaSliceU8, YUVBuffer};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

const MAX_MESSAGES_PER_TICK: usize = 64;
/// Video target for one controller.
const VIDEO_BITS_PER_SECOND: u32 = 3_000_000;
/// A keyframe at least this often lets a controller recover from loss.
const KEYFRAME_INTERVAL: Duration = Duration::from_secs(1);

/// A camera session a desktop window drives: any processor, any host.
pub trait Run {
    fn session(&self) -> &Session;
    fn session_mut(&mut self) -> &mut Session;
    /// Advance the guest by one bounded slice.
    fn step(&mut self) -> Result<(), String>;
    /// Advance the guest about a millisecond while a host waits on a device.
    fn settle(&mut self) -> Result<(), String>;
    /// LCD images shown since the last call.
    fn frames(&mut self) -> Result<Vec<Frame>, String>;
    /// End the run and report it.
    fn finish(self) -> Result<Value, String>
    where
        Self: Sized;
}

/// The share link a controller opens to join a camera's room.
#[derive(Clone, Debug)]
pub struct Endpoint {
    pub url: String,
}

/// The share room a camera is served in, known before the camera is opened.
pub struct Room {
    endpoint: Endpoint,
    events: mpsc::Receiver<HostEvent>,
    _room: Arc<LoopbackRoom>,
}

impl Room {
    /// A room on this machine for the desktop window. Its link names no
    /// site: the window reads only the ticket in its fragment.
    pub fn loopback() -> Result<Self, String> {
        let room =
            Arc::new(LoopbackRoom::start("http://localhost").map_err(|error| error.to_string())?);
        let (send, events) = mpsc::channel();
        let watched = Arc::clone(&room);
        std::thread::Builder::new()
            .name("fujimod-viewer-room".into())
            .spawn(move || {
                while let Ok(event) = futures_lite::future::block_on(watched.room().next_event()) {
                    if send.send(event).is_err() {
                        break;
                    }
                }
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            endpoint: Endpoint {
                url: room.link().to_owned(),
            },
            events,
            _room: room,
        })
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// Wait for the first controller to join. `abandoned` gives up the wait.
    pub fn join(self, abandoned: &mut dyn FnMut() -> bool) -> Result<Viewer, String> {
        let mut viewer = Viewer {
            room: self,
            joining: Vec::new(),
            usb: None,
            usb_controllers: BTreeSet::new(),
            controllers: Vec::new(),
            holds: BTreeMap::new(),
            latest: None,
            sequence: 0,
            catalog: None,
            pending_releases: Vec::new(),
        };
        while viewer.joining.is_empty() {
            if abandoned() {
                return Err("desktop window closed".into());
            }
            match viewer.room.events.recv_timeout(Duration::from_millis(10)) {
                Ok(event) => viewer.event(event),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("share room closed".into());
                }
            }
        }
        Ok(viewer)
    }
}

/// Serve `run` in its room while its host is `open` (for a desktop window,
/// [`crate::launch::Window::open`]); controllers joining and leaving do not end the run.
pub fn serve<R: Run>(
    mut run: R,
    room: Room,
    open: &mut dyn FnMut() -> bool,
) -> Result<Value, String> {
    let mut viewer = room.join(&mut || !open())?;
    while open() {
        viewer.service(&mut run)?;
        run.step()?;
        let frames = run.frames()?;
        viewer.show(&frames)?;
    }
    run.finish()
}

fn identity(session: &Session, key: &str) -> String {
    session.definition[key]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

/// One controller's video: its track and its own encoder, so a controller
/// that joins late starts from a keyframe.
struct Video {
    sender: VideoSender,
    encoder: Encoder,
    /// Sequence of the last frame encoded for this controller.
    sent: u64,
    sent_at: Instant,
    keyframe_at: Option<Instant>,
}

struct Controller {
    id: String,
    socket: PeerSocket,
    video: Option<Video>,
    catalog_sent: bool,
    last_state: Option<String>,
}

/// The controllers joined to a camera's room. It never owns the session: its
/// host calls [`Viewer::service`] between guest slices and [`Viewer::show`]
/// with new images.
pub struct Viewer {
    room: Room,
    /// Controllers whose first message has not arrived; it names their protocol.
    joining: Vec<(String, PeerSocket)>,
    /// The camera's USB port, shared once a controller asks for it.
    usb: Option<UsbShare>,
    /// Controllers speaking the USB share protocol.
    usb_controllers: BTreeSet<String>,
    controllers: Vec<Controller>,
    /// For each held button, the controllers holding it.
    holds: BTreeMap<String, BTreeSet<String>>,
    /// The newest LCD image, replaced by each changed one.
    latest: Option<Frame>,
    sequence: u64,
    catalog: Option<String>,
    /// Buttons whose last holder left, released before the next command.
    pending_releases: Vec<String>,
}

impl Viewer {
    fn event(&mut self, event: HostEvent) {
        match event {
            HostEvent::Controller(id, socket) => self.joining.push((id, socket)),
            HostEvent::Left(id) => self.leave(&id),
        }
    }

    /// Admit a controller whose first message was `first`: the USB share
    /// protocol goes to the camera's USB port, anything else is the viewer's.
    fn admit(
        &mut self,
        run: &mut dyn Run,
        id: String,
        socket: PeerSocket,
        first: Vec<u8>,
    ) -> Result<(), String> {
        if DeviceShare::opens(&first) {
            if self.usb.is_none() {
                match UsbShare::connect(run, &self.room.endpoint.url) {
                    Ok(usb) => self.usb = Some(usb),
                    Err(error) => {
                        eprintln!("emulated USB port unavailable: {error}");
                        return Ok(());
                    }
                }
            }
            let usb = self.usb.as_ref().expect("USB port shared");
            usb.share
                .serve(id.clone(), socket, first)
                .map_err(|error| error.to_string())?;
            self.usb_controllers.insert(id);
            return Ok(());
        }
        let video = socket.video().map(|sender| {
            Ok::<_, String>(Video {
                sender,
                encoder: encoder()?,
                sent: 0,
                sent_at: Instant::now(),
                keyframe_at: None,
            })
        });
        let video = video.transpose()?;
        self.controllers.push(Controller {
            id,
            socket,
            video,
            catalog_sent: false,
            last_state: None,
        });
        let index = self.controllers.len() - 1;
        if let Some(event) = self.receive(run, index, &first) {
            self.send(index, event);
        }
        Ok(())
    }

    /// A controller has gone: drop it and let go of every button it held.
    fn leave(&mut self, id: &str) {
        self.joining.retain(|(joining, _)| joining != id);
        self.usb_controllers.remove(id);
        self.controllers.retain(|controller| controller.id != id);
        let held: Vec<String> = self
            .holds
            .iter_mut()
            .filter_map(|(control, holders)| {
                (holders.remove(id) && holders.is_empty()).then(|| control.clone())
            })
            .collect();
        self.pending_releases.extend(held);
    }

    /// The control catalog, once the session's controls are connected.
    fn catalog(&mut self, session: &Session) -> Result<Option<String>, String> {
        if self.catalog.is_none() && session.control_state().is_ok() {
            let firmware = identity(session, "firmware_id");
            let selection = fujimod_catalog::reference::ReferenceCatalog::bundled()?
                .resolve_firmware(&firmware)?;
            let controls = selection
                .physical_controls
                .ok_or_else(|| format!("{} has no physical controls catalog", selection.body.id))?;
            self.catalog = Some(
                Event::Catalog {
                    configuration: identity(session, "configuration_id"),
                    model: selection.body.id.to_uppercase(),
                    body: selection.body.id,
                    firmware,
                    controls,
                }
                .json()?,
            );
        }
        Ok(self.catalog.clone())
    }

    /// Apply the controllers' pending commands to `run` and send them the
    /// control state.
    pub fn service(&mut self, run: &mut dyn Run) -> Result<(), String> {
        while let Ok(event) = self.room.events.try_recv() {
            self.event(event);
        }
        for control in std::mem::take(&mut self.pending_releases) {
            run.session_mut().contact_input(&control, None, true)?;
        }
        for (id, mut socket) in std::mem::take(&mut self.joining) {
            match socket.try_receive() {
                None => self.joining.push((id, socket)),
                Some(Ok(first)) => self.admit(run, id, socket, first)?,
                Some(Err(_)) => {}
            }
        }
        if let Some(usb) = &mut self.usb {
            usb.service(run);
        }
        let mut gone = Vec::new();
        for index in 0..self.controllers.len() {
            for _ in 0..MAX_MESSAGES_PER_TICK {
                match self.controllers[index].socket.try_receive() {
                    None => break,
                    Some(Ok(bytes)) => {
                        if let Some(event) = self.receive(run, index, &bytes) {
                            self.send(index, event);
                        }
                    }
                    Some(Err(_)) => {
                        gone.push(self.controllers[index].id.clone());
                        break;
                    }
                }
            }
        }
        for id in gone {
            self.leave(&id);
        }
        for control in std::mem::take(&mut self.pending_releases) {
            run.session_mut().contact_input(&control, None, true)?;
        }
        self.publish_state(run.session());
        Ok(())
    }

    /// Keep the newest of `frames` and encode it for each controller whose
    /// track has sent the previous frame.
    pub fn show(&mut self, frames: &[Frame]) -> Result<(), String> {
        if let Some(frame) = frames.iter().max_by_key(|frame| frame.sequence)
            && self
                .latest
                .as_ref()
                .is_none_or(|latest| latest.rgba != frame.rgba)
        {
            self.latest = Some(frame.clone());
            self.sequence += 1;
        }
        let Some(frame) = &self.latest else {
            return Ok(());
        };
        for controller in &mut self.controllers {
            let Some(video) = &mut controller.video else {
                continue;
            };
            if video.sent == self.sequence && video.keyframe_at.is_some() {
                continue;
            }
            if !video.sender.wants_frame() {
                continue;
            }
            let now = Instant::now();
            if video
                .keyframe_at
                .is_none_or(|at| now.duration_since(at) >= KEYFRAME_INTERVAL)
            {
                video.encoder.force_intra_frame();
                video.keyframe_at = Some(now);
            }
            let data = encode(&mut video.encoder, frame)?;
            let duration = now.duration_since(video.sent_at);
            video.sent = self.sequence;
            video.sent_at = now;
            // A closed track is reported when the controller leaves.
            let _ = video.sender.send(VideoFrame { data, duration });
        }
        Ok(())
    }

    fn publish_state(&mut self, session: &Session) {
        let Ok(Some(catalog)) = self.catalog(session) else {
            return;
        };
        let Ok(state) = session.control_state() else {
            return;
        };
        let Ok(serialized) = (Event::State {
            // A native session has no cold power-off; the firmware reads the power switch.
            powered_on: true,
            inputs: state.inputs,
            positions: state.positions,
        })
        .json() else {
            return;
        };
        for index in 0..self.controllers.len() {
            if !self.controllers[index].catalog_sent {
                self.send_text(index, catalog.clone());
                self.controllers[index].catalog_sent = true;
            }
            if self.controllers[index].last_state.as_deref() != Some(&serialized) {
                self.send_text(index, serialized.clone());
                self.controllers[index].last_state = Some(serialized.clone());
            }
        }
    }

    fn receive(&mut self, run: &mut dyn Run, index: usize, bytes: &[u8]) -> Option<Event> {
        let request = match serde_json::from_slice(bytes) {
            Ok(request) => request,
            Err(error) => return Some(Event::error(None, error.to_string())),
        };
        let (serial, action, args) = match request {
            Request::Hello => {
                self.publish_state(run.session());
                return None;
            }
            Request::Rpc {
                serial,
                action,
                args,
            } => (serial, action, args),
        };
        let id = self.controllers[index].id.clone();
        Some(
            command_args(args)
                .and_then(|args| self.command(run.session_mut(), &id, &action, args))
                .map(|value| Event::reply(serial, value))
                .unwrap_or_else(|error| Event::error(Some(serial), error)),
        )
    }

    fn send(&mut self, index: usize, event: Event) {
        if let Ok(text) = event.json() {
            self.send_text(index, text);
        }
    }

    fn send_text(&mut self, index: usize, text: String) {
        // A controller that has gone is dropped by the next service.
        let _ =
            futures_lite::future::block_on(self.controllers[index].socket.send(text.into_bytes()));
    }

    /// One controller's command. A button is pressed while any controller
    /// holds it; a dial or switch takes the most recent position.
    fn command(
        &mut self,
        session: &mut Session,
        controller: &str,
        action: &str,
        args: Value,
    ) -> Result<Value, String> {
        match action {
            "boot" => {
                let state = session.control_state()?;
                serde_json::to_value(state.inputs).map_err(|error| error.to_string())
            }
            "press" => {
                let id = string(&args, "id")?;
                let stage = args.get("stage").and_then(Value::as_str);
                let holders = self.holds.entry(id.to_owned()).or_default();
                // A staged press (half, then full) always reaches the session.
                if holders.is_empty() || stage.is_some() {
                    session.contact_input(id, stage, false)?;
                }
                holders.insert(controller.to_owned());
                Ok(Value::Null)
            }
            "release" => {
                let id = string(&args, "id")?;
                let holders = self.holds.entry(id.to_owned()).or_default();
                holders.remove(controller);
                if holders.is_empty() {
                    self.holds.remove(id);
                    session.contact_input(id, None, true)?;
                }
                Ok(Value::Null)
            }
            "set_position" => {
                session.set_position(string(&args, "id")?, string(&args, "position")?)?;
                Ok(Value::Null)
            }
            "rotate" => {
                let steps = i32::try_from(integer(&args, "steps")?)
                    .map_err(|_| "rotate steps exceed i32".to_owned())?;
                let (node, edges) = session.rotation(string(&args, "id")?, steps)?;
                session.rotate_edges(&node, edges)?;
                Ok(Value::Null)
            }
            "touch" => {
                let x = u16::try_from(integer(&args, "x")?)
                    .map_err(|_| "touch x exceeds 16 bits".to_owned())?;
                let y = u16::try_from(integer(&args, "y")?)
                    .map_err(|_| "touch y exceeds 16 bits".to_owned())?;
                let (node, script) =
                    session.touch_input(string(&args, "id")?, string(&args, "phase")?, x, y)?;
                session.touch(&node, &script)?;
                Ok(Value::Null)
            }
            "close" => Ok(Value::Null),
            other => Err(format!("unknown native command: {other}")),
        }
    }
}

fn string<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{key} must be a string"))
}

fn integer(args: &Value, key: &str) -> Result<i64, String> {
    args.get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("{key} must be an integer"))
}

/// Screen content at a fixed bitrate: Constrained Baseline, no B-frames, and
/// keyframes only when the viewer forces them.
fn encoder() -> Result<Encoder, String> {
    Encoder::with_api_config(
        openh264::OpenH264API::from_source(),
        EncoderConfig::new()
            .profile(Profile::Baseline)
            .usage_type(UsageType::ScreenContentRealTime)
            .rate_control_mode(RateControlMode::Bitrate)
            .bitrate(BitRate::from_bps(VIDEO_BITS_PER_SECOND))
            .max_frame_rate(FrameRate::from_hz(60.0))
            .intra_frame_period(IntraFramePeriod::from_num_frames(u32::MAX))
            .skip_frames(false),
    )
    .map_err(|error| error.to_string())
}

/// Encode one image; H.264 needs even dimensions, so an odd edge row or
/// column is left out.
fn encode(encoder: &mut Encoder, frame: &Frame) -> Result<Vec<u8>, String> {
    let (width, height) = (frame.width as usize & !1, frame.height as usize & !1);
    let stride = frame.width as usize * 4;
    let rgba: std::borrow::Cow<'_, [u8]> = if width * 4 == stride {
        std::borrow::Cow::Borrowed(&frame.rgba[..stride * height])
    } else {
        std::borrow::Cow::Owned(
            frame
                .rgba
                .chunks_exact(stride)
                .take(height)
                .flat_map(|row| &row[..width * 4])
                .copied()
                .collect(),
        )
    };
    let yuv = YUVBuffer::from_rgb_source(RgbaSliceU8::new(&rgba, (width, height)));
    let stream = encoder.encode(&yuv).map_err(|error| error.to_string())?;
    Ok(stream.to_vec())
}
