//! The worker retains descriptors, never an idle ff80 session. Every bounded
//! operation closes files and restores transport state before releasing USB.
use crate::{Device, Region, Result, transport::Transport};
use fujimod_catalog::ReferenceCatalog;
use fujimod_catalog_bulk::FirmwareBulk;
use fujimod_usb::{
    connection::{self, Connection, DeviceFilter, InterfaceSelection, TransferOptions},
    ff80::{
        Session, SessionState,
        commands::{
            info::InfoSelector,
            memory::{Address, DebugMemoryLimits, MemoryRange},
        },
    },
    operations::{
        card::{BufferIo, DirectoryEntry, FirmwareCard, Request, Response, Volume},
        memory,
    },
};
use futures_lite::future::block_on;
use std::{
    num::NonZeroU32,
    sync::mpsc::{self, SyncSender},
    thread::{self, JoinHandle},
    time::Duration,
};

enum Operation {
    Card(Request),
    Memory(u32, u32),
    Sync,
    Close,
}
enum Answer {
    Card(Response),
    Bytes(Vec<u8>),
    Done,
}
type Work = (Operation, mpsc::Sender<Result<Answer>>);

trait Sessions {
    async fn open(&mut self) -> fujimod_usb::Result<Session>;
}
struct Target {
    device: connection::Device,
    interface: InterfaceSelection,
}
impl Sessions for Target {
    async fn open(&mut self) -> fujimod_usb::Result<Session> {
        let connection = Connection::open(
            &self.device,
            self.interface,
            TransferOptions {
                timeout: Duration::from_secs(5),
                max_reply_bytes: NonZeroU32::new(1024 * 1024).expect("constant"),
            },
        )
        .await?;
        connection.set_tool("fujimod-mount".into());
        connection.begin_session(None).await
    }
}
struct Client<F> {
    sessions: F,
    capability: FirmwareCard,
    bulk: Option<FirmwareBulk>,
    limits: DebugMemoryLimits,
    mailbox_hint: Option<u32>,
    fenced: bool,
}
impl<F: Sessions> Client<F> {
    async fn transaction(&mut self, op: Operation) -> fujimod_usb::Result<Answer> {
        use fujimod_usb::Error;
        if self.fenced {
            return Err(Error::CompletionUnknown);
        }
        // Every write is committed and every handle closed before replying.
        if matches!(op, Operation::Sync | Operation::Close) {
            return Ok(Answer::Done);
        }
        let mut session = self.sessions.open().await?;
        let mut transport = Transport::new(
            self.bulk.as_ref(),
            self.limits,
            &session,
            self.capability.upload.as_ref(),
        );
        let mut cleanup_known = true;
        let result = async {
            match op {
                Operation::Card(request) => {
                    let mut volume = Volume::attach(
                        &mut session,
                        self.limits,
                        &self.capability,
                        Some(&mut transport),
                        self.mailbox_hint,
                    )
                    .await?;
                    self.mailbox_hint = volume.mailbox_address();
                    let result = volume.execute(request).await.map(Answer::Card);
                    if let Err(error) = volume.close().await {
                        cleanup_known = false;
                        return Err(error);
                    }
                    result
                }
                Operation::Memory(address, size) => {
                    if session
                        .device_info(InfoSelector::Product)
                        .await?
                        .text
                        .trim()
                        != self.capability.product
                        || session
                            .device_info(InfoSelector::InternalVersion)
                            .await?
                            .text
                            .trim()
                            != self.capability.internal_version
                    {
                        return Err(Error::Card(
                            "Connected camera differs from the mounted firmware".into(),
                        ));
                    }
                    let data = if transport.admits(address, size) {
                        transport.read(&mut session, address, size).await?
                    } else {
                        let range = MemoryRange::new(
                            Address(address),
                            NonZeroU32::new(size).ok_or(Error::InvalidLength)?,
                        )?;
                        memory::read_range(&mut session, range, self.limits).await?
                    };
                    Ok(Answer::Bytes(data))
                }
                Operation::Sync | Operation::Close => unreachable!("handled without a session"),
            }
        }
        .await;
        if !cleanup_known || session.state() != SessionState::Ready {
            self.fenced = true;
            return result.and(Err(Error::CompletionUnknown));
        }
        if let Err(error) = transport.restore(&mut session).await {
            self.fenced = true;
            return Err(error);
        }
        // Even a completed camera error yields its session. Unknown completion
        // above never sends cleanup, close, reclaim or another request.
        if let Err(error) = session.close().await {
            self.fenced = true;
            return Err(error);
        }
        result
    }
}

