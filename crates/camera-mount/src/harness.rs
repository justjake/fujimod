//! Direct access to the owned file helper for emulator and regression tests.
//! Production connections still use [`crate::usb::UsbDevice`] for discovery
//! and session ownership.
use crate::service::Service;
use fujimod_catalog_bulk::FirmwareBulk;
use fujimod_catalog_card::FirmwareCard;
use fujimod_usb::{
    Error, Result,
    ff80::{
        Session,
        commands::{info::InfoSelector, memory::DebugMemoryLimits},
    },
    operations::card::{Request, Response, Volume},
};

/// An owned helper allocation that can execute consecutive file requests over
/// one caller-owned ff80 session. The allocation intentionally survives caller
/// session changes, matching the idle lifetime of the mounted device.
pub struct MountHarness {
    service: Service,
    capability: FirmwareCard,
    mailbox_hint: Option<u32>,
}

impl MountHarness {
    pub async fn install(
        session: &mut Session,
        limits: DebugMemoryLimits,
        bulk: &FirmwareBulk,
        capability: &FirmwareCard,
    ) -> Result<Self> {
        capability.validate().map_err(Error::Card)?;
        let facts = capability.service.as_ref().ok_or_else(|| {
            Error::Card("The selected card capability has no owned file service".into())
        })?;
        if session
            .device_info(InfoSelector::Product)
            .await?
            .text
            .trim()
            != capability.product
            || session
                .device_info(InfoSelector::InternalVersion)
                .await?
                .text
                .trim()
                != capability.internal_version
        {
            return Err(Error::Card(
                "Connected camera differs from the selected firmware".into(),
            ));
        }
        let service = Service::install(session, limits, bulk, facts).await?;
        Ok(Self {
            capability: service.capability(capability),
            service,
            mailbox_hint: None,
        })
    }

    pub async fn execute(
        &mut self,
        session: &mut Session,
        limits: DebugMemoryLimits,
        request: Request,
    ) -> Result<Response> {
        self.service.check(session, limits, None).await?;
        let mut volume =
            Volume::attach(session, limits, &self.capability, None, self.mailbox_hint).await?;
        volume.set_query_trigger(self.service.trigger())?;
        self.mailbox_hint = volume.mailbox_address();
        let result = volume.execute(request).await;
        volume.close().await?;
        result
    }

    pub async fn unload(self, session: &mut Session, limits: DebugMemoryLimits) -> Result<()> {
        self.service.release(session, limits).await
    }
}
