use crate::Xp5Session;
use crate::controls::Xp5Controls;
use crate::protocol::{Event, GuiOptions, Request, command_args};
use fujimod_emulator_session::Session;
use serde_json::{Value, json};
use std::{
    io::ErrorKind,
    net::{IpAddr, TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tungstenite::{
    Message, WebSocket,
    handshake::server::{Request as HttpRequest, Response},
};

/// Address a desktop webview joins for commands and LCD frames.
#[derive(Clone, Debug)]
pub struct Endpoint {
    pub url: String,
}

/// One native camera prepared to serve its frontend.
pub struct Host {
    endpoint: Endpoint,
    listener: TcpListener,
    camera: Xp5Session,
    options: GuiOptions,
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

    pub fn serve(self, camera: Xp5Session, options: GuiOptions) -> Host {
        Host {
            endpoint: self.endpoint,
            listener: self.listener,
            camera,
            options,
        }
    }

    /// Connect a camera before returning the host that serves it.
    pub fn open(self, camera: Xp5Session, options: GuiOptions, stop: &AtomicBool) -> Result<Host, String> {
        accept(&self.listener, stop)?;
        Ok(self.serve(camera, options))
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
        GuiSession::open(self.camera, self.options, self.listener, stop)?.run(stop)
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

/// Bind a loopback endpoint and serve one native camera.
pub fn serve(camera: Xp5Session, options: GuiOptions, address: IpAddr) -> Result<Host, String> {
    Ok(bind(address)?.serve(camera, options))
}

/// Open a camera and reserve its endpoint before the caller launches a frontend.
pub fn open(
    camera: Xp5Session,
    options: GuiOptions,
    address: IpAddr,
    stop: &AtomicBool,
) -> Result<Host, String> {
    bind(address)?.open(camera, options, stop)
}

fn accept(listener: &TcpListener, stop: &AtomicBool) -> Result<WebSocket<TcpStream>, String> {
    loop {
        if stop.load(Ordering::Acquire) {
            return Err("desktop window closed".into());
        }
        match listener.accept() {
            Ok((stream, _)) => {
                stream
                    .set_nonblocking(false)
                    .map_err(|error| error.to_string())?;
                stream
                    .set_read_timeout(Some(Duration::from_millis(2)))
                    .map_err(|error| error.to_string())?;
                let check = |request: &HttpRequest, response: Response| {
                    if request.uri().path() == "/emulator" {
                        Ok(response)
                    } else {
                        let mut refusal = tungstenite::handshake::server::ErrorResponse::new(None);
                        *refusal.status_mut() = tungstenite::http::StatusCode::NOT_FOUND;
                        Err(refusal)
                    }
                };
                return tungstenite::accept_hdr(stream, check)
                    .map_err(|error| error.to_string());
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
    physical: Xp5Controls,
    session: Session,
    inputs: fujimod_emulator_xp5_board::session::Inputs,
    quantum: usize,
    socket: WebSocket<TcpStream>,
    frame_number: u64,
    last_frame: Option<Vec<u8>>,
    last_state: Option<String>,
}

impl GuiSession {
    fn open(
        camera: Xp5Session,
        options: GuiOptions,
        listener: TcpListener,
        stop: &AtomicBool,
    ) -> Result<Self, String> {
        let Xp5Session {
            configuration,
            mut session,
            inputs,
            quantum,
            ..
        } = camera;
        if let Some(path) = options.load_snapshot {
            fujimod_emulator_boot::load_snapshot(&mut session, &path)?;
        }
        session.board.set_execution_policy(options.execution_policy)?;
        let physical = Xp5Controls::new(&configuration)?;
        let socket = accept(&listener, stop)?;
        let mut session = Self {
            configuration_id: configuration.configuration.id,
            body: configuration.configuration.body_id,
            firmware: configuration.configuration.firmware_id,
            controls: physical.document()?,
            physical,
            session,
            inputs,
            quantum,
            socket,
            frame_number: 0,
            last_frame: None,
            last_state: None,
        };
        session.publish_catalog()?;
        session.publish_session()?;
        Ok(session)
    }

    fn publish_session(&mut self) -> Result<(), String> {
        let event = Event::State {
            powered_on: self.physical.powered(),
            inputs: self.physical.catalog(),
            positions: self.physical.positions(),
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

    fn run(&mut self, stop: &AtomicBool) -> Result<Value, String> {
        loop {
            if stop.load(Ordering::Acquire) {
                return Ok(json!({"status":"closed"}));
            }
            loop {
                let message = match self.socket.read() {
                    Ok(message) => message,
                    Err(tungstenite::Error::Io(error))
                        if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
                    {
                        break;
                    }
                    Err(tungstenite::Error::ConnectionClosed) => {
                        return Ok(json!({"status":"closed"}));
                    }
                    Err(error) => return Err(error.to_string()),
                };
                match message {
                    Message::Text(text) => self.receive(text.as_str())?,
                    Message::Close(_) => return Ok(json!({"status":"closed"})),
                    Message::Ping(_) | Message::Pong(_) | Message::Binary(_) | Message::Frame(_) => {}
                }
            }
            self.advance()?;
            self.publish()?;
            self.publish_session()?;
        }
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
        self.session
            .run_with_limits(1_000_000, self.quantum, None, None)
    }

    fn publish(&mut self) -> Result<(), String> {
        for _line in self.session.board.take_console() {}
        let Some(frame) = self
            .session
            .board
            .take_frames()
            .into_iter()
            .max_by_key(|frame| frame.sequence)
        else {
            return Ok(());
        };
        let jpeg = encode_jpeg(&frame)?;
        if self.last_frame.as_ref() == Some(&jpeg) {
            return Ok(());
        }
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
        self.last_frame = Some(jpeg);
        Ok(())
    }

    fn command(&mut self, action: &str, args: Value) -> Result<Value, String> {
        match action {
            "boot" => {
                self.publish_session()?;
                serde_json::to_value(self.physical.catalog())
                    .map_err(|error| error.to_string())
            }
            "press" => {
                let id = string(&args, "id")?;
                let stage = args.get("stage").and_then(Value::as_str);
                self.physical.press(&mut self.session, &self.inputs, id, stage)?;
                Ok(Value::Null)
            }
            "release" => {
                self.physical
                    .release(&mut self.session, &self.inputs, string(&args, "id")?)?;
                Ok(Value::Null)
            }
            "set_position" => {
                self.physical.set_position(
                    &mut self.session,
                    &self.inputs,
                    string(&args, "id")?,
                    string(&args, "position")?,
                )?;
                Ok(Value::Null)
            }
            "rotate" => {
                let steps = integer(&args, "steps")?;
                self.physical.rotate(
                    &mut self.session,
                    &self.inputs,
                    string(&args, "id")?,
                    i32::try_from(steps).map_err(|_| "rotate steps exceed i32".to_owned())?,
                )?;
                Ok(Value::Null)
            }
            "touch" => {
                let x = u16::try_from(integer(&args, "x")?)
                    .map_err(|_| "touch x exceeds 16 bits".to_owned())?;
                let y = u16::try_from(integer(&args, "y")?)
                    .map_err(|_| "touch y exceeds 16 bits".to_owned())?;
                let (node, script) = self.session.touch_input(
                    string(&args, "id")?,
                    string(&args, "phase")?,
                    x,
                    y,
                )?;
                self.session.touch(&node, &script)?;
                Ok(Value::Null)
            }
            "power" => {
                let on = args.get("on").and_then(Value::as_bool).ok_or("power needs on")?;
                self.physical.powered_switch(&mut self.session, &self.inputs, on)?;
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
