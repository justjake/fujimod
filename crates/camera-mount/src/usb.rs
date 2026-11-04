//! The worker releases idle ff80 sessions. Consecutive large reads may share
//! one bounded burst; every file handle is closed before its reply.
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
    Probe,
    Memory(u32, u32),
    Flash(u64, u32),
    Sync,
    Unload,
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
async fn target(device_id: Option<String>) -> std::result::Result<Target, String> {
    let mut devices = connection::discover(DeviceFilter::Diagnostic)
        .await
        .map_err(|e| e.to_string())?;
    if let Some(id) = device_id {
        devices.retain(|d| {
            let ports: String = d.port_chain().iter().map(|p| format!("{p:02x}")).collect();
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
fn automatic_firmware(
    device_id: Option<String>,
    catalog: &ReferenceCatalog,
) -> std::result::Result<String, String> {
    block_on(async {
        let mut target = target(device_id).await?;
        let mut session = target.open().await.map_err(|e| e.to_string())?;
        let identity = async {
            let product = session.device_info(InfoSelector::Product).await?;
            let version = session.device_info(InfoSelector::InternalVersion).await?;
            Ok::<_, fujimod_usb::Error>((
                product.text.trim().to_owned(),
                version.text.trim().to_owned(),
            ))
        }
        .await;
        session.close().await.map_err(|e| e.to_string())?;
        let (product, version) = identity.map_err(|e| e.to_string())?;
        let mut ids = std::collections::BTreeSet::new();
        for facts in catalog.bulk_profiles() {
            if facts.product == product && facts.internal_version == version {
                ids.insert(facts.firmware_id.clone());
            }
        }
        for facts in catalog.card_profiles() {
            if facts.product == product && facts.internal_version == version {
                ids.insert(facts.firmware_id.clone());
            }
        }
        if ids.len() != 1 {
            return Err(format!(
                "Expected one catalog firmware for {product} {version}, found {}",
                ids.len()
            ));
        }
        Ok(ids.into_iter().next().unwrap())
    })
}
struct Client<F> {
    sessions: F,
    capability: Option<FirmwareCard>,
    product: String,
    internal_version: String,
    bulk: Option<FirmwareBulk>,
    flash: Option<crate::flash::Reader>,
    service: Option<crate::service::Service>,
    limits: DebugMemoryLimits,
    mailbox_hint: Option<u32>,
    fenced: bool,
    lease: Option<ReadLease>,
}
const READ_IDLE: Duration = Duration::from_millis(20);
struct ReadLease {
    session: Session,
    mapping: Option<fujimod_workflows::bulk::BulkMapping>,
    started: std::time::Instant,
    bytes: u64,
    service_checked: bool,
}
impl<F: Sessions> Client<F> {
    async fn release_lease(&mut self) -> fujimod_usb::Result<()> {
        let Some(mut lease) = self.lease.take() else {
            return Ok(());
        };
        let mut transport = Transport::new(self.bulk.as_ref(), self.limits, &lease.session, None);
        transport.resume(lease.mapping.take());
        let result = async {
            transport.restore(&mut lease.session).await?;
            lease.session.close().await?;
            Ok(())
        }
        .await;
        if result.is_err() {
            self.fenced = true;
        }
        result
    }
    async fn transaction(&mut self, op: Operation) -> fujimod_usb::Result<Answer> {
        use fujimod_usb::Error;
        if self.fenced {
            return Err(Error::CompletionUnknown);
        }
        let read_bytes = match &op {
            Operation::Memory(_, size) | Operation::Card(Request::ReadAt { size, .. })
                if *size >= 65536 =>
            {
                u64::from(*size)
            }
            _ => 0,
        };
        if read_bytes == 0
            || self.lease.as_ref().is_some_and(|lease| {
                lease.started.elapsed() >= Duration::from_secs(2)
                    || lease.bytes + read_bytes > 64 << 20
            })
        {
            self.release_lease().await?;
        }
        // Every write is committed and every handle closed before replying.
        if matches!(op, Operation::Sync)
            || matches!(op, Operation::Close | Operation::Unload) && self.service.is_none()
        {
            return Ok(Answer::Done);
        }
        let acquisition = std::time::Instant::now();
        let previous = self.lease.take();
        let reused = previous.is_some();
        let (mut session, mapping, burst_started, burst_bytes, mut service_checked) =
            if let Some(lease) = previous {
                (
                    lease.session,
                    lease.mapping,
                    lease.started,
                    lease.bytes,
                    lease.service_checked,
                )
            } else {
                (
                    self.sessions.open().await?,
                    None,
                    std::time::Instant::now(),
                    0,
                    false,
                )
            };
        let acquisition_seconds = acquisition.elapsed().as_secs_f64();
        let started = std::time::Instant::now();
        let metric = match &op {
            Operation::Memory(_, size) => Some(("memory", *size)),
            Operation::Card(Request::ReadAt { size, .. }) => Some(("sd", *size)),
            Operation::Flash(_, size) => Some(("flash", *size)),
            _ => None,
        };
        let mut transport = Transport::new(
            self.bulk.as_ref(),
            self.limits,
            &session,
            self.bulk.as_ref().and_then(|facts| facts.upload.as_ref()),
        );
        transport.resume(mapping);
        let mut cleanup_known = true;
        let result = async {
            match op {
                Operation::Card(request) => {
                    if self.service.is_none() {
                        if let Some(facts) =
                            self.capability.as_ref().and_then(|c| c.service.as_ref())
                        {
                            if session
                                .device_info(InfoSelector::Product)
                                .await?
                                .text
                                .trim()
                                != self.product
                                || session
                                    .device_info(InfoSelector::InternalVersion)
                                    .await?
                                    .text
                                    .trim()
                                    != self.internal_version
                            {
                                return Err(Error::Card(
                                    "Connected camera differs from the mounted firmware".into(),
                                ));
                            }
                            self.service = Some(
                                crate::service::Service::install(
                                    &mut session,
                                    self.limits,
                                    self.bulk.as_ref().ok_or(Error::OutsideDataWindow)?,
                                    facts,
                                )
                                .await?,
                            );
                        }
                    }
                    let owned =
                        if let Some(service) = &self.service {
                            if !service_checked {
                                service
                                    .check(&mut session, self.limits, Some(&mut transport))
                                    .await?;
                                service_checked = true;
                            }
                            self.mailbox_hint = Some(service.base + 0x2000);
                            Some(service.capability(
                                self.capability.as_ref().ok_or(Error::OutsideDataWindow)?,
                            ))
                        } else {
                            None
                        };
                    let mut volume = Volume::attach(
                        &mut session,
                        self.limits,
                        owned
                            .as_ref()
                            .or(self.capability.as_ref())
                            .ok_or(Error::OutsideDataWindow)?,
                        Some(&mut transport),
                        self.mailbox_hint,
                    )
                    .await?;
                    if let Some(service) = &self.service {
                        volume.set_query_trigger(service.trigger())?;
                    }
                    self.mailbox_hint = volume.mailbox_address();
                    let result = volume.execute(request).await.map(Answer::Card);
                    if let Err(error) = volume.close().await {
                        cleanup_known = false;
                        return Err(error);
                    }
                    result
                }
                Operation::Flash(offset, size) => {
                    // Both backends use owned camera RAM. Yield the file helper
                    // allocation before a flash reader requests its workspace.
                    if let Some(service) = &self.service {
                        service.release(&mut session, self.limits).await?;
                        self.service = None;
                        self.mailbox_hint = None;
                    }
                    let flash = self.flash.as_ref().ok_or(Error::OutsideDataWindow)?;
                    let bulk = self.bulk.as_ref().ok_or(Error::OutsideDataWindow)?;
                    let bytes = fujimod_workflows::flash::read_range(
                        &mut session,
                        bulk,
                        &flash.facts,
                        &flash.payload,
                        &flash.image,
                        fujimod_workflows::Limits {
                            memory: self.limits,
                            configuration: NonZeroU32::new(256).unwrap(),
                        },
                        offset,
                        size,
                    )
                    .await
                    .map_err(|e| match e {
                        fujimod_workflows::bulk::BulkError::Usb(e) => e,
                        e => Error::Card(e.to_string()),
                    })?;
                    Ok(Answer::Bytes(bytes))
                }
                Operation::Probe => {
                    if session
                        .device_info(InfoSelector::Product)
                        .await?
                        .text
                        .trim()
                        != self.product
                        || session
                            .device_info(InfoSelector::InternalVersion)
                            .await?
                            .text
                            .trim()
                            != self.internal_version
                    {
                        return Err(Error::Card(
                            "Connected camera differs from the mounted firmware".into(),
                        ));
                    }
                    Ok(Answer::Done)
                }
                Operation::Memory(address, size) => {
                    if !reused
                        && (session
                            .device_info(InfoSelector::Product)
                            .await?
                            .text
                            .trim()
                            != self.product
                            || session
                                .device_info(InfoSelector::InternalVersion)
                                .await?
                                .text
                                .trim()
                                != self.internal_version)
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
                Operation::Close | Operation::Unload => {
                    if let Some(service) = &self.service {
                        service.release(&mut session, self.limits).await?;
                    }
                    self.service = None;
                    self.mailbox_hint = None;
                    Ok(Answer::Done)
                }
                Operation::Sync => unreachable!("handled without a session"),
            }
        }
        .await;
        if !cleanup_known || session.state() != SessionState::Ready {
            self.fenced = true;
            return result.and(Err(Error::CompletionUnknown));
        }
        let retain = result.is_ok()
            && read_bytes != 0
            && burst_bytes + read_bytes < 64 << 20
            && burst_started.elapsed() < Duration::from_secs(2);
        if retain {
            self.lease = Some(ReadLease {
                session,
                mapping: transport.suspend(),
                started: burst_started,
                bytes: burst_bytes + read_bytes,
                service_checked,
            });
        } else {
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
        }
        if std::env::var_os("FUJIMOD_MOUNT_TRACE").is_some() {
            if let Some((kind, size)) = metric {
                eprintln!(
                    "{}",
                    serde_json::json!({"mount_io":kind,"bytes":size,
                    "seconds":started.elapsed().as_secs_f64(),"acquisition_seconds":acquisition_seconds,"success":result.is_ok()})
                );
            }
        }
        result
    }
}

pub struct UsbDevice {
    display: Option<crate::display::Profile>,
    capture_regions: Vec<Region>,
    queue: SyncSender<Work>,
    worker: Option<JoinHandle<()>>,
    flash_bytes: Option<u64>,
    has_card: bool,
}
impl UsbDevice {
    /// Select descriptors and preflight one bounded transaction. Idle mounts
    /// hold no connection, ff80 session or file handle. An owned helper keeps
    /// its allocation until idle cleanup.
    pub fn connect(
        firmware: String,
        device_id: Option<String>,
        limits: DebugMemoryLimits,
        flash_reader: Option<std::path::PathBuf>,
    ) -> std::result::Result<(Self, Vec<Region>), String> {
        let catalog = ReferenceCatalog::bundled()?;
        let firmware = if firmware == "auto" {
            automatic_firmware(device_id.clone(), &catalog)?
        } else {
            firmware
        };
        let selected = catalog.resolve_firmware(&firmware)?;
        if let Some(capability) = &selected.card_files {
            capability.validate()?;
        }
        let flash = flash_reader
            .as_deref()
            .map(|path| crate::flash::Reader::load(path, &selected))
            .transpose()?;
        let flash_bytes = flash.as_ref().map(|reader| reader.facts.window.size);
        let (product, internal_version) = selected
            .bulk
            .as_ref()
            .map(|facts| (facts.product.clone(), facts.internal_version.clone()))
            .or_else(|| {
                selected
                    .card_files
                    .as_ref()
                    .map(|facts| (facts.product.clone(), facts.internal_version.clone()))
            })
            .ok_or("Selected firmware has no located debug identity")?;
        let display = crate::display::Profile::from_soc(&selected.soc, selected.body.dram_bytes);
        let capability = selected.card_files;
        let has_card = capability.is_some();
        let regions: Vec<Region> = selected
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
                    let target = target(device_id).await;
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
                        product,
                        internal_version,
                        bulk: selected.bulk,
                        flash,
                        service: None,
                        limits,
                        mailbox_hint: None,
                        fenced: false,
                        lease: None,
                    };
                    let preflight = if has_card {
                        Operation::Card(Request::ReadDirectory {
                            path: "A:\\".into(),
                        })
                    } else {
                        Operation::Probe
                    };
                    if let Err(error) = client.transaction(preflight).await {
                        if !client.fenced {
                            let _ = client.transaction(Operation::Close).await;
                        }
                        let _ = ready_tx.send(Err(error.to_string()));
                        return;
                    }
                    if ready_tx.send(Ok(())).is_err() {
                        return;
                    }
                    loop {
                        let pending = if client.lease.is_some() {
                            rx.recv_timeout(READ_IDLE)
                        } else if client.service.is_some() && !client.fenced {
                            rx.recv_timeout(Duration::from_secs(2))
                        } else {
                            rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected)
                        };
                        let (op, reply) = match pending {
                            Ok(work) => work,
                            Err(mpsc::RecvTimeoutError::Timeout) => {
                                let result = if client.lease.is_some() {
                                    client.release_lease().await
                                } else {
                                    client.transaction(Operation::Unload).await.map(|_| ())
                                };
                                if let Err(error) = result {
                                    eprintln!("fujimod-mount: {error}");
                                }
                                continue;
                            }
                            Err(mpsc::RecvTimeoutError::Disconnected) => {
                                let _ = client.release_lease().await;
                                let _ = client.transaction(Operation::Close).await;
                                return;
                            }
                        };
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
                    display,
                    capture_regions: regions.clone(),
                    queue: tx,
                    worker: Some(worker),
                    flash_bytes,
                    has_card,
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

const MEMORY_CAPTURE_LIMIT: usize = 64 << 20;
impl Device for UsbDevice {
    fn display_outputs(&self) -> Vec<u32> {
        self.display.as_ref().map_or_else(Vec::new, |p| p.outputs())
    }
    fn display_images(&mut self, output: u32) -> Result<Vec<crate::display::Image>> {
        let profile = self.display.clone().ok_or(libc::ENOTSUP)?;
        let regions = self.capture_regions.clone();
        let mut cache: Option<(u64, Vec<u8>)> = None;
        let result = profile.capture(output, |a, n| {
            // The small register reads delimit attempts; never carry pixels across them.
            if a >= 0xff000000 {
                cache = None;
            }
            if let Some((base, bytes)) = &cache {
                if a >= *base
                    && a.checked_add(n as u64)
                        .is_some_and(|end| end <= *base + bytes.len() as u64)
                {
                    let offset = (a - base) as usize;
                    return Ok(bytes[offset..offset + n].to_vec());
                }
            }
            let requested = n;
            let n = if a < 0xff000000 && n < 262144 {
                regions
                    .iter()
                    .find_map(|r| {
                        let end = r.address.checked_add(r.size)?;
                        (a >= r.address && a.checked_add(n as u64)? <= end)
                            .then_some((end - a).min(262144) as usize)
                    })
                    .unwrap_or(n)
            } else {
                n
            };
            if n > MEMORY_CAPTURE_LIMIT {
                return Err("Display memory read exceeds capture bounds".into());
            }
            let a = u32::try_from(a).map_err(|_| "Display address exceeds transport width")?;
            let mut bytes = Vec::with_capacity(n);
            while bytes.len() < n {
                let count = (n - bytes.len()).min(64 << 20);
                let address = a
                    .checked_add(bytes.len() as u32)
                    .ok_or("Display address overflow")?;
                let part = self
                    .memory(address, count as u32)
                    .map_err(|e| format!("Display memory read: errno {e}"))?;
                if part.len() != count {
                    return Err("Short display memory read".into());
                }
                bytes.extend(part);
            }
            if n > requested {
                let result = bytes[..requested].to_vec();
                cache = Some((u64::from(a), bytes));
                Ok(result)
            } else {
                Ok(bytes)
            }
        });
        self.synchronize()?;
        result.map_err(|error| {
            eprintln!("fujimod-mount: {error}");
            libc::EIO
        })
    }

    fn shutdown(&mut self) -> Result<()> {
        UsbDevice::shutdown(self)
    }
    fn unload_helper(&mut self) -> Result<()> {
        self.request(Operation::Unload).map(|_| ())
    }
    fn has_card(&self) -> bool {
        self.has_card
    }
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
    fn flash_size(&self) -> Option<u64> {
        self.flash_bytes
    }
    fn flash(&mut self, offset: u64, size: u32) -> Result<Vec<u8>> {
        if self.flash_bytes.is_none() {
            return Err(libc::ENOTSUP);
        }
        match self.request(Operation::Flash(offset, size))? {
            Answer::Bytes(bytes) => Ok(bytes),
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
