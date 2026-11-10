//! PTP over a USB bulk pipe (PIMA 15740 / USB Still Image Capture Device
//! Definition): little-endian containers of length, type, code and
//! transaction id, a command phase, an optional data phase and a response.
use std::fmt;

pub const RESPONSE_OK: u16 = 0x2001;
pub const SESSION_ALREADY_OPEN: u16 = 0x201e;

pub const GET_DEVICE_INFO: u16 = 0x1001;
pub const OPEN_SESSION: u16 = 0x1002;
pub const CLOSE_SESSION: u16 = 0x1003;
pub const GET_OBJECT_HANDLES: u16 = 0x1007;
pub const GET_OBJECT: u16 = 0x1009;
pub const DELETE_OBJECT: u16 = 0x100b;
pub const GET_DEVICE_PROP_VALUE: u16 = 0x1015;
pub const SET_DEVICE_PROP_VALUE: u16 = 0x1016;

const COMMAND: u16 = 1;
const DATA: u16 = 2;
const RESPONSE: u16 = 3;
const HEADER: usize = 12;
/// Whole packets for every high- and SuperSpeed endpoint size.
const CHUNK: usize = 1 << 20;

/// The bulk endpoint pair of a claimed still-image interface.
#[allow(async_fn_in_trait)]
pub trait Pipe {
    async fn write(&mut self, bytes: &[u8]) -> std::result::Result<(), String>;
    async fn read(&mut self, maximum: usize) -> std::result::Result<Vec<u8>, String>;
    fn packet_size(&self) -> usize;
}

impl Pipe for fujimod_usb::connection::BulkPipe {
    async fn write(&mut self, bytes: &[u8]) -> std::result::Result<(), String> {
        fujimod_usb::connection::BulkPipe::write(self, bytes)
            .await
            .map_err(|e| e.to_string())
    }
    async fn read(&mut self, maximum: usize) -> std::result::Result<Vec<u8>, String> {
        fujimod_usb::connection::BulkPipe::read(self, maximum)
            .await
            .map_err(|e| e.to_string())
    }
    fn packet_size(&self) -> usize {
        fujimod_usb::connection::BulkPipe::packet_size(self)
    }
}

#[derive(Debug)]
pub enum Error {
    Transport(String),
    /// The device completed the operation with a response other than OK.
    Response {
        operation: u16,
        code: u16,
    },
    Protocol(String),
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(e) => write!(f, "USB transfer failed: {e}"),
            Self::Response { operation, code } => {
                write!(f, "operation {operation:#06x} returned {code:#06x}")
            }
            Self::Protocol(e) => write!(f, "PTP protocol error: {e}"),
        }
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;

pub struct Reply {
    pub params: Vec<u32>,
    pub data: Vec<u8>,
}

pub struct Session<P> {
    pipe: P,
    transaction: u32,
}

impl<P: Pipe> Session<P> {
    /// Opens session 1. OpenSession itself carries transaction id 0. A
    /// session left open by an interrupted run is reused.
    pub async fn open(pipe: P) -> Result<Self> {
        let mut session = Self {
            pipe,
            transaction: 0,
        };
        match session.call(OPEN_SESSION, &[1], None).await {
            Ok(_)
            | Err(Error::Response {
                code: SESSION_ALREADY_OPEN,
                ..
            }) => Ok(session),
            Err(e) => Err(e),
        }
    }

    pub async fn close(mut self) -> Result<P> {
        self.call(CLOSE_SESSION, &[], None).await?;
        Ok(self.pipe)
    }

