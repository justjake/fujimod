use super::sequence::CommandSequence;
use super::*;
use fujimod_workflows::install;

#[pyclass(name = "InstallQueries", from_py_object, module = "fujimod.usb")]
#[derive(Clone)]
pub(super) struct InstallQueries {
    inner: usb::operations::borrowed::BorrowedQueries,
}
#[pymethods]
impl InstallQueries {
    #[new]
    fn new(callback: u32, stock: u32, output: u32, original: Vec<u8>) -> PyResult<Self> {
        Ok(Self {
            inner: usb::operations::borrowed::BorrowedQueries::new(
                Address(callback),
                Address(stock),
                Address(output),
                original,
            )
            .map_err(error)?,
        })
    }
    fn call(
        &mut self,
        callback: u32,
        tag: u32,
        statuses: Vec<(u32, u32)>,
        output: Vec<u8>,
    ) -> PyResult<(usize, usize)> {
        self.inner
            .call(
                Address(callback),
                diagnostic::DiagnosticTag::Research(tag),
                statuses,
                output,
            )
            .map_err(error)
    }
    fn finish(&self) -> CommandSequence {
        CommandSequence {
            commands: self.inner.commands(),
        }
    }
}

#[pyclass(name = "OwnedUpload", from_py_object, module = "fujimod.usb")]
#[derive(Clone)]
pub(super) struct OwnedUpload {
    inner: install::OwnedUpload,
}
#[pymethods]
impl OwnedUpload {
    #[new]
    fn new(
        allocation_address: u32,
        allocation_size: u32,
        address: u32,
        data: Vec<u8>,
        piece_bytes: u32,
        alignment: u32,
    ) -> PyResult<Self> {
        let allocation =
            MemoryRange::new(Address(allocation_address), nz(allocation_size)?).map_err(error)?;
        Ok(Self {
            inner: install::OwnedUpload::new(
                allocation,
                Address(address),
                data,
                nz(piece_bytes)?,
                nz(alignment)?,
            )
            .map_err(error)?,
        })
    }
    #[staticmethod]
    fn padded_size(length: u32, piece_bytes: u32) -> PyResult<u32> {
        install::padded_size(length, nz(piece_bytes)?)
            .map(|n| n.get())
            .map_err(error)
    }
    #[getter]
    fn image_bytes(&self) -> u32 {
        self.inner.image_bytes()
    }
    #[getter]
    fn padded_bytes(&self) -> u32 {
        self.inner.padded_bytes()
    }
}

#[pymethods]
impl CommandSequence {
    #[staticmethod]
    fn verified_writes(
        address: u32,
        data: Vec<u8>,
        limits: FirmwareLimits,
        packet_bytes: u32,
    ) -> PyResult<Vec<Self>> {
        usb::operations::verified::verified_writes(
            Address(address),
            &data,
            limits.memory,
            nz(packet_bytes)?,
        )
        .map(|batches| {
            batches
                .into_iter()
                .map(|commands| Self { commands })
                .collect()
        })
        .map_err(error)
    }
}

#[pymethods]
impl AsyncSession {
    async fn invoke_queries(
        &self,
        plan: InstallQueries,
    ) -> PyResult<Vec<(Py<PyBytes>, Py<PyBytes>)>> {
        let limits = self.limits.memory;
        self.with(move |s| Box::pin(async move { plan.inner.invoke(s, limits).await }))
            .await
            .map(|replies| {
                replies
                    .into_iter()
                    .map(|(reply, output)| (bytes(reply), bytes(output)))
                    .collect()
            })
    }
    async fn upload_owned(
        &self,
        image: OwnedUpload,
        pointer: u32,
        original: u32,
        capacity_address: u32,
        capacity: u32,
    ) -> PyResult<data::UploadReply> {
        let destination = install::UploadDestination::new(
            Address(pointer),
            Address(original),
            Address(capacity_address),
            nz(capacity)?,
        )
        .map_err(error)?;
        let limits = self.limits.memory;
        self.with(move |s| {
            Box::pin(async move { image.inner.upload(s, destination, limits).await })
        })
        .await
        .map(|inner| data::UploadReply { inner })
    }
    async fn upload_owned_chunks(
        &self,
        image: OwnedUpload,
        pointer: u32,
        original: u32,
        capacity_address: u32,
        capacity: u32,
    ) -> PyResult<()> {
        let destination = install::UploadDestination::new(
            Address(pointer),
            Address(original),
            Address(capacity_address),
            nz(capacity)?,
        )
        .map_err(error)?;
        let limits = self.limits.memory;
        self.with(move |s| {
            Box::pin(async move { image.inner.upload_chunks(s, destination, limits).await })
        })
        .await
    }
    async fn verify_owned(&self, image: OwnedUpload, window: data::DataWindow) -> PyResult<()> {
        self.with(move |s| Box::pin(async move { image.inner.verify(s, window.inner).await }))
            .await
    }
}

pub(super) fn register(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<InstallQueries>()?;
    module.add_class::<OwnedUpload>()
}
