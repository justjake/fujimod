//! Memory and configuration access for one workflow on one session.
use fujimod_catalog_workflows::ServiceSetup;
use fujimod_usb::{
    ff80::{
        Session,
        commands::{
            configuration::ConfigurationView,
            memory::{Address, DebugMemoryLimits, MemoryRange},
        },
    },
    operations::{configuration, memory::read_range},
};
use std::num::NonZeroU32;

/// Transfer bounds the caller qualified for this session.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub memory: DebugMemoryLimits,
    pub configuration: NonZeroU32,
}

impl Limits {
    pub(crate) fn for_setup(
        self,
        setup: &ServiceSetup,
    ) -> Result<Self, fujimod_catalog_workflows::WorkflowError> {
        let read = NonZeroU32::new(
            self.memory
                .read
                .get()
                .min(setup.configuration.memory_read_bytes),
        )
        .ok_or(fujimod_catalog_workflows::WorkflowError::Range)?;
        let configuration =
            NonZeroU32::new(self.configuration.get().min(setup.configuration.page_bytes))
                .ok_or(fujimod_catalog_workflows::WorkflowError::Range)?;
        Ok(Self {
            memory: DebugMemoryLimits {
                read,
                write: self.memory.write,
            },
            configuration,
        })
    }
}

pub(crate) fn range(address: u32, size: u32) -> fujimod_usb::Result<MemoryRange> {
    let size = NonZeroU32::new(size).ok_or(fujimod_usb::Error::InvalidLength)?;
    MemoryRange::new(Address(address), size)
}

pub(crate) struct Access<'a> {
    pub session: &'a mut Session,
    pub camera: &'a ServiceSetup,
    pub limits: Limits,
}
impl Access<'_> {
    /// Read at `address` exactly as given.
    pub async fn raw(&mut self, address: u32, size: u32) -> fujimod_usb::Result<Vec<u8>> {
        read_range(self.session, range(address, size)?, self.limits.memory).await
    }
    /// Read through the uncached alias of `address`.
    pub async fn uncached(&mut self, address: u32, size: u32) -> fujimod_usb::Result<Vec<u8>> {
        let alias = self.camera.ram_alias;
        self.raw(address | alias, size).await
    }
    pub async fn uncached_word(&mut self, address: u32) -> fujimod_usb::Result<u32> {
        let bytes = self.uncached(address, 4).await?;
        Ok(u32::from_le_bytes(bytes.try_into().expect("four bytes")))
    }
    pub async fn byte(&mut self, offset: u32) -> fujimod_usb::Result<u8> {
        configuration::read_byte(
            self.session,
            ConfigurationView::Ram,
            offset,
            self.limits.configuration,
        )
        .await
    }
    pub async fn set(&mut self, offset: u32, value: u8) -> fujimod_usb::Result<()> {
        configuration::write_byte_verified(
            self.session,
            ConfigurationView::Ram,
            offset,
            value,
            self.limits.configuration,
        )
        .await
    }
}
