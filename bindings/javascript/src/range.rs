//! Language lifetime management for the core's owned, pull-based readers.
use super::*;
use error::{bytes, nz};
use futures_channel::oneshot;
use requests::DataWindow;
use session::{Cancellation, Session, Slot, take_native};
use std::{cell::RefCell, rc::Rc};
use usb::ff80::{
    Session as NativeSession,
    commands::memory::{Address, MemoryRange},
};

enum NativeReader {
    Memory(usb::operations::memory::MemoryReader<NativeSession>),
    Data(usb::operations::data::DataReader<NativeSession>),
}
impl NativeReader {
    fn completed(&self) -> u32 {
        match self {
            Self::Memory(r) => r.bytes_completed(),
            Self::Data(r) => r.bytes_completed(),
        }
    }
    fn into_session(self) -> NativeSession {
        match self {
            Self::Memory(r) => r.into_inner(),
            Self::Data(r) => r.into_inner(),
        }
    }
    async fn next(&mut self) -> usb::Result<Option<Vec<u8>>> {
        match self {
            Self::Memory(r) => r.next_chunk().await,
            Self::Data(r) => r.next_chunk().await,
        }
    }
}
pub(crate) struct ReaderState {
    reader: Option<NativeReader>,
    owner: Rc<RefCell<Slot>>,
    completed: u32,
    busy: bool,
    closed: bool,
    cancel: Option<oneshot::Sender<Cancellation>>,
}
impl ReaderState {
    fn restore(&mut self) {
        if let Some(reader) = self.reader.take() {
            self.completed = reader.completed();
            let native = reader.into_session();
            let mut owner = self.owner.borrow_mut();
            if matches!(*owner, Slot::Range(_)) {
                *owner = Slot::Open(native);
            }
        }
    }
    pub(crate) fn abandon(&mut self) {
        self.closed = true;
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(Cancellation::Abandoned);
        }
        // SessionSlot is already Abandoned. Dropping never emits C4.
        self.reader.take();
    }
}
impl Drop for ReaderState {
    fn drop(&mut self) {
        self.restore();
    }
}
struct ChunkLease {
    reader: Option<NativeReader>,
    state: Rc<RefCell<ReaderState>>,
}
impl Drop for ChunkLease {
    fn drop(&mut self) {
        let mut state = self.state.borrow_mut();
        state.busy = false;
        state.cancel.take();
        if let Some(reader) = self.reader.take() {
            state.completed = reader.completed();
            state.reader = Some(reader);
        }
        if state.closed {
            state.restore();
        }
    }
}
#[wasm_bindgen]
pub struct RangeReader {
    inner: Rc<RefCell<ReaderState>>,
}
#[wasm_bindgen]
impl RangeReader {
    #[wasm_bindgen(getter,js_name=bytesCompleted)]
    pub fn bytes_completed(&self) -> u32 {
        self.inner.borrow().completed
    }
    #[wasm_bindgen(getter)]
    pub fn closed(&self) -> bool {
        self.inner.borrow().closed
    }
    #[wasm_bindgen(js_name=nextChunk,unchecked_return_type="Promise<Uint8Array | undefined>")]
    pub fn next_chunk(&self) -> Result<js_sys::Promise, JsValue> {
        let mut state = self.inner.borrow_mut();
        if state.closed {
            return Ok(js_sys::Promise::resolve(&JsValue::UNDEFINED));
        }
        if state.busy {
            return Err(fail(
                ErrorKind::Busy,
                "range reader already has a pending chunk",
            ));
        }
        let reader = state
            .reader
            .take()
            .expect("idle open range owns its core reader");
        let (cancel, mut canceled) = oneshot::channel();
        state.cancel = Some(cancel);
        state.busy = true;
        let mut lease = ChunkLease {
            reader: Some(reader),
            state: self.inner.clone(),
        };
        drop(state);
        Ok(wasm_bindgen_futures::future_to_promise(async move {
            let result = futures_lite::future::or(
                async {
                    Err((&mut canceled)
                        .await
                        .unwrap_or(Cancellation::Abandoned)
                        .error())
                },
                async {
                    lease
                        .reader
                        .as_mut()
                        .expect("chunk lease")
                        .next()
                        .await
                        .map_err(error)
                },
            )
            .await;
            match result {
                Ok(Some(data)) => Ok(bytes(data)),
                Ok(None) => {
                    lease.state.borrow_mut().closed = true;
                    Ok(JsValue::UNDEFINED)
                }
                Err(error) => {
                    lease.state.borrow_mut().closed = true;
                    Err(error)
                }
            }
        }))
    }
    /// Release the range lease without sending a camera command. Abandon the
    /// session explicitly to cancel a currently pending chunk.
    pub fn close(&self) -> Result<(), JsValue> {
        let mut state = self.inner.borrow_mut();
        if state.busy {
            return Err(fail(
                ErrorKind::Busy,
                "range chunk is pending; await it or abandon the session",
            ));
        }
        state.closed = true;
        state.restore();
        Ok(())
    }
    /// Cancel a pending chunk and close the reader. Await its Promise before
    /// issuing another session operation. No USB cleanup is sent.
    #[wasm_bindgen(js_name=cancelPending)]
    pub fn cancel_pending(&self) {
        let mut state = self.inner.borrow_mut();
        state.closed = true;
        if let Some(cancel) = state.cancel.take() {
            let _ = cancel.send(Cancellation::Interrupted);
        } else if !state.busy {
            state.restore();
        }
    }
}
fn create(
    session: &Session,
    range: MemoryRange,
    window: Option<usb::operations::data::DataWindow>,
) -> Result<RangeReader, JsValue> {
    if let Some(window) = window {
        window.validate(range).map_err(error)?;
    }
    let native = take_native(&session.owner, Slot::Range(std::rc::Weak::new()), false)?;
    let reader = match window {
        Some(window) => NativeReader::Data(
            usb::operations::data::DataReader::new(native, window, range)
                .expect("window checked before taking session"),
        ),
        None => NativeReader::Memory(usb::operations::memory::MemoryReader::new(
            native,
            range,
            session.limits.memory,
        )),
    };
    let inner = Rc::new(RefCell::new(ReaderState {
        reader: Some(reader),
        owner: session.owner.clone(),
        completed: 0,
        busy: false,
        closed: false,
        cancel: None,
    }));
    *session.owner.borrow_mut() = Slot::Range(Rc::downgrade(&inner));
    Ok(RangeReader { inner })
}
#[wasm_bindgen(js_name=memoryChunks)]
pub fn memory_chunks(
    session: &Session,
    #[wasm_bindgen(unchecked_param_type = "number")] address: JsValue,
    #[wasm_bindgen(unchecked_param_type = "number")] length: JsValue,
) -> Result<RangeReader, JsValue> {
    let address = error::integer(address, u32::MAX)?;
    let length = error::integer(length, u32::MAX)?;

    create(
        session,
        MemoryRange::new(Address(address), nz(length)?).map_err(error)?,
        None,
    )
}
#[wasm_bindgen(js_name=dataChunks)]
pub fn data_chunks(
    session: &Session,
    window: &DataWindow,
    #[wasm_bindgen(unchecked_param_type = "number")] address: JsValue,
    #[wasm_bindgen(unchecked_param_type = "number")] length: JsValue,
) -> Result<RangeReader, JsValue> {
    let address = error::integer(address, u32::MAX)?;
    let length = error::integer(length, u32::MAX)?;

    create(
        session,
        MemoryRange::new(Address(address), nz(length)?).map_err(error)?,
        Some(window.inner),
    )
}
