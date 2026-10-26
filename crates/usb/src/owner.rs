//! One nusb owner, shared by direct local sessions and proxy controllers.
//! Physical interfaces stay claimed between logical sessions. Only shutdown
//! releases them, and that release is awaited before the owner can be replaced.
use fujimod_usb_proxy::{Error, Result, protocol::*};
use nusb::{
    Interface as UsbInterface,
    transfer::{Buffer, Bulk, ControlIn, ControlOut, ControlType, In, Interrupt, Out},
};
use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex},
    time::Duration,
};

fn error(value: impl std::fmt::Display) -> Error {
    Error(value.to_string())
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub configuration: u8,
    pub interface: u8,
    pub alternate: u8,
}
#[derive(Debug, PartialEq, Eq)]
enum Claim {
    Local(u64),
    Remote(String),
}
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnerEvent {
    pub id: String,
    pub present: bool,
    pub open: bool,
    pub sharing: bool,
    pub session_owner: Option<SessionOwner>,
    pub fenced: bool,
}
#[derive(Clone, Debug, serde::Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SessionOwner {
    Local {
        #[serde(skip_serializing_if = "Option::is_none")]
        tool: Option<String>,
    },
    Remote {
        controller: String,
    },
}
#[derive(Default)]
struct State {
    claim: Option<Claim>,
    next: u64,
    local_tool: Option<String>,
    fenced: bool,
    stopped: bool,
    present: bool,
    sharing: bool,
    needs_reopen: bool,
    generation: u64,
    watchers: Vec<async_channel::Sender<OwnerEvent>>,
    log: crate::owner_log::Recorder,
    interface: Option<(Selection, UsbInterface)>,
    remote_guard: Option<async_lock::MutexGuardArc<()>>,
}
pub struct DeviceOwner {
    pub info: Option<nusb::DeviceInfo>,
    pub descriptor: Device,
    device: Mutex<nusb::Device>,
    state: Mutex<State>,
    operation: async_lock::Mutex<()>,
    claim_lock: Arc<async_lock::Mutex<()>>,
    shutdown_lock: async_lock::Mutex<()>,
}
impl std::fmt::Debug for DeviceOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceOwner")
            .field("descriptor", &self.descriptor)
            .finish_non_exhaustive()
    }
}
/// A direct local session's exclusive logical claim. Dropping an unfinished
/// session fences the owner; a successful ff80 close releases it explicitly.
pub struct LocalLease {
    owner: Arc<DeviceOwner>,
    id: u64,
    released: bool,
    _guard: async_lock::MutexGuardArc<()>,
}
impl LocalLease {
    pub(crate) fn owner(&self) -> &DeviceOwner {
        &self.owner
    }
    pub(crate) fn set_tool(&self, tool: String) {
        self.owner.state.lock().unwrap().local_tool = Some(tool);
        self.owner.publish();
    }
    pub fn release(&mut self) {
        self.released = true;
    }
}
impl Drop for LocalLease {
    fn drop(&mut self) {
        let mut state = self.owner.state.lock().unwrap();
        if state.claim == Some(Claim::Local(self.id)) {
            state
                .log
                .fail("Local session ended before the FF80 exchange completed");
            state.claim = None;
            if !self.released {
                state.fenced = true;
            }
        }
        drop(state);
        self.owner.publish();
    }
}

