//! Browser ownership and wire export; all USB execution remains in Rust.
use super::*;
use fujimod_usb::owner::{DeviceOwner, DeviceOwners};
use futures_lite::StreamExt;
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::Rc,
    sync::Arc,
};
thread_local! {
    static WATCHING: Cell<bool> = const { Cell::new(false) };
    static PROXIES: RefCell<BTreeMap<String, Arc<fujimod_usb::remote::Observation>>> = const { RefCell::new(BTreeMap::new()) };
    static OWNERS: Rc<DeviceOwners> = Rc::new(DeviceOwners::default());
}
pub(crate) fn proxy_observation(
    remote: &fujimod_usb_proxy::RemoteDevice,
    source: &str,
    token: &str,
) -> Arc<fujimod_usb::remote::Observation> {
    let next = fujimod_usb::remote::Observation::new(
        source,
        token,
        &remote.descriptor.id,
        remote
            .descriptor
            .status
            .as_ref()
            .is_none_or(|status| status.online),
    );
    let observation = PROXIES.with(|registry| {
        registry
            .borrow_mut()
            .entry(next.status().id.clone())
            .or_insert(next)
            .clone()
    });
    if observation.closed() {
        observation.resume();
    }
    observation.presence(
        remote
            .descriptor
            .status
            .as_ref()
            .is_none_or(|status| status.online),
    );
    monitor_proxy(remote.clone(), observation.clone());
    observation
}
pub(crate) fn monitor_proxy(
    remote: fujimod_usb_proxy::RemoteDevice,
    observation: Arc<fujimod_usb::remote::Observation>,
) {
    if !observation.start_monitor() {
        return;
    }
    wasm_bindgen_futures::spawn_local(async move {
        let mut connection = None;
        while !observation.closed() {
            if connection.is_none() {
                connection = remote.connect().await.ok();
            }
            let result = if let Some(client) = &mut connection {
                client.devices().await.ok()
            } else {
                None
            };
            match result {
                Some(devices) => observation.presence(
                    devices
                        .iter()
                        .find(|device| device.id == remote.descriptor.id)
                        .is_some_and(|device| {
                            device.status.as_ref().is_none_or(|status| status.online)
                        }),
                ),
                None => {
                    observation.presence(false);
                    connection = None;
                }
            }
            futures_timer::Delay::new(std::time::Duration::from_secs(1)).await;
        }
        observation.monitoring_ended();
    });
}
pub(crate) async fn device_owner(info: nusb::DeviceInfo) -> Result<Arc<DeviceOwner>, JsValue> {
    let owners = OWNERS.with(Clone::clone);
    if !WATCHING.with(Cell::get) {
        let mut watch = nusb::watch_devices().map_err(|e| error(e.into()))?;
        WATCHING.with(|value| value.set(true));
        let registry = owners.clone();
        wasm_bindgen_futures::spawn_local(async move {
            while let Some(event) = watch.next().await {
                match event {
                    nusb::hotplug::HotplugEvent::Connected(info) => {
                        registry.presence(info.id(), true).await
                    }
                    nusb::hotplug::HotplugEvent::Disconnected(id) => {
                        registry.presence(id, false).await
                    }
                }
            }
        });
    }
    owners.get(info).await.map_err(|e| error(e.into()))
}
#[wasm_bindgen]
pub struct Exporter {
    inner: Rc<fujimod_usb::owner::Exporter>,
}
impl Exporter {
    pub(crate) fn from_owners(owners: Vec<Arc<DeviceOwner>>) -> Self {
        Self {
            inner: Rc::new(fujimod_usb::owner::Exporter::new(owners)),
        }
    }
}
#[wasm_bindgen]
impl Exporter {
    /// Discover browser-permitted devices; the page registry reuses live owners.
    pub async fn discover() -> Result<Exporter, JsValue> {
        let mut owners = Vec::new();
        for info in nusb::list_devices().await.map_err(|e| error(e.into()))? {
            let owner = device_owner(info).await?;
            owner.share().map_err(|e| error(e.into()))?;
            owners.push(owner);
        }
        Ok(Self::from_owners(owners))
    }
    #[wasm_bindgen(getter,js_name=inventoryJson)]
    pub fn inventory_json(&self) -> Result<String, JsValue> {
        serde_json::to_string(&self.inner.inventory())
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }
    #[wasm_bindgen(js_name=requestDevice)]
    pub async fn request_device() -> Result<Option<Exporter>, JsValue> {
        if nusb::request_device(&[nusb::DeviceSelector::all()])
            .await
            .map_err(|e| error(e.into()))?
            .is_none()
        {
            return Ok(None);
        }
        Ok(Some(Self::discover().await?))
    }
    #[wasm_bindgen(js_name=stopSharing)]
    pub fn stop_sharing(&self) {
        self.inner.stop_sharing();
    }
    pub fn busy(&self, device: &str) -> bool {
        self.inner.busy(device)
    }
    pub fn owns(&self, controller: &str, device: &str) -> bool {
        self.inner.owns(controller, device)
    }
    pub fn fenced(&self, device: &str) -> bool {
        self.inner.fenced(device)
    }
    pub fn abandon(&self, controller: &str) -> bool {
        self.inner.abandon(controller)
    }
    /// Decode and execute one proxy Execute frame, returning its Result frame.
    #[wasm_bindgen(unchecked_return_type = "Promise<Uint8Array>")]
    pub fn execute(&self, controller: String, bytes: &[u8]) -> js_sys::Promise {
        let exporter = self.inner.clone();
        let bytes = bytes.to_vec();
        wasm_bindgen_futures::future_to_promise(async move {
            let reply = exporter
                .handle(&controller, &bytes)
                .await
                .map_err(|e| error(e.into()))?;
            Ok(js_sys::Uint8Array::from(reply.as_slice()).into())
        })
    }
    #[wasm_bindgen(unchecked_return_type = "Promise<void>")]
    pub fn shutdown(&self) -> js_sys::Promise {
        let exporter = self.inner.clone();
        wasm_bindgen_futures::future_to_promise(async move {
            exporter.shutdown().await.map_err(|e| error(e.into()))?;
            Ok(JsValue::UNDEFINED)
        })
    }
}

