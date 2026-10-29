use crate::{
    Error, Result,
    relocation::{self, Destination, Kind, Relocation},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HookWord {
    pub address: u64,
    pub old: u32,
    pub new: u32,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Operations {
    pub install: u32,
    pub rollback: u32,
    pub status: u32,
    pub init: u32,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Mailbox {
    pub offset: u32,
    pub size: u32,
    pub nonce: u32,
    pub operation: u32,
    pub status: u32,
    pub result: u32,
    pub words_new: u32,
    pub words_old: u32,
    pub core: u32,
    pub token: u32,
    pub inits_done: Option<u32>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ControlLayout {
    pub control_offset: u32,
    pub state_offset: u32,
    pub magic_offset: u32,
    pub nonce_offset: u32,
    pub token_offset: u32,
    /// Optional host-written request cell for callbacks with a fixed argument ABI.
    pub operation_offset: Option<u32>,
    pub inits_done_offset: u32,
    pub installed_offset: u32,
    pub magic: u32,
    pub nonce: u32,
    pub core: u32,
    pub init_count: u32,
    pub operations: Operations,
    pub mailbox: Mailbox,
    /// Aliases are explicit caller facts, never inferred from an address.
    pub read_alias: u64,
    /// Immutable prefix checked before every control operation.
    pub code_size: u32,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HookTable {
    pub offset: u32,
    /// Address width in bytes; followed by old and new u32 values.
    pub address_bytes: u32,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeImage {
    pub version: u32,
    #[serde(skip)]
    pub bytes: Vec<u8>,
    pub link_base: u64,
    pub alignment: u64,
    pub capacity: u64,
    pub hooks: Vec<HookWord>,
    pub control: ControlLayout,
    pub relocations: Vec<Relocation>,
    pub hook_table: Option<HookTable>,
}
impl RuntimeImage {
    pub fn validate(&self) -> Result<()> {
        let bad = |s| Error::Invalid(s);
        if self.version != 1
            || self.bytes.is_empty()
            || self.capacity < self.bytes.len() as u64
            || !self.alignment.is_power_of_two()
            || self.link_base % self.alignment != 0
            || self.link_base.checked_add(self.capacity).is_none()
        {
            return Err(bad("invalid image geometry"));
        }
        let c = &self.control;
        if c.control_offset as usize >= self.bytes.len()
            || c.control_offset % 4 != 0
            || c.state_offset % 4 != 0
            || c.code_size == 0
            || c.code_size as usize > self.bytes.len()
            || c.state_offset < c.code_size
            || c.nonce == 0
            || c.mailbox.size % 4 != 0
        {
            return Err(bad("invalid control layout"));
        }
        for (offset, size) in [
            (c.magic_offset, 4),
            (c.nonce_offset, 4),
            (c.token_offset, 4),
            (c.inits_done_offset, 4),
            (c.installed_offset, 4),
            (c.mailbox.offset, c.mailbox.size),
        ] {
            if u64::from(c.state_offset) + u64::from(offset) + u64::from(size)
                > self.bytes.len() as u64
                || offset % 4 != 0
            {
                return Err(bad("state field outside image"));
            }
        }
        let mut fields = Vec::new();
        for (offset, size) in [
            (c.magic_offset, 4),
            (c.nonce_offset, 4),
            (c.token_offset, 4),
            (c.inits_done_offset, 4),
            (c.installed_offset, 4),
            (c.mailbox.offset, c.mailbox.size),
        ] {
            let end = u64::from(offset) + u64::from(size);
            if fields
                .iter()
                .any(|&(start, stop)| u64::from(offset) < stop && start < end)
            {
                return Err(bad("overlapping state fields"));
            }
            fields.push((u64::from(offset), end));
        }
        if let Some(offset) = c.operation_offset {
            let end = u64::from(offset) + 4;
            if offset % 4 != 0
                || u64::from(c.state_offset) + end > self.bytes.len() as u64
                || fields
                    .iter()
                    .any(|&(start, stop)| u64::from(offset) < stop && start < end)
            {
                return Err(bad("invalid operation request cell"));
            }
        }
        let mut operations = BTreeSet::new();
        for op in [
            c.operations.install,
            c.operations.rollback,
            c.operations.status,
            c.operations.init,
        ] {
            if !operations.insert(op) {
                return Err(bad("duplicate control operation"));
            }
        }
        let m = &c.mailbox;
        let mut mailbox_fields = BTreeSet::new();
        for offset in [
            Some(m.nonce),
            Some(m.operation),
            Some(m.status),
            Some(m.result),
            Some(m.words_new),
            Some(m.words_old),
            Some(m.core),
            Some(m.token),
            m.inits_done,
        ]
        .into_iter()
        .flatten()
        {
            if !mailbox_fields.insert(offset)
                || offset % 4 != 0
                || offset.checked_add(4).is_none_or(|end| end > m.size)
            {
                return Err(bad("mailbox field outside mailbox"));
            }
        }
        let mut sites = BTreeSet::new();
        for hook in &self.hooks {
            if hook.address % 4 != 0
                || hook.address.checked_add(4).is_none()
                || !sites.insert(hook.address)
            {
                return Err(bad("invalid or duplicate hook address"));
            }
        }
        if let Some(table) = &self.hook_table {
            if ![4, 8].contains(&table.address_bytes)
                || table.offset % table.address_bytes != 0
                || u64::from(table.offset)
                    + self.hooks.len() as u64 * u64::from(table.address_bytes + 8)
                    > self.bytes.len() as u64
            {
                return Err(bad("hook table outside image"));
            }
        }
        let mut destinations = BTreeSet::new();
        for r in &self.relocations {
            let width = if r.kind == Kind::Aarch64Abs64 { 8 } else { 4 };
            for destination in std::iter::once(r.destination).chain(r.pair) {
                self.get(destination, width)?;
                // Track individual bytes, rejecting partial overlaps too.
                for i in 0..width {
                    let byte = match destination {
                        Destination::Image(n) => (0, n as u64 + i as u64),
                        Destination::Hook(n) => (1, n as u64 * 4 + i as u64),
                    };
                    if !destinations.insert(byte) {
                        return Err(bad("duplicate relocation destination"));
                    }
                }
            }
            if (r.kind == Kind::ArmMov32) != r.pair.is_some() {
                return Err(bad("invalid relocation pair"));
            }
        }
        Ok(())
    }
    fn get(&self, destination: Destination, width: usize) -> Result<u64> {
        match destination {
            Destination::Image(offset) => {
                let offset = offset as usize;
                if offset % 4 != 0 {
                    return Err(Error::Invalid("unaligned relocation destination"));
                }
                let bytes = self
                    .bytes
                    .get(offset..offset + width)
                    .ok_or(Error::Invalid("relocation outside image"))?;
                let mut word = [0; 8];
                word[..width].copy_from_slice(bytes);
                Ok(u64::from_le_bytes(word))
            }
            Destination::Hook(index) => {
                let high = if width == 8 {
                    let low = self
                        .hooks
                        .get(index as usize)
                        .ok_or(Error::Invalid("relocation outside hooks"))?;
                    let high = self
                        .hooks
                        .get(index as usize + 1)
                        .ok_or(Error::Invalid("relocation outside hooks"))?;
                    if low.address % 8 != 0 || high.address != low.address + 4 {
                        return Err(Error::Invalid("noncontiguous 64-bit hook"));
                    }
                    u64::from(high.new) << 32
                } else {
                    0
                };
                Ok(high
                    | u64::from(
                        self.hooks
                            .get(index as usize)
                            .ok_or(Error::Invalid("relocation outside hooks"))?
                            .new,
                    ))
            }
        }
    }
    fn put(&mut self, destination: Destination, width: usize, value: u64) {
        match destination {
            Destination::Image(offset) => self.bytes[offset as usize..offset as usize + width]
                .copy_from_slice(&value.to_le_bytes()[..width]),
            Destination::Hook(index) => {
                self.hooks[index as usize].new = value as u32;
                if width == 8 {
                    self.hooks[index as usize + 1].new = (value >> 32) as u32;
                }
            }
        }
    }
    pub fn relocate(&self, base: u64) -> Result<Self> {
        self.validate()?;
        if base % self.alignment != 0 || base.checked_add(self.capacity).is_none() {
            return Err(Error::Invalid("invalid placement base"));
        }
        let delta = i128::from(base) - i128::from(self.link_base);
        let mut result = self.clone();
        for r in &self.relocations {
            let width = if r.kind == Kind::Aarch64Abs64 { 8 } else { 4 };
            let word = self.get(r.destination, width)?;
            let place = match r.destination {
                Destination::Image(offset) => base + u64::from(offset),
                Destination::Hook(index) => self.hooks[index as usize].address,
            };
            if let Some(pair) = r.pair {
                let (low, high) = relocation::mov_pair(
                    word as u32,
                    self.get(pair, 4)? as u32,
                    if r.moves { delta } else { 0 },
                )?;
                result.put(r.destination, 4, u64::from(low));
                result.put(pair, 4, u64::from(high));
            } else {
                let value = if r.moves {
                    relocation::moved(r.value, delta)?
                } else {
                    r.value
                };
                result.put(
                    r.destination,
                    width,
                    relocation::encode(r.kind, word, value, place)?,
                );
            }
        }
        if let Some(table) = &self.hook_table {
            for (index, hook) in result.hooks.iter().enumerate() {
                if table.address_bytes == 4 && hook.address > u64::from(u32::MAX) {
                    return Err(Error::Invalid("hook table address overflow"));
                }
                let start = table.offset as usize + index * (table.address_bytes as usize + 8);
                let end = start + table.address_bytes as usize;
                result.bytes[start..end]
                    .copy_from_slice(&hook.address.to_le_bytes()[..table.address_bytes as usize]);
                result.bytes[end..end + 4].copy_from_slice(&hook.old.to_le_bytes());
                result.bytes[end + 4..end + 8].copy_from_slice(&hook.new.to_le_bytes());
            }
        }
        // Keep records relative to this placement so subsequent moves are valid.
        for r in &mut result.relocations {
            if r.moves && r.kind != Kind::ArmMov32 {
                r.value = relocation::moved(r.value, delta)?;
            }
        }
        result.link_base = base;
        result.validate()?;
        Ok(result)
    }
}
