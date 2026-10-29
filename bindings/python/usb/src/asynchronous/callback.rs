//! X-Turbo-inspired callback plans; all target facts come from the caller.
use super::*;
use usb::operations::callback as operation;

#[pyclass(name = "CallbackInvocation", from_py_object, module = "fujimod.usb")]
#[derive(Clone)]
pub(super) struct CallbackInvocation {
    inner: operation::Invocation,
}
#[pymethods]
impl CallbackInvocation {
    #[new]
    #[pyo3(signature=(slot_address,required,value,trigger,limits,*,replies=None))]
    fn new(
        slot_address: u32,
        required: Vec<u8>,
        value: Vec<u8>,
        trigger: research::Request,
        limits: FirmwareLimits,
        replies: Option<Vec<Vec<u8>>>,
    ) -> Self {
        Self {
            inner: operation::Invocation {
                preconditions: vec![],
                writes: vec![],
                synchronization: vec![],
                slot: operation::CallbackSlot {
                    address: Address(slot_address),
                    required,
                    value,
                },
                trigger: trigger.inner,
                expected: replies.map_or(
                    operation::ExpectedOutcome::NoReply,
                    operation::ExpectedOutcome::Reply,
                ),
                limits: limits.memory,
            },
        }
    }
    #[staticmethod]
    #[pyo3(signature=(slot_address,required,value,trigger,limits,body_prefix,*,statuses=None))]
    fn completed(
        slot_address: u32,
        required: Vec<u8>,
        value: Vec<u8>,
        trigger: research::Request,
        limits: FirmwareLimits,
        body_prefix: Vec<u8>,
        statuses: Option<Vec<(u32, u32)>>,
    ) -> Self {
        let mut plan = Self::new(slot_address, required, value, trigger, limits, None);
        plan.inner.expected = operation::ExpectedOutcome::Completed {
            body_prefix,
            statuses,
            length: None,
        };
        plan
    }
    fn require(&mut self, address: u32, length: u32, accepted: Vec<Vec<u8>>) -> PyResult<()> {
        let range = MemoryRange::new(Address(address), nz(length)?).map_err(error)?;
        self.inner
            .preconditions
            .push(operation::Precondition { range, accepted });
        Ok(())
    }
    fn write(&mut self, address: u32, accepted: Vec<Vec<u8>>, data: Vec<u8>) {
        self.inner.writes.push(operation::GuardedWrite {
            address: Address(address),
            accepted,
            bytes: data,
        });
    }
    fn synchronize(&mut self, commands: super::sequence::CommandSequence) {
        self.inner.synchronization = commands.commands;
    }
}
#[pymethods]
impl AsyncSession {
    async fn invoke_callback(
        &self,
        invocation: CallbackInvocation,
    ) -> PyResult<Option<Py<PyBytes>>> {
        self.with(move |s| Box::pin(async move { operation::invoke(s, &invocation.inner).await }))
            .await
            .map(|reply| reply.map(bytes))
    }
}
pub(super) fn register(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<CallbackInvocation>()
}
