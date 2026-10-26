use super::*;
use crate::requests::DataWindow;
use crate::sequence::CommandSequence;
use crate::session::{Operation, Session};
use fujimod_workflows::install;
use usb::ff80::commands::{
    diagnostic::DiagnosticTag,
    memory::{Address, MemoryRange},
};

#[wasm_bindgen]
pub struct InstallQueries {
    inner: usb::operations::borrowed::BorrowedQueries,
}
#[wasm_bindgen]
impl InstallQueries {
    #[wasm_bindgen(constructor)]
    pub fn new(
        #[wasm_bindgen(unchecked_param_type = "number")] callback: JsValue,
        #[wasm_bindgen(unchecked_param_type = "number")] stock: JsValue,
        #[wasm_bindgen(unchecked_param_type = "number")] output: JsValue,
        #[wasm_bindgen(unchecked_param_type = "Uint8Array")] original: JsValue,
    ) -> Result<Self, JsValue> {
        Ok(Self {
            inner: usb::operations::borrowed::BorrowedQueries::new(
                Address(error::integer(callback, u32::MAX)?),
                Address(error::integer(stock, u32::MAX)?),
                Address(error::integer(output, u32::MAX)?),
                error::buffer(original)?,
            )
            .map_err(error)?,
        })
    }
    pub fn call(
        &mut self,
        #[wasm_bindgen(unchecked_param_type = "number")] callback: JsValue,
        #[wasm_bindgen(unchecked_param_type = "number")] tag: JsValue,
        #[wasm_bindgen(unchecked_param_type="ReadonlyArray<readonly [number, number]>")] statuses: JsValue,
        #[wasm_bindgen(unchecked_param_type = "Uint8Array")] output: JsValue,
    ) -> Result<Vec<u32>, JsValue> {
        let (query, result) = self
            .inner
            .call(
                Address(error::integer(callback, u32::MAX)?),
                DiagnosticTag::Research(error::integer(tag, u32::MAX)?),
                crate::sequence::parse_statuses(statuses)?,
                error::buffer(output)?,
            )
            .map_err(error)?;
        Ok(vec![query as u32, result as u32])
    }
    pub fn finish(&self) -> CommandSequence {
        CommandSequence {
            commands: self.inner.commands(),
        }
    }
}

#[wasm_bindgen]
#[derive(Clone)]
pub struct OwnedUpload {
    pub(crate) inner: install::OwnedUpload,
}
#[wasm_bindgen]
impl OwnedUpload {
    #[wasm_bindgen(constructor)]
    pub fn new(
        #[wasm_bindgen(unchecked_param_type = "number")] allocation_address: JsValue,
        #[wasm_bindgen(unchecked_param_type = "number")] allocation_size: JsValue,
        #[wasm_bindgen(unchecked_param_type = "number")] address: JsValue,
        #[wasm_bindgen(unchecked_param_type = "Uint8Array")] data: JsValue,
        #[wasm_bindgen(unchecked_param_type = "number")] piece_bytes: JsValue,
        #[wasm_bindgen(unchecked_param_type = "number")] alignment: JsValue,
    ) -> Result<Self, JsValue> {
        let allocation = MemoryRange::new(
            Address(error::integer(allocation_address, u32::MAX)?),
            error::nz(error::integer(allocation_size, u32::MAX)?)?,
        )
        .map_err(error)?;
        Ok(Self {
            inner: install::OwnedUpload::new(
                allocation,
                Address(error::integer(address, u32::MAX)?),
                error::buffer(data)?,
                error::nz(error::integer(piece_bytes, u32::MAX)?)?,
                error::nz(error::integer(alignment, u32::MAX)?)?,
            )
            .map_err(error)?,
        })
    }
    #[wasm_bindgen(js_name=paddedSize)]
    pub fn padded_size(
        #[wasm_bindgen(unchecked_param_type = "number")] length: JsValue,
        #[wasm_bindgen(unchecked_param_type = "number")] piece_bytes: JsValue,
    ) -> Result<u32, JsValue> {
        install::padded_size(
            error::integer(length, u32::MAX)?,
            error::nz(error::integer(piece_bytes, u32::MAX)?)?,
        )
        .map(|n| n.get())
        .map_err(error)
    }
    #[wasm_bindgen(getter,js_name=imageBytes)]
    pub fn image_bytes(&self) -> u32 {
        self.inner.image_bytes()
    }
    #[wasm_bindgen(getter,js_name=paddedBytes)]
    pub fn padded_bytes(&self) -> u32 {
        self.inner.padded_bytes()
    }
}

#[wasm_bindgen]
impl CommandSequence {
    #[wasm_bindgen(js_name=verifiedWrites,unchecked_return_type="CommandSequence[]")]
    pub fn verified_writes(
        #[wasm_bindgen(unchecked_param_type = "number")] address: JsValue,
        #[wasm_bindgen(unchecked_param_type = "Uint8Array")] data: JsValue,
        limits: &FirmwareLimits,
        #[wasm_bindgen(unchecked_param_type = "number")] packet_bytes: JsValue,
    ) -> Result<js_sys::Array, JsValue> {
        usb::operations::verified::verified_writes(
            Address(error::integer(address, u32::MAX)?),
            &error::buffer(data)?,
            limits.memory,
            error::nz(error::integer(packet_bytes, u32::MAX)?)?,
        )
        .map_err(error)
        .map(|batches| {
            batches
                .into_iter()
                .map(|commands| JsValue::from(CommandSequence { commands }))
                .collect()
        })
    }
}

#[wasm_bindgen]
impl Session {
    #[wasm_bindgen(js_name=uploadOwned,unchecked_return_type="Promise<UploadReply>")]
    pub fn upload_owned(
        &self,
        image: &OwnedUpload,
        #[wasm_bindgen(unchecked_param_type = "number")] pointer: JsValue,
        #[wasm_bindgen(unchecked_param_type = "number")] original: JsValue,
        #[wasm_bindgen(unchecked_param_type = "number")] capacity_address: JsValue,
        #[wasm_bindgen(unchecked_param_type = "number")] capacity: JsValue,
    ) -> Result<js_sys::Promise, JsValue> {
        let destination = install::UploadDestination::new(
            Address(error::integer(pointer, u32::MAX)?),
            Address(error::integer(original, u32::MAX)?),
            Address(error::integer(capacity_address, u32::MAX)?),
            error::nz(error::integer(capacity, u32::MAX)?)?,
        )
        .map_err(error)?;
        self.call(Operation::UploadOwned(
            image.inner.clone(),
            destination,
            self.limits.memory,
        ))
    }
    #[wasm_bindgen(js_name=verifyOwned,unchecked_return_type="Promise<void>")]
    pub fn verify_owned(
        &self,
        image: &OwnedUpload,
        window: &DataWindow,
    ) -> Result<js_sys::Promise, JsValue> {
        self.call(Operation::VerifyOwned(image.inner.clone(), window.inner))
    }
}