#[wasm_bindgen(typescript_custom_section)]
const OWNER_TYPES: &str = r#"
export interface DeviceState {
    id: string;
    present: boolean;
    open: boolean;
    sharing: boolean;
    sessionOwner: { kind: 'local'; tool?: string } | { kind: 'remote'; controller: string } | null;
    fenced: boolean;
}
"#;
pub(crate) fn state_value(event: &fujimod_usb::owner::OwnerEvent) -> Result<JsValue, JsValue> {
    js_sys::JSON::parse(
        &serde_json::to_string(event).map_err(|e| JsValue::from_str(&e.to_string()))?,
    )
}
/// A per-device ordered subscription. The first item is the current state.
#[wasm_bindgen]
pub struct DeviceEvents {
    receiver: async_channel::Receiver<fujimod_usb::owner::OwnerEvent>,
}
impl DeviceEvents {
    pub(crate) fn new(owner: &DeviceOwner) -> Self {
        Self::from_receiver(owner.watch())
    }
    pub(crate) fn from_receiver(
        receiver: async_channel::Receiver<fujimod_usb::owner::OwnerEvent>,
    ) -> Self {
        Self { receiver }
    }
}
impl Drop for DeviceEvents {
    fn drop(&mut self) {
        self.receiver.close();
    }
}
#[wasm_bindgen]
impl DeviceEvents {
    #[wasm_bindgen(unchecked_return_type = "Promise<DeviceState | undefined>")]
    pub fn next(&self) -> js_sys::Promise {
        let receiver = self.receiver.clone();
        wasm_bindgen_futures::future_to_promise(async move {
            match receiver.recv().await {
                Ok(event) => state_value(&event),
                Err(_) => Ok(JsValue::UNDEFINED),
            }
        })
    }
    pub fn close(&self) {
        self.receiver.close();
    }
}

#[wasm_bindgen(typescript_custom_section)]
const LOG_TYPES: &str = r#"
export interface Ff80Exchange {
    deviceId: string;
    holder: NonNullable<DeviceState['sessionOwner']>;
    sequence: number;
    timestampMs: number;
    kind: number;
    session: number;
    operation: string;
    command: number | null;
    function: number | null;
    address: number | null;
    selector: number | null;
    page: number | null;
    length: number | null;
    requested: number;
    transferred: number;
    envelope: Uint8Array;
    data: Uint8Array;
    truncated: boolean;
    error: string | null;
    consoleData: Uint8Array | null;
}
"#;
#[wasm_bindgen]
pub struct LogEvents {
    inner: Arc<fujimod_usb::owner_log::LogEvents>,
}
impl LogEvents {
    pub(crate) fn new(owner: &DeviceOwner) -> Self {
        Self::from_events(owner.watch_log())
    }
    pub(crate) fn from_events(inner: Arc<fujimod_usb::owner_log::LogEvents>) -> Self {
        Self { inner }
    }
}
impl Drop for LogEvents {
    fn drop(&mut self) {
        self.inner.close();
    }
}
#[wasm_bindgen]
impl LogEvents {
    #[wasm_bindgen(unchecked_return_type = "Promise<Ff80Exchange | undefined>")]
    pub fn next(&self) -> js_sys::Promise {
        let events = self.inner.clone();
        wasm_bindgen_futures::future_to_promise(async move {
            let Some(event) = events.next().await.map_err(JsValue::from_str)? else {
                return Ok(JsValue::UNDEFINED);
            };
            let value = js_sys::JSON::parse(
                &serde_json::to_string(&event.decoded)
                    .map_err(|e| JsValue::from_str(&e.to_string()))?,
            )?;
            let set = |key: &str, item: JsValue| {
                js_sys::Reflect::set(&value, &JsValue::from_str(key), &item)
            };
            set("deviceId", JsValue::from_str(&event.device_id))?;
            set(
                "holder",
                js_sys::JSON::parse(
                    &serde_json::to_string(&event.holder)
                        .map_err(|e| JsValue::from_str(&e.to_string()))?,
                )?,
            )?;
            set("sequence", JsValue::from_f64(event.sequence as f64))?;
            set("timestampMs", JsValue::from_f64(event.timestamp_ms))?;
            set("kind", JsValue::from(event.kind))?;
            set("session", JsValue::from(event.session))?;
            set("requested", JsValue::from(event.requested))?;
            set("transferred", JsValue::from_f64(event.transferred as f64))?;
            set(
                "envelope",
                js_sys::Uint8Array::from(event.envelope.as_slice()).into(),
            )?;
            set(
                "data",
                js_sys::Uint8Array::from(event.data.as_slice()).into(),
            )?;
            set("truncated", JsValue::from_bool(event.truncated))?;
            set(
                "consoleData",
                event
                    .console_data
                    .as_deref()
                    .map(|data| js_sys::Uint8Array::from(data).into())
                    .unwrap_or(JsValue::NULL),
            )?;
            set(
                "error",
                event
                    .error
                    .as_deref()
                    .map(JsValue::from_str)
                    .unwrap_or(JsValue::NULL),
            )?;
            Ok(value)
        })
    }
    pub fn close(&self) {
        self.inner.close();
    }
}