impl DeviceOwner {
    pub async fn open(info: nusb::DeviceInfo, id: String) -> Result<Arc<Self>> {
        let device = info.open().await.map_err(error)?;
        let descriptor = Device {
            id,
            vendor_id: info.vendor_id(),
            product_id: info.product_id(),
            manufacturer: info.manufacturer_string().map(str::to_owned),
            product: info.product_string().map(str::to_owned),
            serial: info.serial_number().map(str::to_owned),
            interfaces: vec![],
            status: None,
        };
        Ok(Self::from_device(device, Some(info), descriptor))
    }
    /// Attach an already opened nusb device. Descriptor interfaces are read by
    /// nusb from that device; the caller supplies only its identity and strings.
    pub fn from_device(
        device: nusb::Device,
        info: Option<nusb::DeviceInfo>,
        mut descriptor: Device,
    ) -> Arc<Self> {
        let interfaces = device
            .configurations()
            .flat_map(|config| {
                config.interface_alt_settings().map(move |iface| Interface {
                    configuration: config.configuration_value(),
                    interface: iface.interface_number(),
                    alternate: iface.alternate_setting(),
                    endpoints: iface
                        .endpoints()
                        .map(|ep| Endpoint {
                            address: ep.address(),
                            packet_size: ep.max_packet_size() as u16,
                            kind: match ep.transfer_type() {
                                nusb::descriptors::TransferType::Bulk => "bulk",
                                nusb::descriptors::TransferType::Interrupt => "interrupt",
                                nusb::descriptors::TransferType::Isochronous => "isochronous",
                                nusb::descriptors::TransferType::Control => "control",
                            }
                            .into(),
                        })
                        .collect(),
                })
            })
            .collect();
        descriptor.interfaces = interfaces;
        Arc::new(Self {
            info,
            descriptor,
            device: Mutex::new(device),
            state: Mutex::new(State {
                present: true,
                ..State::default()
            }),
            operation: async_lock::Mutex::new(()),
            claim_lock: Arc::new(async_lock::Mutex::new(())),
            shutdown_lock: async_lock::Mutex::new(()),
        })
    }
    fn snapshot(&self, state: &State) -> OwnerEvent {
        OwnerEvent {
            id: self.descriptor.id.clone(),
            present: state.present,
            open: !state.stopped,
            sharing: state.sharing,
            fenced: state.fenced,
            session_owner: state.claim.as_ref().map(|claim| match claim {
                Claim::Local(_) => SessionOwner::Local {
                    tool: state.local_tool.clone(),
                },
                Claim::Remote(controller) => SessionOwner::Remote {
                    controller: controller.clone(),
                },
            }),
        }
    }
    pub fn watch_log(&self) -> Arc<crate::owner_log::LogEvents> {
        self.state.lock().unwrap().log.watch()
    }
    pub(crate) fn observe_control(&self, bytes: &[u8]) {
        let mut state = self.state.lock().unwrap();
        if let Some(holder) = self.snapshot(&state).session_owner {
            state.log.start(&self.descriptor.id, holder, bytes);
        }
    }
    pub(crate) fn observe_control_complete(&self) {
        self.state.lock().unwrap().log.control_complete();
    }
    pub(crate) fn observe_payload(
        &self,
        input: bool,
        bytes: &[u8],
        requested: usize,
        transferred: usize,
        error: Option<String>,
    ) {
        self.state
            .lock()
            .unwrap()
            .log
            .payload(input, bytes, requested, transferred, error);
    }
    pub(crate) fn observe_failure(&self, message: &str) {
        self.state.lock().unwrap().log.fail(message);
    }
    pub fn status(&self) -> OwnerEvent {
        self.snapshot(&self.state.lock().unwrap())
    }
    fn publish(&self) {
        let mut state = self.state.lock().unwrap();
        let event = self.snapshot(&state);
        state
            .watchers
            .retain(|sender| sender.try_send(event.clone()).is_ok());
    }
    pub fn watch(&self) -> async_channel::Receiver<OwnerEvent> {
        let (sender, receiver) = async_channel::unbounded();
        let mut state = self.state.lock().unwrap();
        sender.try_send(self.snapshot(&state)).unwrap();
        state.watchers.push(sender);
        receiver
    }
    pub fn set_present(&self, present: bool) {
        let mut state = self.state.lock().unwrap();
        if state.present == present {
            return;
        }
        state.present = present;
        if !present {
            state
                .log
                .fail("USB device disconnected before the FF80 exchange completed");
            state.generation += 1;
            state.needs_reopen = true;
            state.stopped = true;
            if state.claim.is_some() {
                state.fenced = true;
            }
            if matches!(state.claim, Some(Claim::Remote(_))) {
                state.claim = None;
                state.remote_guard = None;
            }
        }
        drop(state);
        self.publish();
    }
    pub async fn reopen(&self) -> Result<()> {
        let _shutdown = self.shutdown_lock.lock().await;
        let (needs_reopen, generation, changed) = {
            let state = self.state.lock().unwrap();
            if !state.present {
                return Err(error("USB device is disconnected"));
            }
            if state.fenced && !state.needs_reopen {
                return Err(error("USB completion unknown; reconnect the device"));
            }
            (state.needs_reopen, state.generation, state.stopped)
        };
        if needs_reopen {
            let _claim = self
                .claim_lock
                .try_lock_arc()
                .ok_or_else(|| error("A session still holds the disconnected device"))?;
            let _operation = self.operation.lock().await;
            let previous = self.state.lock().unwrap().interface.take();
            if let Some((_, interface)) = previous {
                // A physically disconnected handle can reject release. Await
                // settlement before reopening; never overlap interface changes.
                let _ = interface.release().await;
            }
            let info = self.info.as_ref().ok_or_else(|| {
                error("Externally attached USB device must be attached again after disconnection")
            })?;
            let device = info.open().await.map_err(error)?;
            *self.device.lock().unwrap() = device;
        }
        let mut state = self.state.lock().unwrap();
        if !state.present || state.generation != generation {
            return Err(error("USB device disconnected while reopening"));
        }
        state.stopped = false;
        if needs_reopen {
            state.needs_reopen = false;
            state.fenced = false;
        }
        drop(state);
        if changed || needs_reopen {
            self.publish();
        }
        Ok(())
    }
    fn validate_selection(&self, selection: Selection) -> Result<()> {
        if !self.descriptor.interfaces.iter().any(|i| {
            i.configuration == selection.configuration
                && i.interface == selection.interface
                && i.alternate == selection.alternate
        }) {
            return Err(error("USB interface unavailable"));
        }
        if self
            .state
            .lock()
            .unwrap()
            .interface
            .as_ref()
            .is_some_and(|(configured, _)| *configured != selection)
        {
            return Err(error("Close the device before changing its USB interface"));
        }
        Ok(())
    }
    pub fn share(&self) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        if !state.present || state.stopped {
            return Err(error("USB device is disconnected"));
        }
        if state.fenced {
            return Err(error("USB completion unknown; reconnect the device"));
        }
        state.sharing = true;
        drop(state);
        self.publish();
        Ok(())
    }
    pub fn stop_sharing(&self) {
        let mut state = self.state.lock().unwrap();
        state.sharing = false;
        if matches!(state.claim, Some(Claim::Remote(_))) {
            state.claim = None;
            state.remote_guard = None;
            state
                .log
                .fail("Remote session ended before the FF80 exchange completed");
            state.fenced = true;
        }
        drop(state);
        self.publish();
    }
    fn available(state: &State) -> Result<()> {
        if !state.present {
            return Err(error("USB device is disconnected"));
        }
        if state.stopped {
            return Err(error("USB device owner is closed"));
        }
        if state.fenced {
            return Err(error("USB completion unknown; reconnect the device"));
        }
        if state.claim.is_some() {
            return Err(error(
                "Camera is in use by another session. Wait for its controller to release the device.",
            ));
        }
        Ok(())
    }
    async fn interface(&self, selection: Selection) -> Result<UsbInterface> {
        let previous = self.state.lock().unwrap().interface.clone();
        if let Some((configured, iface)) = previous {
            if configured != selection {
                return Err(error("Close the device before changing its USB interface"));
            }
            return Ok(iface);
        }
        if !self.descriptor.interfaces.iter().any(|i| {
            i.configuration == selection.configuration
                && i.interface == selection.interface
                && i.alternate == selection.alternate
        }) {
            return Err(error("USB interface unavailable"));
        }
        let device = self.device.lock().unwrap().clone();
        if device
            .active_configuration()
            .map(|c| c.configuration_value())
            .ok()
            != Some(selection.configuration)
        {
            device
                .set_configuration(selection.configuration)
                .await
                .map_err(error)?;
        }
        let iface = device
            .claim_interface(selection.interface)
            .await
            .map_err(error)?;
        self.state.lock().unwrap().interface = Some((selection, iface.clone()));
        if iface.get_alt_setting() != selection.alternate {
            iface
                .set_alt_setting(selection.alternate)
                .await
                .map_err(error)?;
        }
        Ok(iface)
    }
    pub async fn local(
        self: &Arc<Self>,
        selection: Selection,
    ) -> Result<(LocalLease, UsbInterface)> {
        self.validate_selection(selection)?;
        let guard = self.claim_lock.try_lock_arc().ok_or_else(|| error("Camera is in use by another session. Wait for its controller to release the device."))?;
        let id = {
            let mut state = self.state.lock().unwrap();
            Self::available(&state)?;
            state.next += 1;
            let id = state.next;
            state.local_tool = None;
            state.claim = Some(Claim::Local(id));
            id
        };
        self.publish();
        let lease = LocalLease {
            owner: self.clone(),
            id,
            released: false,
            _guard: guard,
        };
        let _operation = self.operation.lock().await;
        Ok((lease, self.interface(selection).await?))
    }
    pub fn owns(&self, controller: &str) -> bool {
        self.state.lock().unwrap().claim == Some(Claim::Remote(controller.into()))
    }
    pub fn fenced(&self) -> bool {
        self.state.lock().unwrap().fenced
    }
    pub fn abandon(&self, controller: &str) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.claim == Some(Claim::Remote(controller.into())) {
            state.claim = None;
            state.remote_guard = None;
            state
                .log
                .fail("Remote session ended before the FF80 exchange completed");
            state.fenced = true;
            drop(state);
            self.publish();
            true
        } else {
            false
        }
    }
    pub async fn shutdown(&self) -> Result<()> {
        let _shutdown = self.shutdown_lock.lock().await;
        {
            let mut state = self.state.lock().unwrap();
            state.stopped = true;
            state.sharing = false;
            if matches!(state.claim, Some(Claim::Remote(_))) {
                state
                    .log
                    .fail("Owner shut down before the FF80 exchange completed");
                state.claim = None;
                state.remote_guard = None;
                state.fenced = true;
            }
        }
        self.publish();
        let _claim = self.claim_lock.lock_arc().await;
        let _operation = self.operation.lock().await;
        let iface = self.state.lock().unwrap().interface.take();
        if let Some((_, iface)) = iface {
            iface.release().await.map_err(error)?;
        }
        Ok(())
    }
    async fn execute(
        &self,
        controller: &str,
        op: Operation,
        timeout: Duration,
    ) -> Result<Completion> {
        if let Operation::Open {
            configuration,
            interface,
            alternate,
        } = op
        {
            self.validate_selection(Selection {
                configuration,
                interface,
                alternate,
            })?;
            {
                let guard = self.claim_lock.try_lock_arc().ok_or_else(|| error("Camera is in use by another session. Wait for its controller to release the device."))?;
                let mut state = self.state.lock().unwrap();
                Self::available(&state)?;
                if !state.sharing {
                    return Err(error("USB device is not shared"));
                }
                state.claim = Some(Claim::Remote(controller.into()));
                state.remote_guard = Some(guard);
            }
            self.publish();
            let _operation = self.operation.lock().await;
            let result = self
                .interface(Selection {
                    configuration,
                    interface,
                    alternate,
                })
                .await;
            if result.is_err() {
                self.state.lock().unwrap().fenced = true;
                self.publish();
            }
            result?;
            return Ok(Completion::default());
        }
        let _operation = self.operation.lock().await;
        if !self.owns(controller) {
            return Err(error("USB device is not claimed by this controller"));
        }
        if matches!(op, Operation::Close) {
            let mut state = self.state.lock().unwrap();
            state
                .log
                .fail("Remote session closed before the FF80 exchange completed");
            state.claim = None;
            state.remote_guard = None;
            drop(state);
            self.publish();
            return Ok(Completion::default());
        }
        if self.fenced() {
            return Err(error("USB completion unknown; reconnect the device"));
        }
        let (selection, iface) = self
            .state
            .lock()
            .unwrap()
            .interface
            .clone()
            .ok_or_else(|| error("USB interface unavailable"))?;
        if let Operation::TransferIn { endpoint, .. }
        | Operation::TransferOut { endpoint, .. }
        | Operation::ClearHalt { endpoint } = &op
        {
            if !self
                .descriptor
                .interfaces
                .iter()
                .filter(|i| {
                    i.configuration == selection.configuration
                        && i.interface == selection.interface
                        && i.alternate == selection.alternate
                })
                .flat_map(|i| &i.endpoints)
                .any(|ep| {
                    ep.address == *endpoint && matches!(ep.kind.as_str(), "bulk" | "interrupt")
                })
            {
                return Err(error("Endpoint unavailable"));
            }
        }
        if matches!(&op, Operation::ControlIn { length, .. } if *length > u16::MAX as usize)
            || matches!(&op, Operation::ControlOut { data, .. } if data.len() > u16::MAX as usize)
        {
            return Err(error("USB control transfer exceeds 65535 bytes"));
        }
        if let Operation::ControlOut { setup, data } = &op
            && matches!(setup.request_type, RequestType::Vendor)
            && matches!(setup.recipient, Recipient::Other)
            && setup.request == 0
            && setup.value == 0
            && setup.index == 0
        {
            self.observe_control(data);
        }
        let observed = op.clone();
        let result = self.transfer(&iface, selection, op, timeout).await;
        match (&observed, &result) {
            (Operation::ControlOut { .. }, Ok(reply)) if reply.error.is_none() => {
                self.observe_control_complete()
            }
            (Operation::TransferOut { data, .. }, Ok(reply)) => self.observe_payload(
                false,
                data,
                data.len(),
                reply.transferred,
                reply.error.clone(),
            ),
            (Operation::TransferIn { length, .. }, Ok(reply)) => self.observe_payload(
                true,
                &reply.data,
                *length,
                reply.transferred,
                reply.error.clone(),
            ),
            (_, Err(error)) => self.observe_failure(&error.to_string()),
            _ => (),
        }
        if result.is_err() || result.as_ref().is_ok_and(|reply| reply.error.is_some()) {
            self.state.lock().unwrap().fenced = true;
            self.publish();
        }
        result
    }
    async fn transfer(
        &self,
        iface: &UsbInterface,
        selection: Selection,
        op: Operation,
        timeout: Duration,
    ) -> Result<Completion> {
        let mut reply = Completion::default();
        match op {
            Operation::ControlIn { setup, length } => {
                reply.data = iface
                    .control_in(
                        ControlIn {
                            control_type: control_type(setup.request_type),
                            recipient: recipient(setup.recipient),
                            request: setup.request,
                            value: setup.value,
                            index: setup.index,
                            length: u16::try_from(length).map_err(error)?,
                        },
                        timeout,
                    )
                    .await
                    .map_err(error)?;
                reply.transferred = reply.data.len();
            }
            Operation::ControlOut { setup, data } => {
                iface
                    .control_out(
                        ControlOut {
                            control_type: control_type(setup.request_type),
                            recipient: recipient(setup.recipient),
                            request: setup.request,
                            value: setup.value,
                            index: setup.index,
                            data: &data,
                        },
                        timeout,
                    )
                    .await
                    .map_err(error)?;
                reply.transferred = data.len();
            }
            op @ (Operation::TransferIn { .. }
            | Operation::TransferOut { .. }
            | Operation::ClearHalt { .. }) => {
                let address = match &op {
                    Operation::TransferIn { endpoint, .. }
                    | Operation::TransferOut { endpoint, .. }
                    | Operation::ClearHalt { endpoint } => *endpoint,
                    _ => unreachable!(),
                };
                let descriptor = self
                    .descriptor
                    .interfaces
                    .iter()
                    .find(|i| {
                        i.configuration == selection.configuration
                            && i.interface == selection.interface
                            && i.alternate == selection.alternate
                    })
                    .and_then(|i| i.endpoints.iter().find(|ep| ep.address == address))
                    .ok_or_else(|| error("Endpoint unavailable"))?;
                macro_rules! input {
                    ($kind:ty) => {{
                        let mut ep = iface.endpoint::<$kind, In>(address).map_err(error)?;
                        if matches!(op, Operation::ClearHalt { .. }) {
                            ep.clear_halt().await.map_err(error)?;
                        } else if let Operation::TransferIn { length, .. } = op {
                            ep.submit(Buffer::new(length));
                            let done = ep.next_complete().await;
                            reply.data = done.buffer.to_vec();
                            reply.transferred = done.actual_len;
                            reply.error = done.status.err().map(transfer_error);
                        } else {
                            return Err(error("Invalid endpoint direction"));
                        }
                    }};
                }
                macro_rules! output {
                    ($kind:ty) => {{
                        let mut ep = iface.endpoint::<$kind, Out>(address).map_err(error)?;
                        if matches!(op, Operation::ClearHalt { .. }) {
                            ep.clear_halt().await.map_err(error)?;
                        } else if let Operation::TransferOut { data, .. } = op {
                            let expected = data.len();
                            ep.submit(data.into());
                            let done = ep.next_complete().await;
                            reply.transferred = done.actual_len;
                            reply.error = done.status.err().map(transfer_error).or_else(|| {
                                (done.actual_len != expected).then(|| "short OUT transfer".into())
                            });
                        } else {
                            return Err(error("Invalid endpoint direction"));
                        }
                    }};
                }
                match (descriptor.kind.as_str(), address & 0x80 != 0) {
                    ("bulk", true) => input!(Bulk),
                    ("bulk", false) => output!(Bulk),
                    ("interrupt", true) => input!(Interrupt),
                    ("interrupt", false) => output!(Interrupt),
                    _ => return Err(error("Unsupported endpoint kind")),
                }
            }
            _ => return Err(error("Unsupported USB operation")),
        }
        Ok(reply)
    }
}
fn transfer_error(value: nusb::transfer::TransferError) -> String {
    match value {
        nusb::transfer::TransferError::Stall => "stall".into(),
        nusb::transfer::TransferError::Disconnected => "NotFoundError".into(),
        nusb::transfer::TransferError::Fault => "babble".into(),
        other => other.to_string(),
    }
}
fn control_type(value: RequestType) -> ControlType {
    match value {
        RequestType::Standard => ControlType::Standard,
        RequestType::Class => ControlType::Class,
        RequestType::Vendor => ControlType::Vendor,
    }
}
fn recipient(value: Recipient) -> nusb::transfer::Recipient {
    match value {
        Recipient::Device => nusb::transfer::Recipient::Device,
        Recipient::Interface => nusb::transfer::Recipient::Interface,
        Recipient::Endpoint => nusb::transfer::Recipient::Endpoint,
        Recipient::Other => nusb::transfer::Recipient::Other,
    }
}

