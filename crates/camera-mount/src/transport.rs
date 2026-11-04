//! Choose a qualified E1 read only when its command count beats CPU reads.
//! Qualification and borrowed publication state live for one ff80 transaction.
use fujimod_catalog_bulk::{BulkRange, BulkSource, FirmwareBulk};
use fujimod_usb::{
    Error, Result,
    ff80::{Session, commands::memory::DebugMemoryLimits},
    operations::card::BufferIo,
};
use fujimod_workflows::{
    Limits,
    bulk::{BulkError, BulkMapping},
};
use std::num::NonZeroU32;

pub(crate) struct Transport<'a> {
    facts: Option<&'a FirmwareBulk>,
    limits: DebugMemoryLimits,
    cpu_bytes: u32,
    cpu_write: u32,
    bulk_bytes: u32,
    mapping: Option<BulkMapping>,
    upload: Option<&'a fujimod_catalog_bulk::Upload>,
    upload_checked: bool,
}
impl<'a> Transport<'a> {
    pub fn resume(&mut self, mapping: Option<BulkMapping>) {
        self.mapping = mapping;
    }
    pub fn suspend(&mut self) -> Option<BulkMapping> {
        self.mapping.take()
    }
    pub fn new(
        facts: Option<&'a FirmwareBulk>,
        limits: DebugMemoryLimits,
        session: &Session,
        upload: Option<&'a fujimod_catalog_bulk::Upload>,
    ) -> Self {
        Self {
            facts,
            limits,
            cpu_bytes: limits
                .read
                .get()
                .min(session.info().advertised_packet_bytes.get())
                .min(session.max_reply_bytes().get().saturating_sub(36))
                .max(1),
            cpu_write: limits
                .write
                .get()
                .min(session.info().advertised_packet_bytes.get()),
            bulk_bytes: session.max_reply_bytes().get(),
            mapping: None,
            upload,
            upload_checked: false,
        }
    }
    fn capacity(&self) -> u32 {
        let capacity = match self.facts.map(|f| &f.source) {
            Some(
                BulkSource::Query { capacities, .. } | BulkSource::DirectQuery { capacities, .. },
            ) => capacities
                .iter()
                .copied()
                .filter(|n| *n > 0)
                .min()
                .unwrap_or(1),
            Some(
                BulkSource::Pattern { capacity, .. } | BulkSource::PatternZeroBase { capacity, .. },
            ) => *capacity,
            None => 1,
        };
        capacity.min(self.bulk_bytes).max(1)
    }
    fn range(&self, address: u32, size: u32) -> Option<BulkRange> {
        let facts = self.facts?;
        if size == 0 {
            return None;
        }
        if matches!(facts.source, BulkSource::Query { base, wrapping: false, .. } if u64::from(address) < base)
        {
            return None;
        }
        let (align, granule) = facts.dma.map_or((1, 1), |d| {
            (u64::from(d.address_alignment), u64::from(d.length_granule))
        });
        if align == 0 || granule == 0 {
            return None;
        }
        let start = u64::from(address) / align * align;
        let bytes = (u64::from(address) - start + u64::from(size)).div_ceil(granule) * granule;
        let range = BulkRange {
            address: start,
            size: bytes,
        };
        (start.checked_add(bytes)? <= 1u64 << 32 && facts.window.contains(range)).then_some(range)
    }
    fn setup_commands(&self) -> u64 {
        if self.mapping.is_some() {
            return 0;
        }
        let Some(facts) = self.facts else {
            return u64::MAX;
        };
        let cpu = |n: u64| n.div_ceil(u64::from(self.cpu_bytes));
        let bulk = |n: u64| n.div_ceil(u64::from(self.capacity()));
        // Identity, source guards/publication, restoration, and per-probe reads.
        2 + match facts.source {
            BulkSource::Query { .. } => 10,
            _ => 7,
        } + facts.code_checks.iter().map(|c| cpu(c.size)).sum::<u64>()
            + facts
                .probes
                .iter()
                .map(|p| cpu(p.size) + bulk(p.size))
                .sum::<u64>()
            + facts.mapped_probe.as_ref().map_or(0, |p| bulk(p.size))
    }
    pub async fn restore(&mut self, session: &mut Session) -> Result<()> {
        if let Some(mapping) = self.mapping.take() {
            mapping.restore(session).await.map_err(error)?;
        }
        Ok(())
    }
}
impl BufferIo for Transport<'_> {
    fn admits_write(&self, address: u32, length: u32, capacity: u32) -> bool {
        let Some(upload) = self.upload else {
            return false;
        };
        let Some(piece) = NonZeroU32::new(upload.piece_bytes) else {
            return false;
        };
        if length == 0
            || !upload.address_alignment.is_power_of_two()
            || !address.is_multiple_of(upload.address_alignment)
            || !fujimod_workflows::install::padded_size(length, piece)
                .is_ok_and(|n| n.get() <= capacity)
        {
            return false;
        }
        let qualification = if self.upload_checked {
            0
        } else {
            upload
                .code_checks
                .iter()
                .map(|c| c.size.div_ceil(u64::from(self.cpu_bytes)))
                .sum::<u64>()
        };
        let commands = qualification
            + upload.configuration_checks.len() as u64
            + 9
            + 2 * u64::from(upload.pointer_bytes).div_ceil(u64::from(self.cpu_write))
            + u64::from(length).div_ceil(u64::from(upload.piece_bytes));
        u64::from(length).div_ceil(u64::from(self.cpu_write)) > commands
    }

    fn write_owned<'a>(
        &'a mut self,
        session: &'a mut Session,
        address: u32,
        data: &'a [u8],
        capacity: u32,
    ) -> fujimod_usb::emulator::EndpointFuture<'a, Result<bool>> {
        Box::pin(async move {
            use fujimod_usb::{
                ff80::{
                    SessionState,
                    commands::memory::{Address, MemoryRange},
                },
                operations::{checks, memory},
            };
            use fujimod_workflows::install::{OwnedUpload, UploadDestination, padded_size};
            let Some(upload) = self.upload else {
                return Ok(false);
            };
            let piece = NonZeroU32::new(upload.piece_bytes).ok_or(Error::InvalidLength)?;
            let length = u32::try_from(data.len()).map_err(|_| Error::InvalidLength)?;
            if !self.admits_write(address, length, capacity) {
                return Ok(false);
            }
            let range = |at, bytes| {
                MemoryRange::new(
                    Address(at),
                    NonZeroU32::new(bytes).ok_or(Error::InvalidLength)?,
                )
            };
            if !self.upload_checked {
                for check in &upload.code_checks {
                    let bytes = memory::read_range(
                        session,
                        range(
                            check
                                .address
                                .try_into()
                                .map_err(|_| Error::AddressOverflow)?,
                            check.size.try_into().map_err(|_| Error::InvalidLength)?,
                        )?,
                        self.limits,
                    )
                    .await?;
                    let hash: String = checks::sha256_bytes(&bytes)
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect();
                    if hash != check.sha256 {
                        return Err(Error::Card(format!(
                            "Stock upload code differs: {}",
                            check.name
                        )));
                    }
                }
                self.upload_checked = true;
            }
            for guard in &upload.configuration_checks {
                use fujimod_usb::ff80::commands::configuration::ConfigurationView;
                if fujimod_usb::operations::configuration::read_byte(
                    session,
                    ConfigurationView::Ram,
                    guard.offset,
                    NonZeroU32::new(256).unwrap(),
                )
                .await?
                    != guard.value
                {
                    return Ok(false);
                }
            }
            let original = memory::read_range(
                session,
                range(upload.pointer, upload.pointer_bytes)?,
                self.limits,
            )
            .await?;
            let raw_capacity =
                memory::read_range(session, range(upload.capacity_address, 4)?, self.limits)
                    .await?;
            let Some(available) = NonZeroU32::new(u32::from_le_bytes(
                raw_capacity.try_into().expect("four bytes"),
            )) else {
                return Ok(false);
            };
            // A small registered staging capacity cannot accelerate this owned buffer.
            if padded_size(length, piece)?.get() > available.get() {
                return Ok(false);
            }
            let image = OwnedUpload::new(
                range(address, capacity)?,
                Address(address),
                data.to_vec(),
                piece,
                NonZeroU32::new(upload.address_alignment).ok_or(Error::InvalidLength)?,
            )?;
            let destination = UploadDestination::new(
                Address(upload.pointer),
                {
                    let mut word = [0; 8];
                    word[..original.len()].copy_from_slice(&original);
                    u64::from_le_bytes(word)
                },
                upload.pointer_bytes,
                Address(upload.capacity_address),
                available,
            )?;
            let result = image.upload(session, destination, self.limits).await;
            if result.is_err() && session.state() == SessionState::Ready {
                let restored = async {
                    let current = memory::read_range(
                        session,
                        range(upload.pointer, upload.pointer_bytes)?,
                        self.limits,
                    )
                    .await?;
                    if current != original {
                        if current
                            != u64::from(address).to_le_bytes()[..upload.pointer_bytes as usize]
                        {
                            return Err(Error::Card("Upload destination changed owner".into()));
                        }
                        memory::write_range(
                            session,
                            Address(upload.pointer),
                            &original,
                            self.limits,
                        )
                        .await?;
                        if memory::read_range(
                            session,
                            range(upload.pointer, upload.pointer_bytes)?,
                            self.limits,
                        )
                        .await?
                            != original
                        {
                            return Err(Error::Card("Upload destination was not restored".into()));
                        }
                    }
                    Ok(())
                }
                .await;
                if let Err(error) = restored {
                    session.invalidate_completion();
                    return Err(error);
                }
            }
            result.map(|_| true)
        })
    }

    fn admits(&self, address: u32, size: u32) -> bool {
        self.range(address, size).is_some_and(|range| {
            self.setup_commands() + range.size.div_ceil(u64::from(self.capacity()))
                < u64::from(size).div_ceil(u64::from(self.cpu_bytes))
        })
    }
    fn read<'a>(
        &'a mut self,
        session: &'a mut Session,
        address: u32,
        size: u32,
    ) -> fujimod_usb::emulator::EndpointFuture<'a, Result<Vec<u8>>> {
        Box::pin(async move {
            if self.mapping.is_none() {
                self.mapping = Some(
                    BulkMapping::prepare_preserving(
                        session,
                        self.facts.ok_or(Error::OutsideDataWindow)?,
                        Limits {
                            memory: self.limits,
                            configuration: NonZeroU32::new(256).expect("constant"),
                        },
                    )
                    .await
                    .map_err(error)?,
                );
            }
            BufferIo::read(
                self.mapping.as_mut().expect("prepared"),
                session,
                address,
                size,
            )
            .await
        })
    }
}
fn error(error: BulkError) -> Error {
    match error {
        BulkError::Usb(error) => error,
        error => Error::Card(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_usb::{
        ff80::framing::RequestKind,
        testing::{Exchange as E, Script},
    };
    use futures_lite::future::block_on;
    #[test]
    fn all_bulk_families_are_selected_by_cost_and_dma_aperture_not_camera_names() {
        let mut control = [0; 12];
        control[0] = RequestKind::Open as u8;
        control[6..10].copy_from_slice(&8u32.to_be_bytes());
        let script = Script::new(vec![
            E::Control(control),
            E::Read {
                maximum: 8,
                data: vec![0x34, 0x12, 0, 0, 0, 0, 1, 0],
            },
        ]);
        let session = block_on(script.begin_session(65536, None)).unwrap();
        for source in [
            BulkSource::DirectQuery {
                tag: 128,
                query_base: 0x1000,
                base: 0x1000,
                data_offset: 0,
                capacities: vec![65536],
                wrapping: true,
            },
            BulkSource::Query {
                tag: 128,
                callback_table: 0x1000,
                callback_offset: 12,
                stock_callback: 0x2000,
                base: 0x1000,
                source_state: 0x1200,
                capacities: vec![65536],
                wrapping: true,
            },
            BulkSource::Pattern {
                registered_buffer: 0x1300,
                source_state: 0x1400,
                capacity: 65536,
            },
            BulkSource::PatternZeroBase {
                registered_buffer: 0x1300,
                source_state: 0x1400,
                configuration_pointer: 0x1500,
                configuration_bytes: 1024,
                system_mode_address: 0x1600,
                capacity: 65536,
            },
        ] {
            let facts = FirmwareBulk {
                upload: None,
                schema_uri: fujimod_catalog_bulk::BULK_SCHEMA_URI.into(),
                schema: 1,
                id: "body-1-bulk".into(),
                firmware_id: "body-1".into(),
                product: "Body".into(),
                internal_version: "1".into(),
                code_alias: 0,
                source,
                window: BulkRange {
                    address: 0x1000,
                    size: 1024 * 1024,
                },
                dma: Some(fujimod_catalog_bulk::BulkDma {
                    address_alignment: 32,
                    length_granule: 32,
                }),
                code_checks: vec![],
                probes: vec![],
                mapped_probe: None,
                scopes: vec![],
            };
            let limits = DebugMemoryLimits {
                read: NonZeroU32::new(256).unwrap(),
                write: NonZeroU32::new(256).unwrap(),
            };
            let reader = Transport::new(Some(&facts), limits, &session, None);
            assert!(
                !reader.admits(0x1800, 512),
                "CPU is cheaper for a small control read"
            );
            assert!(
                reader.admits(0x1801, 128 * 1024),
                "large unaligned RAM reads use padded E1"
            );
            assert!(
                !reader.admits(0xff0, 128 * 1024),
                "DMA never crosses the admitted aperture"
            );
        }
    }
}

#[cfg(test)]
mod upload_tests;