pub struct UsbDevice {
    queue: SyncSender<Work>,
    worker: Option<JoinHandle<()>>,
}
impl UsbDevice {
    /// Select descriptors and preflight one bounded transaction. Idle mounts
    /// hold no connection, ff80 session, file handle or camera RAM allocation.
    pub fn connect(
        firmware: String,
        device_id: Option<String>,
        limits: DebugMemoryLimits,
    ) -> std::result::Result<(Self, Vec<Region>), String> {
        let selected = ReferenceCatalog::bundled()?.resolve_firmware(&firmware)?;
        let capability = selected
            .card_files
            .ok_or("Selected firmware has no card capability")?;
        let regions = selected
            .state
            .ok_or("Selected firmware has no readable memory map")?
            .regions
            .into_iter()
            .map(|r| Region {
                name: r.name,
                address: r.address,
                size: r.size,
            })
            .collect();
        let (tx, rx) = mpsc::sync_channel::<Work>(32);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("camera-mount-usb".into())
            .spawn(move || {
                block_on(async {
                    let target = async {
                        let mut devices = connection::discover(DeviceFilter::Diagnostic)
                            .await
                            .map_err(|e| e.to_string())?;
                        if let Some(id) = device_id {
                            devices.retain(|d| {
                                let ports: String =
                                    d.port_chain().iter().map(|p| format!("{p:02x}")).collect();
                                d.bus_id() == id
                                    || format!("{}-{ports}", d.bus_id()) == id
                                    || d.serial_number() == Some(id.as_str())
                            });
                        }
                        if devices.len() != 1 {
                            return Err(format!(
                                "Expected one selected debug camera, found {}",
                                devices.len()
                            ));
                        }
                        let device = devices.remove(0);
                        let interfaces = connection::interfaces(&device)
                            .await
                            .map_err(|e| e.to_string())?;
                        if interfaces.len() != 1 {
                            return Err(format!(
                                "Expected one debug interface, found {}",
                                interfaces.len()
                            ));
                        }
                        Ok(Target {
                            device,
                            interface: interfaces[0],
                        })
                    }
                    .await;
                    let target = match target {
                        Ok(target) => target,
                        Err(error) => {
                            let _ = ready_tx.send(Err(error));
                            return;
                        }
                    };
                    let mut client = Client {
                        sessions: target,
                        capability,
                        bulk: selected.bulk,
                        limits,
                        mailbox_hint: None,
                        fenced: false,
                    };
                    if let Err(error) = client
                        .transaction(Operation::Card(Request::ReadDirectory {
                            path: "A:\\".into(),
                        }))
                        .await
                    {
                        let _ = ready_tx.send(Err(error.to_string()));
                        return;
                    }
                    if ready_tx.send(Ok(())).is_err() {
                        return;
                    }
                    while let Ok((op, reply)) = rx.recv() {
                        let close = matches!(op, Operation::Close);
                        let result = client.transaction(op).await.map_err(report);
                        let _ = reply.send(result);
                        if close {
                            return;
                        }
                    }
                });
            })
            .map_err(|e| e.to_string())?;
        match ready_rx.recv().map_err(|e| e.to_string())? {
            Ok(()) => Ok((
                Self {
                    queue: tx,
                    worker: Some(worker),
                },
                regions,
            )),
            Err(error) => {
                let _ = worker.join();
                Err(error)
            }
        }
    }
    fn request(&self, op: Operation) -> Result<Answer> {
        let (tx, rx) = mpsc::channel();
        self.queue.send((op, tx)).map_err(|_| libc::ENODEV)?;
        rx.recv().map_err(|_| libc::ENODEV)?
    }
    fn card(&self, request: Request) -> Result<Response> {
        match self.request(Operation::Card(request))? {
            Answer::Card(r) => Ok(r),
            _ => Err(libc::EIO),
        }
    }
    pub fn close(mut self) -> Result<()> {
        self.shutdown()
    }
    fn shutdown(&mut self) -> Result<()> {
        if let Some(worker) = self.worker.take() {
            let result = self.request(Operation::Close).map(|_| ());
            worker.join().map_err(|_| libc::EIO)?;
            result
        } else {
            Ok(())
        }
    }
}

fn report(error: fujimod_usb::Error) -> i32 {
    eprintln!("fujimod-mount: {error}");
    match error {
        fujimod_usb::Error::CompletionUnknown => libc::ENODEV,
        fujimod_usb::Error::SessionAlreadyOpen => libc::EBUSY,
        _ => libc::EIO,
    }
}

impl Device for UsbDevice {
    fn list(&mut self, path: &str) -> Result<Vec<DirectoryEntry>> {
        match self.card(Request::ReadDirectory { path: path.into() })? {
            Response::Directory(v) => Ok(v),
            _ => Err(libc::EIO),
        }
    }
    fn read(&mut self, path: &str, offset: u32, size: u32) -> Result<Vec<u8>> {
        match self.card(Request::ReadAt {
            path: path.into(),
            offset,
            size,
        })? {
            Response::Bytes(v) => Ok(v),
            _ => Err(libc::EIO),
        }
    }
    fn write(&mut self, path: &str, offset: u32, data: &[u8]) -> Result<()> {
        match self.card(Request::WriteAt {
            path: path.into(),
            offset,
            data: data.into(),
        })? {
            Response::Done => Ok(()),
            _ => Err(libc::EIO),
        }
    }
    fn create(&mut self, path: &str) -> Result<()> {
        match self.card(Request::Create { path: path.into() })? {
            Response::Done => Ok(()),
            _ => Err(libc::EIO),
        }
    }
    fn shrink(&mut self, path: &str, size: u32) -> Result<()> {
        match self.card(Request::Shrink {
            path: path.into(),
            size,
        })? {
            Response::Done => Ok(()),
            _ => Err(libc::EIO),
        }
    }
    fn delete(&mut self, path: &str) -> Result<()> {
        match self.card(Request::Delete { path: path.into() })? {
            Response::Done => Ok(()),
            _ => Err(libc::EIO),
        }
    }
    fn memory(&mut self, address: u32, size: u32) -> Result<Vec<u8>> {
        match self.request(Operation::Memory(address, size))? {
            Answer::Bytes(v) => Ok(v),
            _ => Err(libc::EIO),
        }
    }
    fn synchronize(&mut self) -> Result<()> {
        self.request(Operation::Sync).map(|_| ())
    }
}

impl Drop for UsbDevice {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

#[cfg(test)]
mod tests;
