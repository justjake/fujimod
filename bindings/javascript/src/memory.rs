//! Session memory: one-packet commands for small ranges and the firmware's
//! qualified bulk path for larger reads.
use super::*;
use error::{bytes, nz};
use fujimod_catalog_bulk::BulkRange;
use fujimod_workflows::{Limits, bulk::BulkMapping};
use session::{Operation, Session};
use std::{cell::RefCell, rc::Rc};
use usb::ff80::{
    self,
    commands::memory::{Address, MemoryRange, ReadMemory, WriteMemory},
};

/// Reads and writes of any size within one session. Reads larger than one
/// packet use the bulk mapping qualified when the handle opened.
#[wasm_bindgen]
pub struct Memory {
    session: Session,
    mapping: Rc<RefCell<Option<BulkMapping>>>,
}
pub(crate) enum MemoryOperation {
    Open {
        facts: fujimod_catalog_bulk::FirmwareBulk,
        limits: Limits,
        memory: Memory,
    },
    Read(Rc<RefCell<Option<BulkMapping>>>, MemoryRange, Limits),
    Write(Rc<RefCell<Option<BulkMapping>>>, Address, Vec<u8>, Limits),
    Close(Rc<RefCell<Option<BulkMapping>>>),
}
fn bulk_error(source: fujimod_workflows::bulk::BulkError) -> JsValue {
    match source {
        fujimod_workflows::bulk::BulkError::Usb(source) => error(source),
        other => crate::workflows::workflow_error(other),
    }
}
fn closed() -> JsValue {
    fail(ErrorKind::Closed, "memory handle is closed")
}
impl MemoryOperation {
    pub(crate) async fn run(self, s: &mut ff80::Session) -> Result<JsValue, JsValue> {
        let packet = s.info().advertised_packet_bytes.get();
        match self {
            Self::Open {
                facts,
                limits,
                memory,
            } => {
                let mapping = BulkMapping::prepare(s, &facts, limits)
                    .await
                    .map_err(bulk_error)?;
                *memory.mapping.borrow_mut() = Some(mapping);
                Ok(memory.into())
            }
            Self::Read(cell, range, limits) => {
                if range.length().get() <= packet {
                    let command = ReadMemory::new(range, limits.memory).map_err(error)?;
                    return s.read_memory(command).await.map(bytes).map_err(error);
                }
                // Operations on one session never overlap, so the mapping is
                // taken for the read and returned afterwards.
                let mapping = cell.borrow_mut().take().ok_or_else(closed)?;
                let target = BulkRange {
                    address: u64::from(range.address().0),
                    size: u64::from(range.length().get()),
                };
                let result = mapping.read(s, target).await;
                *cell.borrow_mut() = Some(mapping);
                result.map(bytes).map_err(bulk_error)
            }
            Self::Write(cell, address, data, limits) => {
                if cell.borrow().is_none() {
                    return Err(closed());
                }
                if data.len() as u64 <= u64::from(packet) {
                    let command = WriteMemory::new(address, data, limits.memory).map_err(error)?;
                    s.write_memory(command).await.map_err(error)?;
                } else {
                    // The camera has no bulk write; larger writes are split
                    // into packet-sized commands.
                    usb::operations::memory::write_range(s, address, &data, limits.memory)
                        .await
                        .map_err(error)?;
                }
                Ok(JsValue::UNDEFINED)
            }
            Self::Close(cell) => {
                let Some(mapping) = cell.borrow_mut().take() else {
                    return Ok(JsValue::UNDEFINED);
                };
                mapping
                    .restore(s)
                    .await
                    .map(|_| JsValue::UNDEFINED)
                    .map_err(bulk_error)
            }
        }
    }
}
fn limits(session: &Session) -> Limits {
    Limits {
        memory: session.limits.memory,
        configuration: session.limits.configuration,
    }
}
#[wasm_bindgen]
impl Session {
    /// Qualify the firmware's stock bulk path once and return a handle for reads
    /// and writes of any size. Close it to restore any bulk source override.
    #[wasm_bindgen(unchecked_return_type = "Promise<Memory>")]
    pub fn memory(&self, firmware_id: &str) -> Result<js_sys::Promise, JsValue> {
        let facts = crate::reference::bundled()?
            .bulk(firmware_id)
            .cloned()
            .ok_or_else(|| {
                crate::workflows::workflow_error(fujimod_catalog::generation::missing_table_reason(
                    firmware_id,
                    "bulk",
                ))
            })?;
        self.call(Operation::Memory(Box::new(MemoryOperation::Open {
            facts,
            limits: limits(self),
            memory: Memory {
                session: self.share(),
                mapping: Rc::new(RefCell::new(None)),
            },
        })))
    }
}
#[wasm_bindgen]
impl Memory {
    #[wasm_bindgen(unchecked_return_type = "Promise<Uint8Array>")]
    pub fn read(
        &self,
        #[wasm_bindgen(unchecked_param_type = "number")] address: JsValue,
        #[wasm_bindgen(unchecked_param_type = "number")] length: JsValue,
    ) -> Result<js_sys::Promise, JsValue> {
        let address = error::integer(address, u32::MAX)?;
        let length = error::integer(length, u32::MAX)?;
        let range = MemoryRange::new(Address(address), nz(length)?).map_err(error)?;
        self.session
            .call(Operation::Memory(Box::new(MemoryOperation::Read(
                self.mapping.clone(),
                range,
                limits(&self.session),
            ))))
    }
    #[wasm_bindgen(unchecked_return_type = "Promise<void>")]
    pub fn write(
        &self,
        #[wasm_bindgen(unchecked_param_type = "number")] address: JsValue,
        #[wasm_bindgen(unchecked_param_type = "Uint8Array")] data: JsValue,
    ) -> Result<js_sys::Promise, JsValue> {
        let data = error::buffer(data)?;
        let address = error::integer(address, u32::MAX)?;
        MemoryRange::new(
            Address(address),
            nz(u32::try_from(data.len()).map_err(|_| error(usb::Error::InvalidLength))?)?,
        )
        .map_err(error)?;
        self.session
            .call(Operation::Memory(Box::new(MemoryOperation::Write(
                self.mapping.clone(),
                Address(address),
                data,
                limits(&self.session),
            ))))
    }
    /// Diagnostic scratch the stock bulk setup overwrote, which may lie inside
    /// bytes this handle reads; null when the setup overwrote nothing.
    #[wasm_bindgen(
        getter,
        unchecked_return_type = "{ address: number; size: number } | null"
    )]
    pub fn overwritten(&self) -> JsValue {
        let Some((address, size)) = self
            .mapping
            .borrow()
            .as_ref()
            .and_then(BulkMapping::overwritten)
        else {
            return JsValue::NULL;
        };
        let range = js_sys::Object::new();
        crate::error::property(&range, "address", address as f64);
        crate::error::property(&range, "size", size);
        range.into()
    }
    /// Restore any bulk source override. Later reads and writes refuse.
    #[wasm_bindgen(unchecked_return_type = "Promise<void>")]
    pub fn close(&self) -> Result<js_sys::Promise, JsValue> {
        self.session
            .call(Operation::Memory(Box::new(MemoryOperation::Close(
                self.mapping.clone(),
            ))))
    }
}
