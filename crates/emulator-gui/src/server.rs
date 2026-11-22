use crate::RunHost;
use crate::protocol::{Event, Request, command_args};
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

/// Address a desktop webview joins for commands and LCD frames.
#[derive(Clone, Debug)]
pub struct Endpoint {
    pub url: String,
}

/// One native camera prepared to serve its frontend.
pub struct Host {
    endpoint: Endpoint,
    listener: TcpListener,
    run: RunHost,
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

    pub fn serve(self, run: RunHost) -> Host {
        Host {
            endpoint: self.endpoint,
            listener: self.listener,
            run,
        }
    }
}

impl Host {
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// Run the camera until its frontend closes the socket.
    pub fn run(self) -> Result<Value, String> {
        let stop = AtomicBool::new(false);
        self.run_until(&stop)
    }

    pub fn run_until(self, stop: &AtomicBool) -> Result<Value, String> {
        GuiSession::open(self.run, self.listener, stop)?.run(stop)
    }
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

struct GuiSession {
    configuration_id: String,
    body: String,
    firmware: String,
    controls: Value,
    run: RunHost,
    socket: WebSocket<TcpStream>,
    frame_number: u64,
    last_rgba: Option<Vec<u8>>,
    last_state: Option<String>,
}

impl GuiSession {
    fn open(run: RunHost, listener: TcpListener, stop: &AtomicBool) -> Result<Self, String> {
        let socket = accept(&listener, stop)?;
        let state = run.session().control_state()?;
        let mut session = Self {
            configuration_id: run.configuration_id().to_owned(),
            body: run.body_id().to_owned(),
            firmware: run.firmware_id().to_owned(),
            controls: serde_json::to_value(&state.inputs).map_err(|error| error.to_string())?,
            run,
            socket,
            frame_number: 0,
            last_rgba: None,
            last_state: None,
        };
        session.publish_catalog()?;
        session.publish_session()?;
        session.publish_latest()?;
        Ok(session)
    }

    fn publish_session(&mut self) -> Result<(), String> {
        let state = self.run.session().control_state()?;
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

    fn publish_catalog(&mut self) -> Result<(), String> {
        self.send_json(Event::Catalog {
            configuration: self.configuration_id.clone(),
            body: self.body.clone(),
            model: self.firmware.clone(),
            firmware: self.firmware.clone(),
            controls: self.controls.clone(),
        })
    }

    fn run(mut self, stop: &AtomicBool) -> Result<Value, String> {
        loop {
            if stop.load(Ordering::Acquire) {
                return self.finish();
            }
            for _ in 0..MAX_MESSAGES_PER_TICK {
                let message = match self.socket.read() {
                    Ok(message) => message,
                    Err(tungstenite::Error::Io(error)) if error.kind() == ErrorKind::WouldBlock => {
                        break;
                    }
                    Err(tungstenite::Error::ConnectionClosed) => {
                        return self.finish();
                    }
                    Err(tungstenite::Error::Protocol(
                        tungstenite::error::ProtocolError::ResetWithoutClosingHandshake,
                    )) => return self.finish(),
                    Err(error) => return Err(error.to_string()),
                };
                match message {
                    Message::Text(text) => self.receive(text.as_str())?,
                    Message::Close(_) => return self.finish(),
                    Message::Ping(_)
                    | Message::Pong(_)
                    | Message::Binary(_)
                    | Message::Frame(_) => {}
                }
            }
            self.advance()?;
            self.publish()?;
            self.publish_session()?;
        }
    }

    fn finish(self) -> Result<Value, String> {
        self.run.finish()
    }

    fn receive(&mut self, source: &str) -> Result<(), String> {
        let Request::Rpc {
            serial,
            action,
            args,
        } = serde_json::from_str(source).map_err(|error| error.to_string())?;
        let event = self
            .command(&action, command_args(args)?)
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

    fn advance(&mut self) -> Result<(), String> {
        self.run.step()
    }

    fn publish(&mut self) -> Result<(), String> {
        let frame = self
            .run
            .take_frames()
            .into_iter()
            .max_by_key(|frame| frame.sequence);
        if let Some(frame) = frame.as_ref() {
            self.publish_frame(frame)?;
        }
        Ok(())
    }

    fn publish_frame(
        &mut self,
        frame: &fujimod_emulator_devices::graphics::scanout::Frame,
    ) -> Result<(), String> {
        if self.last_rgba.as_deref() == Some(frame.rgba.as_slice()) {
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
        self.last_rgba = Some(frame.rgba.clone());
        Ok(())
    }

    /// Send the held LCD image after a frontend joins a running session.
    fn publish_latest(&mut self) -> Result<(), String> {
        if let Some(frame) = self.run.latest_frame() {
            self.publish_frame(&frame)?;
        }
        Ok(())
    }

    fn command(&mut self, action: &str, args: Value) -> Result<Value, String> {
        match action {
            "boot" => {
                self.publish_session()?;
                let state = self.run.session().control_state()?;
                serde_json::to_value(state.inputs).map_err(|error| error.to_string())
            }
            "press" => {
                let id = string(&args, "id")?;
                let stage = args.get("stage").and_then(Value::as_str);
                self.run.session_mut().contact_input(id, stage, false)?;
                Ok(Value::Null)
            }
            "release" => {
                let id = string(&args, "id")?;
                self.run.session_mut().contact_input(id, None, true)?;
                Ok(Value::Null)
            }
            "set_position" => {
                let (id, position) = (string(&args, "id")?, string(&args, "position")?);
                self.run.session_mut().set_position(id, position)?;
                Ok(Value::Null)
            }
            "rotate" => {
                let steps = i32::try_from(integer(&args, "steps")?)
                    .map_err(|_| "rotate steps exceed i32".to_owned())?;
                let session = self.run.session_mut();
                let (node, edges) = session.rotation(string(&args, "id")?, steps)?;
                session.rotate_edges(&node, edges)?;
                Ok(Value::Null)
            }
            "touch" => {
                let x = u16::try_from(integer(&args, "x")?)
                    .map_err(|_| "touch x exceeds 16 bits".to_owned())?;
                let y = u16::try_from(integer(&args, "y")?)
                    .map_err(|_| "touch y exceeds 16 bits".to_owned())?;
                let (node, script) = self.run.session().touch_input(
                    string(&args, "id")?,
                    string(&args, "phase")?,
                    x,
                    y,
                )?;
                self.run.session_mut().touch(&node, &script)?;
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
