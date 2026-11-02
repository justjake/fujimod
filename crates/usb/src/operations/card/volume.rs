//! A mount owns its reservation until explicit close. Preparation is amortized
//! across requests; the same exclusive session also serves read-only memory.
use super::*;
use crate::ff80::SessionState;

enum Backend<'a> {
    Sdk(sdk::Sdk<'a, 'a>),
    Mailbox(mailbox::Mailbox<'a>),
}

pub struct Volume<'a> {
    backend: Backend<'a>,
    usable: bool,
}

impl<'a> Volume<'a> {
    pub async fn attach(
        session: &'a mut Session,
        limits: DebugMemoryLimits,
        capability: &'a FirmwareCard,
        reader: Option<&'a mut dyn RangeRead>,
    ) -> Result<Self> {
        capability.validate().map_err(fail)?;
        require(
            session
                .device_info(InfoSelector::Product)
                .await?
                .text
                .trim()
                == capability.product
                && session
                    .device_info(InfoSelector::InternalVersion)
                    .await?
                    .text
                    == capability.internal_version,
            "Card capability identifies another firmware",
        )?;
        let mut host = Host {
            session,
            limits,
            reader,
        };
        for check in &capability.code_checks {
            let bytes = host
                .read(
                    check
                        .address
                        .try_into()
                        .map_err(|_| Error::AddressOverflow)?,
                    check.size.try_into().map_err(|_| Error::InvalidLength)?,
                )
                .await?;
            require(
                format!("{:x}", Sha256::digest(bytes)) == check.sha256,
                "Stock card instructions differ",
            )?;
        }
        let backend = match &capability.mechanism {
            Mechanism::StockSdk(p) => Backend::Sdk(sdk::prepare(host, p).await?),
            Mechanism::Mailbox {
                search_address,
                search_bytes,
                address_alias,
                build,
            } => Backend::Mailbox(
                mailbox::prepare(host, *search_address, *search_bytes, *address_alias, *build)
                    .await?,
            ),
        };
        Ok(Self {
            backend,
            usable: true,
        })
    }

    fn host(&mut self) -> &mut Host<'a> {
        match &mut self.backend {
            Backend::Sdk(s) => &mut s.host,
            Backend::Mailbox(s) => &mut s.host,
        }
    }

    pub async fn execute(&mut self, request: Request) -> Result<Response> {
        validate_request(&request)?;
        require(
            self.usable,
            "Card volume fenced after an uncertain operation",
        )?;
        let result = match &mut self.backend {
            Backend::Sdk(sdk) => {
                let result = sdk.transfer(request).await;
                if sdk.host.session.state() == SessionState::Ready && sdk.idle() {
                    if let Err(error) = if result.is_err() {
                        sdk.close().await
                    } else {
                        Ok(())
                    } {
                        self.usable = false;
                        return Err(error);
                    }
                } else {
                    self.usable = false;
                }
                result
            }
            Backend::Mailbox(service) => {
                let result = service.transfer(request).await;
                if service.host.session.state() == SessionState::Ready {
                    if let Err(error) = service.finish().await {
                        self.usable = false;
                        return Err(error);
                    }
                } else {
                    self.usable = false;
                }
                result
            }
        };
        result
    }

    /// Caller validates readable regions. This never uses writes or E1 setup.
    pub async fn read_memory(&mut self, address: u32, size: u32) -> Result<Vec<u8>> {
        require(
            self.usable,
            "Card volume fenced after an uncertain operation",
        )?;
        let host = self.host();
        host.read(address, size).await
    }

    /// Async cleanup must complete before closing the underlying USB session.
    /// An uncertain callback retains its camera allocation and rejects cleanup.
    pub async fn close(mut self) -> Result<()> {
        require(self.usable, "Card volume fenced; retain its reservation")?;
        match &mut self.backend {
            Backend::Sdk(sdk) => sdk.cleanup().await,
            Backend::Mailbox(service) => service.finish().await,
        }
    }

    pub async fn synchronize(&mut self) -> Result<()> {
        require(self.usable, "Card volume fenced; retain its reservation")?;
        match &mut self.backend {
            Backend::Sdk(sdk) => sdk.close().await,
            Backend::Mailbox(service) => service.finish().await,
        }
    }
}
