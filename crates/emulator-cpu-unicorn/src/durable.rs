//! Architectural CPU state in binary form. Opaque execution-library contexts,
//! host pointers, hooks and translation caches never enter this encoding.
use super::*;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

mod r5;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) enum Architecture {
    CortexR5f(r5::State),
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document<T> {
    schema: u32,
    architecture: T,
}
pub(super) struct Imported {
    owner: Identity,
    state: Architecture,
}
impl Snapshot for Imported {
    fn owner(&self) -> Identity {
        self.owner
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn export(&self) -> Result<Vec<u8>, String> {
        export(&self.state)
    }
}
impl Imported {
    pub(super) fn restore(&self, cpu: &Core) -> Result<(), String> {
        match &self.state {
            Architecture::CortexR5f(state) => state.install(cpu),
        }
    }
}
pub(super) fn capture(cpu: &Core, context: &Context) -> Result<Architecture, String> {
    match cpu.model {
        Model::CortexR5F => Ok(Architecture::CortexR5f(r5::State::capture(cpu, context)?)),
        model => Err(format!(
            "durable Unicorn architecture is not implemented for {model:?}"
        )),
    }
}
pub(super) fn export(state: &Architecture) -> Result<Vec<u8>, String> {
    encode(&Document {
        schema: 1,
        architecture: state,
    })
}
pub(super) fn import(cpu: &Core, bytes: &[u8]) -> Result<Box<dyn Snapshot>, String> {
    let document: Document<Architecture> = decode(bytes)?;
    if document.schema != 1 {
        return Err("unknown Unicorn architectural snapshot schema".into());
    }
    match &document.architecture {
        Architecture::CortexR5f(state) => state.validate(cpu)?,
    }
    Ok(Box::new(Imported {
        owner: cpu.identity,
        state: document.architecture,
    }))
}
pub(crate) fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    ciborium::into_writer(value, &mut bytes).map_err(|e| format!("CPU snapshot encode: {e}"))?;
    Ok(bytes)
}
pub(crate) fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, String> {
    let mut input = bytes;
    let value =
        ciborium::from_reader(&mut input).map_err(|e| format!("CPU snapshot decode: {e}"))?;
    if !input.is_empty() {
        return Err("CPU snapshot has trailing bytes".into());
    }
    Ok(value)
}

impl Context {
    fn get<const N: usize>(&self, register: Register) -> Result<[u8; N], String> {
        let mut bytes = [0; N];
        let mut size = N;
        check(&self.api, unsafe {
            (self.api.uc_context_reg_read2)(
                self.raw,
                register_id(register)?,
                bytes.as_mut_ptr().cast(),
                &mut size,
            )
        })?;
        if size != N {
            return Err("CPU snapshot register width differs".into());
        }
        Ok(bytes)
    }
    fn get32(&self, register: Register) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.get(register)?))
    }
    fn put32(&self, register: Register, value: u32) -> Result<(), String> {
        let mut size = 4;
        check(&self.api, unsafe {
            (self.api.uc_context_reg_write2)(
                self.raw,
                register_id(register)?,
                value.to_le_bytes().as_ptr().cast(),
                &mut size,
            )
        })
    }
}
fn array<T, const N: usize>(
    values: impl IntoIterator<Item = Result<T, String>>,
) -> Result<[T; N], String> {
    values
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?
        .try_into()
        .map_err(|_| "CPU snapshot array length differs".into())
}

pub(crate) mod bytes {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(value: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(value)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        Vec::<u8>::deserialize(deserializer)
    }
}
