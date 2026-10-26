//! Passive FF80 decoding at the shared physical transport. It never sends USB
//! traffic or changes execution, validation, ownership, or camera state.
use crate::owner::SessionOwner;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
const CAPTURE: usize = 256;
const QUEUE_BYTES: usize = 16 * 1024 * 1024;
#[derive(Clone, Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Decoded {
    pub operation: String,
    pub command: Option<u32>,
    pub function: Option<u32>,
    pub address: Option<u32>,
    pub selector: Option<u32>,
    pub page: Option<u32>,
    pub length: Option<u32>,
}
#[derive(Clone, Debug)]
pub struct Exchange {
    pub device_id: String,
    pub holder: SessionOwner,
    pub sequence: u64,
    pub timestamp_ms: f64,
    pub kind: u8,
    pub session: u16,
    pub decoded: Decoded,
    pub requested: u32,
    pub transferred: usize,
    pub envelope: [u8; 12],
    pub data: Vec<u8>,
    pub truncated: bool,
    pub console_data: Option<Vec<u8>>,
    pub error: Option<String>,
}
impl Exchange {
    pub fn bytes(&self) -> usize {
        self.envelope.len() + self.data.len() + self.console_data.as_ref().map_or(0, Vec::len)
    }
}
struct Subscriber {
    sender: async_channel::Sender<Arc<Exchange>>,
    bytes: Arc<AtomicUsize>,
    overflow: Arc<AtomicBool>,
}
pub struct LogEvents {
    receiver: async_channel::Receiver<Arc<Exchange>>,
    bytes: Arc<AtomicUsize>,
    overflow: Arc<AtomicBool>,
}
impl LogEvents {
    pub async fn next(&self) -> Result<Option<Arc<Exchange>>, &'static str> {
        match self.receiver.recv().await {
            Ok(event) => {
                self.bytes.fetch_sub(event.bytes(), Ordering::Relaxed);
                Ok(Some(event))
            }
            Err(_) if self.overflow.load(Ordering::Relaxed) => {
                Err("FF80 log subscriber exceeded its 16 MiB or 128 event budget")
            }
            Err(_) => Ok(None),
        }
    }
    pub fn close(&self) {
        self.receiver.close();
    }
}
impl Drop for LogEvents {
    fn drop(&mut self) {
        self.close();
    }
}
#[derive(Default)]
pub(crate) struct Recorder {
    frame: Option<Exchange>,
    command: Option<Decoded>,
    sequence: u64,
    subscribers: Vec<Subscriber>,
}
fn word(data: &[u8], offset: usize) -> Option<u32> {
    data.get(offset..offset + 4)
        .map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()))
}
fn timestamp() -> f64 {
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::Date::now()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64()
            * 1000.0
    }
}
fn decode(
    kind: u8,
    data: &[u8],
    previous: Option<&Decoded>,
    offset: u32,
    requested: u32,
) -> Decoded {
    let mut decoded = Decoded {
        operation: match kind {
            0xc0 => "Open session",
            0xc2 => "Ping",
            0xc4 => "Close session",
            0xd0 => "Write command",
            0xd1 => "Read command",
            0xe0 => "Write data",
            0xe1 => "Read data",
            0xe8 => "Empty response",
            _ => "Unknown FF80 operation",
        }
        .into(),
        ..Decoded::default()
    };
    if matches!(kind, 0xe0 | 0xe1) {
        decoded.address = Some(offset);
        decoded.length = Some(requested);
    }
    if !matches!(kind, 0xd0 | 0xd1) {
        return decoded;
    }
    let Some(command) = word(data, 0) else {
        return decoded;
    };
    if kind == 0xd1
        && let Some(previous) = previous.filter(|p| p.command == Some(command))
    {
        return previous.clone();
    }
    decoded.command = Some(command);
    match command {
        0x150000 => {
            decoded.operation = "Device info".into();
            decoded.selector = word(data, 16);
        }
        0x200001 | 0x1a0002 => {
            let function = word(data, 16);
            decoded.function = function;
            decoded.operation = match (command, function) {
                (0x200001, Some(1)) => "Read log",
                (0x200001, Some(2)) => "Read memory",
                (0x200001, Some(3)) => "Write memory",
                (0x200001, Some(5)) => "Debug event",
                (0x200001, Some(6)) => "Console submit",
                (0x200001, Some(7)) => "Console poll",
                (0x1a0002, Some(1)) => "Configuration geometry",
                (0x1a0002, Some(2)) => "Read configuration",
                (0x1a0002, Some(3)) => "Write configuration",
                _ => "Unknown function",
            }
            .into();
            if matches!(function, Some(2 | 3)) {
                decoded.address = word(data, 28);
                decoded.length = word(data, 32);
                if command == 0x1a0002 {
                    decoded.page = word(data, 24);
                }
            }
            if command == 0x200001 && function == Some(6) {
                decoded.length = word(data, 20);
            }
        }
        _ => decoded.operation = format!("Command 0x{command:x}"),
    }
    decoded
}
impl Recorder {
    pub fn watch(&mut self) -> Arc<LogEvents> {
        let (sender, receiver) = async_channel::bounded(128);
        let bytes = Arc::new(AtomicUsize::new(0));
        let overflow = Arc::new(AtomicBool::new(false));
        self.subscribers.push(Subscriber {
            sender,
            bytes: bytes.clone(),
            overflow: overflow.clone(),
        });
        Arc::new(LogEvents {
            receiver,
            bytes,
            overflow,
        })
    }
    fn emit(&mut self, event: Exchange) {
        let event = Arc::new(event);
        self.subscribers.retain(|subscriber| {
            if subscriber.sender.is_closed() {
                return false;
            }
            let count = event.bytes();
            let reserved = loop {
                let previous = subscriber.bytes.load(Ordering::Relaxed);
                let Some(next) = previous
                    .checked_add(count)
                    .filter(|sum| *sum <= QUEUE_BYTES)
                else {
                    break false;
                };
                if subscriber
                    .bytes
                    .compare_exchange_weak(previous, next, Ordering::Relaxed, Ordering::Relaxed)
                    .is_ok()
                {
                    break true;
                }
            };
            if !reserved {
                subscriber.overflow.store(true, Ordering::Relaxed);
                subscriber.sender.close();
                return false;
            }
            if subscriber.sender.try_send(event.clone()).is_err() {
                subscriber.bytes.fetch_sub(count, Ordering::Relaxed);
                subscriber.overflow.store(true, Ordering::Relaxed);
                subscriber.sender.close();
                return false;
            }
            true
        });
    }
    pub fn start(&mut self, device_id: &str, holder: SessionOwner, data: &[u8]) {
        self.fail("FF80 exchange superseded before completion");
        let Ok(envelope): Result<[u8; 12], _> = data.try_into() else {
            self.command = None;
            return;
        };
        if envelope[1] != 0
            || !matches!(
                envelope[0],
                0xc0 | 0xc2 | 0xc4 | 0xd0 | 0xd1 | 0xe0 | 0xe1 | 0xe8
            )
        {
            self.command = None;
            return;
        }
        if envelope[0] != 0xd1 {
            self.command = None;
        }
        self.sequence += 1;
        self.frame = Some(Exchange {
            device_id: device_id.into(),
            holder,
            sequence: self.sequence,
            timestamp_ms: timestamp(),
            kind: envelope[0],
            session: u16::from_be_bytes(envelope[10..12].try_into().unwrap()),
            requested: u32::from_be_bytes(envelope[6..10].try_into().unwrap()),
            decoded: Decoded::default(),
            transferred: 0,
            envelope,
            data: Vec::new(),
            truncated: false,
            console_data: None,
            error: None,
        });
    }
    pub fn control_complete(&mut self) {
        if self
            .frame
            .as_ref()
            .is_some_and(|frame| frame.requested == 0)
        {
            self.finish(None);
        }
    }
    pub fn payload(
        &mut self,
        input: bool,
        data: &[u8],
        requested: usize,
        transferred: usize,
        error: Option<String>,
    ) {
        let Some(frame) = self.frame.as_mut() else {
            return;
        };
        if input == matches!(frame.kind, 0xd0 | 0xe0) {
            return;
        }
        let count = transferred
            .min(data.len())
            .min(CAPTURE.saturating_sub(frame.data.len()));
        frame.data.extend_from_slice(&data[..count]);
        frame.transferred = frame.transferred.saturating_add(transferred);
        frame.truncated = frame.transferred > frame.data.len();
        if error.is_some()
            || frame.transferred >= frame.requested as usize
            || (input && transferred < requested)
        {
            self.finish(error);
        }
    }
    pub fn fail(&mut self, message: &str) {
        if self.frame.is_some() {
            self.finish(Some(message.into()));
        }
    }
    fn finish(&mut self, error: Option<String>) {
        let Some(mut frame) = self.frame.take() else {
            return;
        };
        frame.error = error;
        frame.decoded = decode(
            frame.kind,
            &frame.data,
            self.command.as_ref(),
            u32::from_be_bytes(frame.envelope[2..6].try_into().unwrap()),
            frame.requested,
        );
        if frame.decoded.command == Some(0x200001) && frame.error.is_none() {
            let range = match (frame.kind, word(&frame.data, 16)) {
                (0xd0, Some(6)) => word(&frame.data, 20).map(|length| (24usize, length as usize)),
                (0xd1, Some(7)) => word(&frame.data, 28).map(|length| (32usize, length as usize)),
                _ => None,
            };
            frame.console_data = range.and_then(|(offset, length)| {
                frame
                    .data
                    .get(offset..offset.checked_add(length)?)
                    .map(<[u8]>::to_vec)
            });
        }
        if frame.kind == 0xd0 && frame.error.is_none() {
            self.command = Some(frame.decoded.clone());
        } else if frame.kind != 0xd0 {
            self.command = None;
        }
        self.emit(frame);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn envelope(kind: u8, size: u32) -> [u8; 12] {
        crate::ff80::framing::envelope(
            match kind {
                0xc0 => crate::ff80::framing::RequestKind::Open,
                0xd0 => crate::ff80::framing::RequestKind::WriteCommand,
                _ => crate::ff80::framing::RequestKind::ReadCommand,
            },
            size,
            0x1234,
        )
    }
    #[test]
    fn each_exchange_is_emitted_once_after_all_chunks_with_decoded_request_context() {
        let mut recorder = Recorder::default();
        let log = recorder.watch();
        let mut request = vec![0u8; 36];
        for (offset, value) in [(0, 0x200001u32), (16, 2), (28, 0x12345678), (32, 5)] {
            request[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        recorder.start(
            "camera",
            SessionOwner::Local { tool: None },
            &envelope(0xd0, 36),
        );
        recorder.control_complete();
        assert!(log.receiver.try_recv().is_err());
        recorder.payload(false, &request[..16], 16, 16, None);
        assert!(log.receiver.try_recv().is_err());
        recorder.payload(false, &request[16..], 20, 20, None);
        let sent = log.receiver.try_recv().unwrap();
        assert_eq!(sent.decoded.operation, "Read memory");
        assert_eq!(sent.decoded.address, Some(0x12345678));
        assert_eq!(sent.decoded.length, Some(5));
        assert_eq!(sent.data, request);
        recorder.start(
            "camera",
            SessionOwner::Local { tool: None },
            &envelope(0xd1, 64),
        );
        recorder.payload(true, &request, 64, 36, None);
        let received = log.receiver.try_recv().unwrap();
        assert_eq!(received.sequence, sent.sequence + 1);
        assert_eq!(received.decoded.address, sent.decoded.address);
        assert_eq!(received.requested, 64);
        assert_eq!(received.transferred, 36);
        assert!(log.receiver.try_recv().is_err());
    }
    #[test]
    fn payload_capture_is_bounded_across_chunks_without_losing_transfer_counts() {
        for input in [false, true] {
            for length in [255, 256, 257, 1024] {
                let mut recorder = Recorder::default();
                let log = recorder.watch();
                let data: Vec<u8> = (0..length).map(|index| index as u8).collect();
                recorder.start(
                    "camera",
                    SessionOwner::Local { tool: None },
                    &envelope(if input { 0xd1 } else { 0xd0 }, length as u32),
                );
                for chunk in data.chunks(128) {
                    recorder.payload(input, chunk, chunk.len(), chunk.len(), None);
                }
                let event = log.receiver.try_recv().unwrap();
                assert_eq!(event.data, data[..length.min(256)]);
                assert_eq!(event.requested, length as u32);
                assert_eq!(event.transferred, length);
                assert_eq!(event.truncated, length > 256);
                assert!(event.error.is_none());
                assert!(log.receiver.try_recv().is_err());
            }
        }
    }
    #[test]
    fn failures_keep_partial_bytes_and_remote_holder_without_duplicate_cleanup_records() {
        let mut recorder = Recorder::default();
        let log = recorder.watch();
        recorder.start(
            "camera-2",
            SessionOwner::Remote {
                controller: "peer".into(),
            },
            &envelope(0xc0, 8),
        );
        recorder.payload(true, &[0x34], 64, 1, Some("stall".into()));
        recorder.fail("abandoned");
        let event = log.receiver.try_recv().unwrap();
        assert_eq!(event.device_id, "camera-2");
        assert!(
            matches!(&event.holder, SessionOwner::Remote { controller } if controller == "peer")
        );
        assert_eq!(event.error.as_deref(), Some("stall"));
        assert_eq!(event.data, [0x34]);
        assert!(log.receiver.try_recv().is_err());
    }
    #[test]
    fn console_submit_and_each_output_chunk_are_decoded_once_with_bounds_checks() {
        let mut recorder = Recorder::default();
        let log = recorder.watch();
        for (kind, function, offset, length_offset, text) in [
            (0xd0, 6u32, 24, 20, b"help".as_slice()),
            (0xd1, 7, 32, 28, b"hello\n".as_slice()),
            (0xd0, 6, 24, 20, [b'x'; 233].as_slice()),
            (0xd1, 7, 32, 28, [b'x'; 225].as_slice()),
        ] {
            let mut data = vec![0; offset + text.len()];
            data[..4].copy_from_slice(&0x200001u32.to_le_bytes());
            data[16..20].copy_from_slice(&function.to_le_bytes());
            data[length_offset..length_offset + 4]
                .copy_from_slice(&(text.len() as u32).to_le_bytes());
            data[offset..].copy_from_slice(text);
            for invalid in [false, true] {
                if invalid {
                    data[length_offset..length_offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());
                }
                recorder.start(
                    "camera",
                    SessionOwner::Remote {
                        controller: "peer".into(),
                    },
                    &envelope(kind, data.len() as u32),
                );
                recorder.payload(kind == 0xd1, &data, data.len(), data.len(), None);
                let event = log.receiver.try_recv().unwrap();
                assert_eq!(
                    event.console_data.as_deref(),
                    if invalid || data.len() > 256 {
                        None
                    } else {
                        Some(text)
                    }
                );
                assert_eq!(event.data, data[..data.len().min(256)]);
                assert_eq!(event.transferred, data.len());
                assert_eq!(event.truncated, data.len() > 256);
                assert!(log.receiver.try_recv().is_err());
            }
        }
    }
    #[test]
    fn slow_subscribers_report_overflow_instead_of_silent_loss() {
        let mut recorder = Recorder::default();
        let log = recorder.watch();
        for _ in 0..129 {
            recorder.start(
                "camera",
                SessionOwner::Local { tool: None },
                &envelope(0xc0, 8),
            );
            recorder.payload(true, &[0; 8], 8, 8, None);
        }
        futures_lite::future::block_on(async {
            for _ in 0..128 {
                assert!(log.next().await.unwrap().is_some());
            }
            assert!(log.next().await.unwrap_err().contains("budget"));
        });
    }
}