/// The browser exporter runs the same wire operations as a remote controller.
/// Its owners can simultaneously be referenced by direct local Device handles.
pub struct Exporter {
    devices: BTreeMap<String, Arc<DeviceOwner>>,
}
impl Exporter {
    pub fn new(devices: impl IntoIterator<Item = Arc<DeviceOwner>>) -> Self {
        Self {
            devices: devices
                .into_iter()
                .map(|d| (d.descriptor.id.clone(), d))
                .collect(),
        }
    }
    pub fn inventory(&self) -> Vec<Device> {
        self.devices
            .values()
            .map(|d| d.descriptor.clone())
            .collect()
    }
    pub fn busy(&self, device: &str) -> bool {
        self.devices
            .get(device)
            .is_some_and(|d| d.state.lock().unwrap().claim.is_some())
    }
    pub fn owns(&self, controller: &str, device: &str) -> bool {
        self.devices.get(device).is_some_and(|d| d.owns(controller))
    }
    pub fn abandon(&self, controller: &str) -> bool {
        self.devices
            .values()
            .fold(false, |held, d| d.abandon(controller) || held)
    }
    pub fn fenced(&self, device: &str) -> bool {
        self.devices.get(device).is_some_and(|d| d.fenced())
    }
    pub fn stop_sharing(&self) {
        for d in self.devices.values() {
            d.stop_sharing();
        }
    }
    pub async fn shutdown(&self) -> Result<()> {
        for d in self.devices.values() {
            d.shutdown().await?;
        }
        Ok(())
    }
    pub async fn handle(&self, controller: &str, bytes: &[u8]) -> Result<Vec<u8>> {
        let Message::Execute {
            seq,
            device,
            timeout_ms,
            op,
        } = Message::decode(bytes)?
        else {
            return Err(error("Expected proxy execute message"));
        };
        op.validate()?;
        if timeout_ms == 0 || timeout_ms > 120000 {
            return Err(error("Invalid USB timeout"));
        }
        let completion = match op {
            Operation::Probe => Completion::default(),
            Operation::List => Completion {
                devices: self.inventory(),
                ..Default::default()
            },
            op => {
                let owner = self
                    .devices
                    .get(&device)
                    .ok_or_else(|| error("Unknown USB device"))?;
                let timeout = Duration::from_millis(timeout_ms.into());
                let result =
                    futures_lite::future::race(owner.execute(controller, op, timeout), async {
                        futures_timer::Delay::new(timeout).await;
                        owner.state.lock().unwrap().fenced = true;
                        owner.publish();
                        owner.observe_failure("USB deadline expired; completion unknown");
                        Err(error("USB deadline expired; completion unknown"))
                    })
                    .await;
                result.unwrap_or_else(|e| Completion {
                    error: Some(e.0),
                    ..Default::default()
                })
            }
        };
        Message::Result { seq, completion }.encode()
    }
}

/// One page's registry: repeated discovery and export reuse the same Rust owner.
#[derive(Default)]
pub struct DeviceOwners {
    devices: async_lock::Mutex<(u64, HashMap<nusb::DeviceId, Arc<DeviceOwner>>)>,
}
impl DeviceOwners {
    pub async fn presence(&self, id: nusb::DeviceId, present: bool) {
        if let Some(owner) = self.devices.lock().await.1.get(&id) {
            owner.set_present(present);
        }
    }
    pub async fn get(&self, info: nusb::DeviceInfo) -> Result<Arc<DeviceOwner>> {
        let mut devices = self.devices.lock().await;
        if let Some(owner) = devices.1.get(&info.id()).cloned() {
            if !owner.status().open {
                owner.reopen().await?;
            }
            return Ok(owner);
        }
        devices.0 += 1;
        let id = format!("usb-{}", devices.0);
        let key = info.id();
        let owner = DeviceOwner::open(info, id).await?;
        devices.1.insert(key, owner.clone());
        Ok(owner)
    }
}