    /// One transaction; `send` is the host-to-device data phase. A device
    /// data phase is collected whenever the device sends one.
    pub async fn call(&mut self, code: u16, params: &[u32], send: Option<&[u8]>) -> Result<Reply> {
        let transaction = self.transaction;
        self.transaction += 1;
        let mut command = header(HEADER + 4 * params.len(), COMMAND, code, transaction);
        for p in params {
            command.extend_from_slice(&p.to_le_bytes());
        }
        self.write_container(&command, &[]).await?;
        if let Some(data) = send {
            let header = header(HEADER + data.len(), DATA, code, transaction);
            self.write_container(&header, data).await?;
        }
        let mut data = Vec::new();
        loop {
            let (kind, reply_code, id, payload) = self.read_container().await?;
            if id != transaction {
                return Err(Error::Protocol(format!(
                    "transaction {id} answered while {transaction} was pending"
                )));
            }
            match kind {
                DATA => data = payload,
                RESPONSE => {
                    if reply_code != RESPONSE_OK {
                        return Err(Error::Response {
                            operation: code,
                            code: reply_code,
                        });
                    }
                    let params = payload
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|c| u32::from_le_bytes(*c))
                        .collect();
                    return Ok(Reply { params, data });
                }
                other => return Err(Error::Protocol(format!("container type {other}"))),
            }
        }
    }

    async fn write_container(&mut self, header: &[u8], payload: &[u8]) -> Result<()> {
        let total = header.len() + payload.len();
        let mut first = header.to_vec();
        let split = payload.len().min(CHUNK - header.len());
        first.extend_from_slice(&payload[..split]);
        self.write(&first).await?;
        for chunk in payload[split..].chunks(CHUNK) {
            self.write(chunk).await?;
        }
        // A data phase that fills its last packet ends with a zero-length packet.
        if total.is_multiple_of(self.pipe.packet_size()) {
            self.write(&[]).await?;
        }
        Ok(())
    }

    async fn write(&mut self, bytes: &[u8]) -> Result<()> {
        self.pipe.write(bytes).await.map_err(Error::Transport)
    }

    async fn read_container(&mut self) -> Result<(u16, u16, u32, Vec<u8>)> {
        let packet = self.pipe.packet_size();
        let mut bytes = self.pipe.read(CHUNK).await.map_err(Error::Transport)?;
        // A transfer ends early at a short or zero-length packet; one that
        // filled its whole request leaves a terminating zero-length packet unread.
        let mut filled = bytes.len() == CHUNK;
        if bytes.len() < HEADER {
            return Err(Error::Protocol(format!("{}-byte container", bytes.len())));
        }
        let length = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        if length < HEADER {
            return Err(Error::Protocol(format!("container length {length}")));
        }
        while bytes.len() < length {
            let request = CHUNK.min(length - bytes.len()).div_ceil(packet) * packet;
            let more = self.pipe.read(request).await.map_err(Error::Transport)?;
            if more.is_empty() {
                return Err(Error::Protocol("data phase ended early".into()));
            }
            filled = more.len() == request;
            bytes.extend_from_slice(&more);
        }
        if bytes.len() != length {
            return Err(Error::Protocol(format!(
                "{} bytes for a {length}-byte container",
                bytes.len()
            )));
        }
        if filled && length.is_multiple_of(packet) {
            let zlp = self.pipe.read(packet).await.map_err(Error::Transport)?;
            if !zlp.is_empty() {
                return Err(Error::Protocol("expected a zero-length packet".into()));
            }
        }
        let kind = u16::from_le_bytes([bytes[4], bytes[5]]);
        let code = u16::from_le_bytes([bytes[6], bytes[7]]);
        let id = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        Ok((kind, code, id, bytes.split_off(HEADER)))
    }
}

fn header(length: usize, kind: u16, code: u16, transaction: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER);
    out.extend_from_slice(&(length as u32).to_le_bytes());
    out.extend_from_slice(&kind.to_le_bytes());
    out.extend_from_slice(&code.to_le_bytes());
    out.extend_from_slice(&transaction.to_le_bytes());
    out
}

/// PTP string: a character count including the terminator, then UTF-16LE.
pub fn write_string(out: &mut Vec<u8>, text: &str) {
    let units: Vec<u16> = text.encode_utf16().collect();
    if units.is_empty() {
        out.push(0);
        return;
    }
    out.push((units.len() + 1) as u8);
    for unit in units.iter().chain([&0]) {
        out.extend_from_slice(&unit.to_le_bytes());
    }
}

/// Cursor over a little-endian PTP dataset.
pub struct Reader<'a> {
    pub bytes: &'a [u8],
    pub offset: usize,
}
impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.offset + n;
        let slice = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(|| Error::Protocol("dataset ended early".into()))?;
        self.offset = end;
        Ok(slice)
    }
    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn string(&mut self) -> Result<String> {
        let count = self.take(1)?[0] as usize;
        let units: Vec<u16> = self
            .take(2 * count)?
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .take_while(|u| *u != 0)
            .collect();
        Ok(String::from_utf16_lossy(&units))
    }
    pub fn u16_array(&mut self) -> Result<Vec<u16>> {
        let count = self.u32()? as usize;
        (0..count).map(|_| self.u16()).collect()
    }
    pub fn u32_array(&mut self) -> Result<Vec<u32>> {
        let count = self.u32()? as usize;
        (0..count).map(|_| self.u32()).collect()
    }
}

pub struct DeviceInfo {
    pub manufacturer: String,
    pub model: String,
    pub device_version: String,
    pub properties: Vec<u16>,
}
impl DeviceInfo {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        r.u16()?; // StandardVersion
        r.u32()?; // VendorExtensionID
        r.u16()?; // VendorExtensionVersion
        r.string()?; // VendorExtensionDesc
        r.u16()?; // FunctionalMode
        r.u16_array()?; // OperationsSupported
        r.u16_array()?; // EventsSupported
        let properties = r.u16_array()?;
        r.u16_array()?; // CaptureFormats
        r.u16_array()?; // ImageFormats
        Ok(Self {
            manufacturer: r.string()?,
            model: r.string()?,
            device_version: r.string()?,
            properties,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_round_trip_with_terminator() {
        let mut out = Vec::new();
        write_string(&mut out, "FUP_FILE.dat");
        assert_eq!(out[0], 13);
        assert_eq!(out.len(), 1 + 26);
        assert_eq!(Reader::new(&out).string().unwrap(), "FUP_FILE.dat");
        let mut empty = Vec::new();
        write_string(&mut empty, "");
        assert_eq!(empty, [0]);
    }
}
