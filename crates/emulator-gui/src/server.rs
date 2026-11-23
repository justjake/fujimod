use crate::protocol::{Event, Request, command_args};
use fujimod_emulator_devices::graphics::scanout::Frame;
use fujimod_emulator_session::Session;
use serde_json::Value;
use std::{
    io::ErrorKind,
    net::{IpAddr, TcpListener, TcpStream},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tungstenite::{
    Message, WebSocket,
    handshake::server::{Request as HttpRequest, Response},
};

const MAX_MESSAGES_PER_TICK: usize = 64;

/// A camera session a desktop window drives: any processor, any host.
pub trait Run {
    fn session(&self) -> &Session;
    fn session_mut(&mut self) -> &mut Session;
    /// Advance the guest by one bounded slice.
    fn step(&mut self) -> Result<(), String>;
    /// LCD images shown since the last call.
    fn frames(&mut self) -> Result<Vec<Frame>, String>;
    /// End the run and report it.
    fn finish(self) -> Result<Value, String>
    where
        Self: Sized;
}

/// Address a desktop webview joins for commands and LCD frames.
#[derive(Clone, Debug)]
pub struct Endpoint {
    pub url: String,
}

/// A listener whose endpoint is known before the camera session is opened.
pub struct Socket {
    endpoint: Endpoint,
    listener: TcpListener,
}

impl Socket {
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// Wait for the desktop window to join, then attach it to `run`.
    pub fn join(self, run: &dyn Run, stop: &AtomicBool) -> Result<Viewer, String> {
        Viewer::open(accept(&self.listener, stop)?, run.session())
    }
}

/// Serve `run` to its desktop window until the window closes or `stop` is set.
pub fn serve<R: Run>(mut run: R, socket: Socket, stop: &AtomicBool) -> Result<Value, String> {
    let mut viewer = socket.join(&run, stop)?;
    while !stop.load(Ordering::Acquire) && viewer.service(&mut run)? {
        run.step()?;
        let frames = run.frames()?;
        viewer.show(&frames)?;
    }
    run.finish()
}

/// Reserve an endpoint before opening the camera.
pub fn bind(address: IpAddr) -> Result<Socket, String> {
    let listener = TcpListener::bind((address, 0)).map_err(|error| error.to_string())?;
    listener
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    let address = listener.local_addr().map_err(|error| error.to_string())?;
    Ok(Socket {
        endpoint: Endpoint {
            url: format!("ws://{address}/emulator"),
        },
        listener,
    })
}

fn accept(listener: &TcpListener, stop: &AtomicBool) -> Result<WebSocket<TcpStream>, String> {
    loop {
        if stop.load(Ordering::Acquire) {
            return Err("desktop window closed".into());
        }
        match listener.accept() {
            Ok((stream, _)) => {
                let check = |request: &HttpRequest, response: Response| {
                    if request.uri().path() == "/emulator" {
                        Ok(response)
                    } else {
                        let mut refusal = tungstenite::handshake::server::ErrorResponse::new(None);
                        *refusal.status_mut() = tungstenite::http::StatusCode::NOT_FOUND;
                        Err(refusal)
                    }
                };
                let mut socket =
                    tungstenite::accept_hdr(stream, check).map_err(|error| error.to_string())?;
                socket
                    .get_mut()
                    .set_nonblocking(true)
                    .map_err(|error| error.to_string())?;
                return Ok(socket);
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error.to_string()),
        }
    }
}

fn identity(session: &Session, key: &str) -> String {
    session.definition[key].as_str().unwrap_or_default().to_owned()
}

/// One joined desktop window. It never owns the session: its host calls
/// [`Viewer::service`] between guest slices and [`Viewer::show`] with new images.
pub struct Viewer {
    socket: WebSocket<TcpStream>,
    frame_number: u64,
    last: Option<Frame>,
    last_state: Option<String>,
    catalog_sent: bool,
}

impl Viewer {
    fn open(socket: WebSocket<TcpStream>, session: &Session) -> Result<Self, String> {
        let mut viewer = Self {
            socket,
            frame_number: 0,
            last: None,
            last_state: None,
            catalog_sent: false,
        };
        viewer.publish_state(session)?;
        Ok(viewer)
    }

    /// The control catalog, once the session's controls are connected.
    fn publish_catalog(&mut self, session: &Session) -> Result<(), String> {
        let Ok(_) = session.control_state() else {
            return Ok(());
        };
        let firmware = identity(session, "firmware_id");
        let selection = fujimod_catalog::reference::ReferenceCatalog::bundled()?
            .resolve_firmware(&firmware)?;
        let controls = selection.physical_controls.ok_or_else(|| {
            format!("{} has no physical controls catalog", selection.body.id)
        })?;
        self.send_json(Event::Catalog {
            configuration: identity(session, "configuration_id"),
            model: selection.body.id.to_uppercase(),
            body: selection.body.id,
            firmware,
            controls,
        })?;
        self.catalog_sent = true;
        Ok(())
    }

    /// Apply the window's pending commands to `run` and send its control
    /// state. False once the window has closed.
    pub fn service(&mut self, run: &mut dyn Run) -> Result<bool, String> {
        for _ in 0..MAX_MESSAGES_PER_TICK {
            let message = match self.socket.read() {
                Ok(message) => message,
                Err(tungstenite::Error::Io(error)) if error.kind() == ErrorKind::WouldBlock => {
                    break;
                }
                Err(tungstenite::Error::ConnectionClosed)
                | Err(tungstenite::Error::Protocol(
                    tungstenite::error::ProtocolError::ResetWithoutClosingHandshake,
                )) => return Ok(false),
                Err(error) => return Err(error.to_string()),
            };
            match message {
                Message::Text(text) => self.receive(run, text.as_str())?,
                Message::Close(_) => return Ok(false),
                Message::Ping(_) | Message::Pong(_) | Message::Binary(_) | Message::Frame(_) => {}
            }
        }
        self.publish_state(run.session())?;
        Ok(true)
    }

    /// Send the newest of `frames` when it differs from the last image sent.
    pub fn show(&mut self, frames: &[Frame]) -> Result<(), String> {
        let Some(frame) = frames.iter().max_by_key(|frame| frame.sequence) else {
            return Ok(());
        };
        if self.last.as_ref().is_some_and(|last| last.rgba == frame.rgba) {
            return Ok(());
        }
        let jpeg = encode_jpeg(frame)?;
        self.frame_number = self.frame_number.saturating_add(1);
        let mut packet = Vec::with_capacity(20 + jpeg.len());
        packet.extend_from_slice(b"FJMF");
        packet.extend_from_slice(&frame.width.to_be_bytes());
        packet.extend_from_slice(&frame.height.to_be_bytes());
        packet.extend_from_slice(&self.frame_number.to_be_bytes());
        packet.extend_from_slice(&jpeg);
        self.socket
            .send(Message::Binary(packet.into()))
            .map_err(|error| error.to_string())?;
        self.last = Some(frame.clone());
        Ok(())
    }

    fn publish_state(&mut self, session: &Session) -> Result<(), String> {
        if !self.catalog_sent {
            self.publish_catalog(session)?;
            if !self.catalog_sent {
                return Ok(());
            }
        }
        let state = session.control_state()?;
        let event = Event::State {
            // A native session has no cold power-off; the firmware reads the power switch.
            powered_on: true,
            inputs: state.inputs,
            positions: state.positions,
        };
        let serialized = serde_json::to_string(&event).map_err(|error| error.to_string())?;
        if self.last_state.as_deref() != Some(&serialized) {
            self.socket
                .send(Message::Text(serialized.clone().into()))
                .map_err(|error| error.to_string())?;
            self.last_state = Some(serialized);
        }
        Ok(())
    }

    fn receive(&mut self, run: &mut dyn Run, source: &str) -> Result<(), String> {
        let Request::Rpc {
            serial,
            action,
            args,
        } = serde_json::from_str(source).map_err(|error| error.to_string())?;
        let event = command_args(args)
            .and_then(|args| command(run.session_mut(), &action, args))
            .map(|value| Event::reply(serial, value))
            .unwrap_or_else(|error| Event::error(Some(serial), error));
        self.send_json(event)
    }

    fn send_json(&mut self, event: Event) -> Result<(), String> {
        let text = event.json()?;
        self.socket
            .send(Message::Text(text.into()))
            .map_err(|error| error.to_string())
    }
}

fn command(session: &mut Session, action: &str, args: Value) -> Result<Value, String> {
    match action {
        "boot" => {
            let state = session.control_state()?;
            serde_json::to_value(state.inputs).map_err(|error| error.to_string())
        }
        "press" => {
            let id = string(&args, "id")?;
            let stage = args.get("stage").and_then(Value::as_str);
            session.contact_input(id, stage, false)?;
            Ok(Value::Null)
        }
        "release" => {
            session.contact_input(string(&args, "id")?, None, true)?;
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

fn encode_jpeg(
    frame: &fujimod_emulator_devices::graphics::scanout::Frame,
) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    jpeg_encoder::Encoder::new(&mut bytes, 85)
        .encode(
            &frame.rgba,
            u16::try_from(frame.width).map_err(|_| "frame width exceeds JPEG limits")?,
            u16::try_from(frame.height).map_err(|_| "frame height exceeds JPEG limits")?,
            jpeg_encoder::ColorType::Rgba,
        )
        .map_err(|error| error.to_string())?;
    Ok(bytes)
}
