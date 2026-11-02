//! One worker owns the USB session and mount reservation. Bounded requests
//! serialize both mount interfaces without a second claim or nested executor.
use crate::{Device, Region, Result};
use fujimod_catalog::ReferenceCatalog;
use fujimod_usb::{
    connection::{self, Connection, DeviceFilter, TransferOptions},
    ff80::{SessionState, commands::memory::DebugMemoryLimits},
    operations::card::{DirectoryEntry, Request, Response, Volume},
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

pub struct UsbDevice {
    queue: SyncSender<Work>,
    worker: Option<JoinHandle<()>>,
}

impl UsbDevice {
    /// Opens only an already available ff80 service. Never enables service
    /// mode, installs code, reclaims a session, resets or retries a camera.
    pub fn connect(
        firmware: String,
        device_id: Option<String>,
        limits: DebugMemoryLimits,
    ) -> std::result::Result<(Self, Vec<Region>), String> {
        let catalog = ReferenceCatalog::bundled()?;
        let selected = catalog.resolve_firmware(&firmware)?;
        let capability = selected
            .card_files
            .ok_or("Selected firmware has no card capability")?;
        let bulk = selected
            .bulk
            .filter(|b| matches!(b.source, fujimod_catalog_bulk::BulkSource::Query { .. }));
        let regions: Vec<_> = selected
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
                    let prepared = async {
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
                        let connection = Connection::open(
                            &device,
                            interfaces[0],
                            TransferOptions {
                                timeout: Duration::from_secs(5),
                                max_reply_bytes: NonZeroU32::new(1024 * 1024).expect("constant"),
                            },
                        )
                        .await
                        .map_err(|e| e.to_string())?;
                        connection.set_tool("fujimod-mount".into());
                        connection
                            .begin_session(None)
                            .await
                            .map_err(|e| e.to_string())
                    }
                    .await;
                    let mut session = match prepared {
                        Ok(s) => s,
                        Err(e) => {
                            let _ = ready_tx.send(Err(e));
                            return;
                        }
                    };
                    let mut mapping = if let Some(bulk) = bulk {
                        match fujimod_workflows::bulk::BulkMapping::prepare(
                            &mut session,
                            &bulk,
                            fujimod_workflows::Limits {
                                memory: limits,
                                configuration: NonZeroU32::new(256).expect("constant"),
                            },
                        )
                        .await
                        {
                            Ok(mapping) => Some(mapping),
                            Err(error) => {
                                let _ = ready_tx.send(Err(error.to_string()));
                                return;
                            }
                        }
                    } else {
                        None
                    };
                    let reader = mapping
                        .as_mut()
                        .map(|m| m as &mut dyn fujimod_usb::operations::card::RangeRead);
                    let mut volume =
                        match Volume::attach(&mut session, limits, &capability, reader).await {
                            Ok(v) => v,
                            Err(e) => {
                                let _ = ready_tx.send(Err(e.to_string()));
                                let _ = finish(session, mapping, Ok(())).await;
                                return;
                            }
                        };
                    // Refuse an old installed service before exposing an unusable mount.
                    if let Err(e) = volume
                        .execute(Request::ReadDirectory {
                            path: "A:\\".into(),
                        })
                        .await
                    {
                        let _ = ready_tx.send(Err(e.to_string()));
                        let result = volume.close().await;
                        let _ = finish(session, mapping, result).await;
                        return;
                    }
                    if ready_tx.send(Ok(())).is_err() {
                        let result = volume.close().await;
                        let _ = finish(session, mapping, result).await;
                        return;
                    }
                    while let Ok((op, reply)) = rx.recv() {
                        let result = match op {
                            Operation::Card(request) => {
                                volume.execute(request).await.map(Answer::Card)
                            }
                            Operation::Memory(address, size) => {
                                volume.read_memory(address, size).await.map(Answer::Bytes)
                            }
                            Operation::Sync => volume.synchronize().await.map(|()| Answer::Done),
                            Operation::Close => {
                                let result = volume.close().await;
                                let result = finish(session, mapping, result).await;
                                let _ = reply.send(result.map(|()| Answer::Done).map_err(report));
                                return;
                            }
                        };
                        let _ = reply.send(result.map_err(report));
                    }
                    let result = volume.close().await;
                    let _ = finish(session, mapping, result).await;
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
            Err(e) => {
                let _ = worker.join();
                Err(e)
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

async fn finish(
    mut session: fujimod_usb::ff80::Session,
    mapping: Option<fujimod_workflows::bulk::BulkMapping>,
    result: fujimod_usb::Result<()>,
) -> fujimod_usb::Result<()> {
    result?;
    if session.state() == SessionState::Ready {
        if let Some(mapping) = mapping {
            mapping
                .restore(&mut session)
                .await
                .map_err(|error| fujimod_usb::Error::Card(error.to_string()))?;
        }
        session.close().await?;
    }
    Ok(())
}
