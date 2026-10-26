use super::*;
use usb::ff80::{
    commands::{
        diagnostic::DiagnosticTag,
        memory::{Address, MemoryRange},
    },
    sequence::SequenceCommand,
};

#[wasm_bindgen]
#[derive(Default)]
pub struct CommandSequence {
    pub(crate) commands: Vec<SequenceCommand>,
}
#[wasm_bindgen]
impl CommandSequence {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self::default()
    }
    pub fn read(
        &mut self,
        #[wasm_bindgen(unchecked_param_type = "number")] address: JsValue,
        #[wasm_bindgen(unchecked_param_type = "number")] length: JsValue,
        expected: Option<js_sys::Uint8Array>,
    ) -> Result<usize, JsValue> {
        let range = MemoryRange::new(
            Address(error::integer(address, u32::MAX)?),
            error::nz(error::integer(length, u32::MAX)?)?,
        )
        .map_err(error)?;
        let index = self.commands.len();
        self.commands.push(SequenceCommand::Read {
            range,
            expected: expected.map(|b| b.to_vec()),
        });
        Ok(index)
    }
    pub fn write(
        &mut self,
        #[wasm_bindgen(unchecked_param_type = "number")] address: JsValue,
        #[wasm_bindgen(unchecked_param_type = "Uint8Array")] data: JsValue,
    ) -> Result<usize, JsValue> {
        let address = Address(error::integer(address, u32::MAX)?);
        let bytes = error::buffer(data)?;
        let index = self.commands.len();
        self.commands
            .push(SequenceCommand::Write { address, bytes });
        Ok(index)
    }
    pub fn query(
        &mut self,
        #[wasm_bindgen(unchecked_param_type = "number")] tag: JsValue,
        #[wasm_bindgen(unchecked_param_type = "ReadonlyArray<readonly [number, number]>")] statuses: JsValue,
    ) -> Result<usize, JsValue> {
        let tag = DiagnosticTag::Research(error::integer(tag, u32::MAX)?);
        let statuses = parse_statuses(statuses)?;
        let index = self.commands.len();
        self.commands.push(SequenceCommand::Query { tag, statuses });
        Ok(index)
    }
}

pub(crate) fn parse_statuses(statuses: JsValue) -> Result<Vec<(u32, u32)>, JsValue> {
    if !js_sys::Array::is_array(&statuses) {
        return Err(error(usb::Error::InvalidLength));
    }
    js_sys::Array::from(&statuses)
        .iter()
        .map(|pair| {
            if !js_sys::Array::is_array(&pair) {
                return Err(error(usb::Error::InvalidLength));
            }
            let pair = js_sys::Array::from(&pair);
            if pair.length() != 2 {
                return Err(error(usb::Error::InvalidLength));
            }
            Ok((
                error::integer(pair.get(0), u32::MAX)?,
                error::integer(pair.get(1), u32::MAX)?,
            ))
        })
        .collect::<Result<Vec<_>, JsValue>>()
}
