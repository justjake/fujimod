//! The JavaScript bindings with synthetic USB endpoints, built only for tests; the
//! shipped package has no device injection path.
#![cfg(target_arch = "wasm32")]
#![forbid(unsafe_code)]
use fujimod_javascript::{
    error::{self, ErrorKind, error, fail},
    session::Session,
    types::{FirmwareLimits, TransferOptions},
};
use fujimod_usb as usb;
use wasm_bindgen::prelude::*;
#[wasm_bindgen]
#[derive(Clone)]
pub struct TestExchange {
    inner: usb::testing::Exchange,
}
#[wasm_bindgen]
impl TestExchange {
    pub fn control(
        #[wasm_bindgen(unchecked_param_type = "Uint8Array")] bytes: JsValue,
    ) -> Result<Self, JsValue> {
        let bytes = error::buffer(bytes)?;

        Ok(Self {
            inner: usb::testing::Exchange::Control(bytes.try_into().map_err(|_| {
                fail(
                    ErrorKind::InvalidArgument,
                    "control envelope must contain 12 bytes",
                )
            })?),
        })
    }
    pub fn write(
        #[wasm_bindgen(unchecked_param_type = "Uint8Array")] bytes: JsValue,
    ) -> Result<Self, JsValue> {
        let bytes = error::buffer(bytes)?;
        Ok({
            Self {
                inner: usb::testing::Exchange::Write(bytes),
            }
        })
    }
    pub fn read(
        #[wasm_bindgen(unchecked_param_type = "number")] maximum: JsValue,
        #[wasm_bindgen(unchecked_param_type = "Uint8Array")] data: JsValue,
    ) -> Result<Self, JsValue> {
        let data = error::buffer(data)?;

        let maximum = error::integer(maximum, u32::MAX)?;
        Ok({
            Self {
                inner: usb::testing::Exchange::Read {
                    maximum: maximum as usize,
                    data,
                },
            }
        })
    }
    pub fn disconnected(
        #[wasm_bindgen(unchecked_param_type = "number")] maximum: JsValue,
        #[wasm_bindgen(unchecked_param_type = "Uint8Array")] partial: JsValue,
    ) -> Result<Self, JsValue> {
        let partial = error::buffer(partial)?;

        let maximum = error::integer(maximum, u32::MAX)?;
        Ok({
            Self {
                inner: usb::testing::Exchange::ReadFailure {
                    maximum: maximum as usize,
                    error: nusb::transfer::TransferError::Disconnected,
                    partial,
                },
            }
        })
    }
    #[wasm_bindgen(js_name=pendingRead)]
    pub fn pending_read(
        #[wasm_bindgen(unchecked_param_type = "number")] maximum: JsValue,
    ) -> Result<Self, JsValue> {
        let maximum = error::integer(maximum, u32::MAX)?;
        Ok({
            Self {
                inner: usb::testing::Exchange::PendingRead {
                    maximum: maximum as usize,
                },
            }
        })
    }
}
#[wasm_bindgen]
pub struct TestScript {
    inner: usb::testing::Script,
}
#[wasm_bindgen]
impl TestScript {
    #[wasm_bindgen(constructor)]
    pub fn new(exchanges: Vec<TestExchange>) -> Self {
        Self {
            inner: usb::testing::Script::new(exchanges.into_iter().map(|e| e.inner)),
        }
    }
    #[wasm_bindgen(getter)]
    pub fn remaining(&self) -> usize {
        self.inner.remaining()
    }
    #[wasm_bindgen(unchecked_return_type = "Promise<Session>")]
    pub fn open(&self, limits: &FirmwareLimits, options: &TransferOptions) -> js_sys::Promise {
        let script = self.inner.clone();
        let limits = *limits;
        let max = options.max_reply_bytes();
        let trace = options.trace();
        wasm_bindgen_futures::future_to_promise(async move {
            Ok(Session::wrap(
                script.begin_session(max, trace).await.map_err(error)?,
                limits,
            )
            .into())
        })
    }
}
