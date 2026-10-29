//! Cooperative Python Body adapter. A step performs no I/O: it polls until the
//! Rust installer requests one body operation, which Python awaits separately.
use super::*;
use fujimod_runtime_install as runtime;
use runtime::{Body, Pending};
use serde_json::{Value, json};
use std::task::Waker;

type Answer = std::result::Result<Vec<u8>, String>;
struct Request {
    kind: String,
    metadata: Value,
    bytes: Vec<u8>,
    answer: oneshot::Sender<Answer>,
}
struct Peer {
    queue: Arc<Mutex<Option<Request>>>,
}
impl Peer {
    fn request(&mut self, kind: &str, metadata: Value, bytes: Vec<u8>) -> Pending<'_, Vec<u8>> {
        let (tx, rx) = oneshot::channel();
        *self.queue.lock().expect("request queue") = Some(Request {
            kind: kind.into(),
            metadata,
            bytes,
            answer: tx,
        });
        Box::pin(async move {
            rx.await
                .map_err(|_| runtime::Error::Body("body adapter closed".into()))?
                .map_err(runtime::Error::Body)
        })
    }
    fn unit(&mut self, kind: &str, metadata: Value, bytes: Vec<u8>) -> Pending<'_, ()> {
        let future = self.request(kind, metadata, bytes);
        Box::pin(async move {
            let bytes = future.await?;
            if !bytes.is_empty() {
                return Err(runtime::Error::Body("unexpected body result".into()));
            }
            Ok(())
        })
    }
}
impl Body for Peer {
    fn allocate(&mut self, size: u64, align: u64) -> Pending<'_, u64> {
        let pending = self.request("allocate", json!({"size":size,"align":align}), vec![]);
        Box::pin(async move {
            Ok(u64::from_le_bytes(pending.await?.try_into().map_err(
                |_| runtime::Error::Body("allocator result size".into()),
            )?))
        })
    }
    fn free(&mut self, base: u64) -> Pending<'_, ()> {
        self.unit("free", json!({"base":base}), vec![])
    }
    fn read(&mut self, range: runtime::Range) -> Pending<'_, Vec<u8>> {
        self.request("read", json!(range), vec![])
    }
    fn write(&mut self, address: u64, bytes: Vec<u8>) -> Pending<'_, ()> {
        self.unit("write", json!({"address":address}), bytes)
    }
    fn synchronize(&mut self, range: runtime::Range) -> Pending<'_, ()> {
        self.unit("synchronize", json!(range), vec![])
    }
    fn protect(&mut self, range: runtime::Range) -> Pending<'_, ()> {
        self.unit("protect", json!(range), vec![])
    }
    fn control(&mut self, entry: u64, operation: u32) -> Pending<'_, ()> {
        self.unit(
            "control",
            json!({"entry":entry,"operation":operation}),
            vec![],
        )
    }
    fn placed(&mut self, image: &runtime::RuntimeImage, lease: &runtime::Lease) -> Pending<'_, ()> {
        self.unit(
            "placed",
            json!({"image":image,"lease":lease}),
            image.bytes.clone(),
        )
    }
}
fn invalid(error: impl ToString) -> PyErr {
    PyValueError::new_err(error.to_string())
}
#[pyclass(name = "RuntimeImage", from_py_object, module = "fujimod.usb")]
#[derive(Clone)]
pub(super) struct RuntimeImage {
    inner: runtime::RuntimeImage,
}
#[pymethods]
impl RuntimeImage {
    #[new]
    fn new(data: Vec<u8>, metadata: &str) -> PyResult<Self> {
        let mut inner: runtime::RuntimeImage = serde_json::from_str(metadata).map_err(invalid)?;
        inner.bytes = data;
        inner.validate().map_err(invalid)?;
        Ok(Self { inner })
    }
    fn relocate(&self, base: u64) -> PyResult<Self> {
        Ok(Self {
            inner: self.inner.relocate(base).map_err(invalid)?,
        })
    }
    #[getter]
    fn metadata(&self) -> PyResult<String> {
        serde_json::to_string(&self.inner).map_err(invalid)
    }
    #[getter]
    fn data(&self) -> Py<PyBytes> {
        bytes(&self.inner.bytes)
    }
    #[staticmethod]
    fn relocate_mov_pair(low: u32, high: u32, delta: i64) -> PyResult<(u32, u32)> {
        runtime::relocation::mov_pair(low, high, i128::from(delta)).map_err(invalid)
    }
}
struct Completed {
    installer: runtime::Installer,
    result: runtime::Result<Value>,
    output_data: Option<Vec<u8>>,
}
struct Driver {
    future: Pin<Box<dyn Future<Output = Completed> + Send>>,
    queue: Arc<Mutex<Option<Request>>>,
    answer: Option<oneshot::Sender<Answer>>,
    finished: bool,
}
#[pyclass(name = "RuntimeTask", frozen, module = "fujimod.usb")]
pub(super) struct RuntimeTask {
    driver: Mutex<Driver>,
}
#[pymethods]
impl RuntimeTask {
    #[new]
    #[pyo3(signature=(image,operation,*,lease=None,token=1,address=0,size=0,data=Vec::new(),relink=None))]
    fn new(
        image: RuntimeImage,
        operation: String,
        lease: Option<String>,
        token: u32,
        address: u64,
        size: u64,
        data: Vec<u8>,
        relink: Option<Py<PyAny>>,
    ) -> PyResult<Self> {
        let mut installer = match lease {
            Some(lease) => runtime::Installer::attach(
                image.inner,
                serde_json::from_str(&lease).map_err(invalid)?,
            ),
            None => runtime::Installer::plan(image.inner),
        }
        .map_err(invalid)?;
        if ![
            "prepare", "install", "publish", "init", "status", "rollback", "set", "read",
        ]
        .contains(&operation.as_str())
        {
            return Err(invalid("unknown runtime operation"));
        }
        let queue = Arc::new(Mutex::new(None));
        let mut peer = Peer {
            queue: queue.clone(),
        };
        let future = Box::pin(async move {
            let use_relink = relink.is_some();
            let mut rebuild = |base| -> runtime::Result<runtime::RuntimeImage> {
                Python::attach(|py| -> PyResult<runtime::RuntimeImage> {
                    let callback = relink.as_ref().ok_or_else(|| invalid("missing relinker"))?;
                    let result = callback.call1(py, (base,))?;
                    Ok(result.bind(py).extract::<RuntimeImage>()?.inner)
                })
                .map_err(|error| runtime::Error::Body(error.to_string()))
            };
            let placement = if use_relink {
                runtime::Placement::Relink(&mut rebuild)
            } else {
                runtime::Placement::Relocate
            };
            let mut output_data = None;
            let result: runtime::Result<Value> = async {
                let result = match operation.as_str() {
                    "prepare" => {
                        installer.prepare(&mut peer, placement).await?;
                        json!(null)
                    }
                    "install" => json!(installer.install(&mut peer, placement, token).await?),
                    "publish" => json!(installer.publish(&mut peer, token).await?),
                    "init" => json!(installer.init(&mut peer, token).await?),
                    "status" => json!(installer.status(&mut peer, token).await?),
                    "rollback" => json!(installer.rollback(&mut peer, token).await?),
                    "set" => {
                        installer.set(&mut peer, address, data).await?;
                        json!(null)
                    }
                    "read" => {
                        output_data = Some(
                            installer
                                .read(&mut peer, runtime::Range { address, size })
                                .await?,
                        );
                        json!(null)
                    }
                    _ => unreachable!(),
                };
                Ok(result)
            }
            .await;
            Completed {
                installer,
                result,
                output_data,
            }
        });
        Ok(Self {
            driver: Mutex::new(Driver {
                future,
                queue,
                answer: None,
                finished: false,
            }),
        })
    }
    /// Respond to the previous request, then return (kind, metadata, bytes).
    #[pyo3(signature=(response=None, failure=None))]
    fn step(
        &self,
        response: Option<Vec<u8>>,
        failure: Option<String>,
    ) -> PyResult<(String, String, Py<PyBytes>)> {
        let mut driver = lock(&self.driver)?;
        if driver.finished {
            return Err(invalid("runtime task already finished"));
        }
        if let Some(answer) = driver.answer.take() {
            if response.is_some() && failure.is_some() {
                return Err(invalid("response and failure are mutually exclusive"));
            }
            answer
                .send(failure.map_or_else(|| Ok(response.unwrap_or_default()), Err))
                .map_err(|_| invalid("runtime task closed"))?;
        } else if response.is_some() || failure.is_some() {
            return Err(invalid("no pending body operation"));
        }
        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);
        match driver.future.as_mut().poll(&mut cx) {
            Poll::Ready(done) => {
                driver.finished = true;
                let metadata = match done.result {
                    Ok(output) => {
                        json!({"image":done.installer.image,"lease":done.installer.lease,"output":output})
                    }
                    Err(error) => {
                        json!({"image":done.installer.image,"lease":done.installer.lease,"error":error.to_string()})
                    }
                };
                Ok((
                    "done".into(),
                    metadata.to_string(),
                    bytes(
                        done.output_data
                            .as_deref()
                            .unwrap_or(&done.installer.image.bytes),
                    ),
                ))
            }
            Poll::Pending => {
                let request = lock(&driver.queue)?
                    .take()
                    .ok_or_else(|| invalid("runtime task pending without request"))?;
                driver.answer = Some(request.answer);
                Ok((
                    request.kind,
                    request.metadata.to_string(),
                    bytes(request.bytes),
                ))
            }
        }
    }
}
pub(super) fn register(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<RuntimeImage>()?;
    module.add_class::<RuntimeTask>()
}
