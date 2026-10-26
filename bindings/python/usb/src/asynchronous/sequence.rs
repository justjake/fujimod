use super::*;
use usb::ff80::sequence::SequenceCommand;

#[pyclass(name = "CommandSequence", from_py_object, module = "fujimod.usb")]
#[derive(Clone, Default)]
pub(super) struct CommandSequence {
    pub(super) commands: Vec<SequenceCommand>,
}
#[pymethods]
impl CommandSequence {
    #[new]
    fn new() -> Self {
        Self::default()
    }
    #[pyo3(signature=(address,length,expected=None))]
    fn read(&mut self, address: u32, length: u32, expected: Option<Vec<u8>>) -> PyResult<usize> {
        let range = MemoryRange::new(Address(address), nz(length)?).map_err(error)?;
        let index = self.commands.len();
        self.commands
            .push(SequenceCommand::Read { range, expected });
        Ok(index)
    }
    fn write(&mut self, address: u32, data: Vec<u8>) -> usize {
        let index = self.commands.len();
        self.commands.push(SequenceCommand::Write {
            address: Address(address),
            bytes: data,
        });
        index
    }
    #[pyo3(signature=(tag,statuses=None))]
    fn query(&mut self, tag: u32, statuses: Option<Vec<(u32, u32)>>) -> usize {
        let index = self.commands.len();
        self.commands.push(SequenceCommand::Query {
            tag: diagnostic::DiagnosticTag::Research(tag),
            statuses: statuses.unwrap_or_else(|| vec![(0, 0)]),
        });
        index
    }
}
