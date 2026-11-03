//! An owned helper allocation, independent of the lifetime of any USB session.
//! Every callback borrow restores the stock slot before returning.
use fujimod_catalog_bulk::{BulkSource, FirmwareBulk};
use fujimod_catalog_card::{FirmwareCard, Mechanism, QueryService};
use fujimod_usb::{
    Error, Result,
    ff80::{
        Session, SessionState,
        commands::{
            diagnostic::DiagnosticTag,
            memory::{Address, DebugMemoryLimits, MemoryRange},
        },
    },
    operations::{borrowed::BorrowedQueries, card::QueryTrigger, memory},
};
use std::num::NonZeroU32;

const CALL: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/call.bin"));
const IMAGE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/card.bin"));
const SHARED: u32 = 0x2000;
const BUFFER: u32 = 0x3000;
const CAPACITY: u32 = 8 << 20;
const ALIAS: u32 = 0x40000000;
fn require(ok: bool, message: &str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(Error::Card(message.into()))
    }
}
fn words(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|n| n.to_le_bytes()).collect()
}
fn word(raw: &[u8]) -> u32 {
    u32::from_le_bytes(raw[..4].try_into().unwrap())
}
async fn read(
    session: &mut Session,
    limits: DebugMemoryLimits,
    address: u32,
    size: u32,
) -> Result<Vec<u8>> {
    memory::read_range(
        session,
        MemoryRange::new(
            Address(address),
            NonZeroU32::new(size).ok_or(Error::InvalidLength)?,
        )?,
        limits,
    )
    .await
}
async fn write(
    session: &mut Session,
    limits: DebugMemoryLimits,
    address: u32,
    raw: &[u8],
) -> Result<()> {
    memory::write_range(session, Address(address), raw, limits).await?;
    require(
        read(session, limits, address, raw.len() as u32).await? == raw,
        "Owned service write readback differs",
    )
}
#[derive(Clone)]
struct Query {
    slot: u32,
    stock: u32,
    outputs: u32,
}
impl Query {
    async fn call(
        &self,
        session: &mut Session,
        limits: DebugMemoryLimits,
        entry: u32,
        argument: u32,
        output: [u32; 2],
        void: bool,
    ) -> Result<Vec<u8>> {
        let saved = read(session, limits, self.outputs, 8).await?;
        let mut call = BorrowedQueries::new(
            Address(self.slot),
            Address(self.stock),
            Address(self.outputs),
            saved,
        )?;
        let statuses = if void {
            vec![(0, 0), (1, 0xffff)]
        } else {
            vec![(0, 0)]
        };
        call.call(
            Address(entry),
            DiagnosticTag::Research(argument),
            statuses,
            words(&output),
        )?;
        Ok(call.invoke(session, limits).await?.remove(0).1)
    }
    async fn sync(
        &self,
        session: &mut Session,
        limits: DebugMemoryLimits,
        facts: &QueryService,
    ) -> Result<()> {
        self.call(session, limits, facts.cache_clean, 0, [0, 0], true)
            .await?;
        self.call(
            session,
            limits,
            facts.barrier,
            self.outputs,
            [self.outputs + 4, 0],
            true,
        )
        .await?;
        self.call(
            session,
            limits,
            facts.instruction_invalidate,
            0,
            [0, 0],
            false,
        )
        .await?;
        self.call(
            session,
            limits,
            facts.barrier,
            self.outputs,
            [self.outputs + 4, 0],
            true,
        )
        .await?;
        self.call(session, limits, facts.instruction_barrier, 0, [0, 0], false)
            .await?;
        Ok(())
    }
}

pub(crate) struct Service {
    raw: u32,
    pub base: u32,
    bytes: u32,
    query: Query,
    facts: QueryService,
}
impl Service {
    pub fn trigger(&self) -> QueryTrigger {
        QueryTrigger {
            slot: self.query.slot,
            stock: self.query.stock,
            outputs: self.query.outputs,
            entry: self.base,
            argument: (self.base + SHARED) | ALIAS,
        }
    }
    pub fn capability(&self, original: &FirmwareCard) -> FirmwareCard {
        let mut capability = original.clone();
        capability.mechanism = Mechanism::Mailbox {
            search_address: self.base,
            search_bytes: self.bytes - 32,
            address_alias: ALIAS,
            build: Some(1),
        };
        capability.service = None;
        capability
    }
    async fn check_image(&self, session: &mut Session, limits: DebugMemoryLimits) -> Result<()> {
        // The final word is the helper's private uncached active-request cell.
        require(
            read(session, limits, self.base | ALIAS, (IMAGE.len() - 4) as u32).await?
                == IMAGE[..IMAGE.len() - 4],
            "Owned file-service code changed",
        )
    }
    pub async fn check(
        &self,
        session: &mut Session,
        limits: DebugMemoryLimits,
        io: Option<&mut dyn fujimod_usb::operations::card::BufferIo>,
    ) -> Result<()> {
        use fujimod_usb::operations::checks::{Sha256Check, sha256_bytes, verify_sha256};
        let mut checks = self
            .facts
            .checks
            .iter()
            .map(|c| {
                Sha256Check::from_hex(
                    (c.address as u32) | ALIAS,
                    c.size.try_into().map_err(|_| Error::InvalidLength)?,
                    &c.sha256,
                )
            })
            .collect::<Result<Vec<_>>>()?;
        checks.push(Sha256Check {
            range: MemoryRange::new(
                Address(self.base | ALIAS),
                NonZeroU32::new((IMAGE.len() - 4) as u32).unwrap(),
            )?,
            expected: sha256_bytes(&IMAGE[..IMAGE.len() - 4]),
        });
        verify_sha256(session, limits, &checks, io).await
    }
    pub async fn install(
        session: &mut Session,
        limits: DebugMemoryLimits,
        bulk: &FirmwareBulk,
        facts: &QueryService,
    ) -> Result<Self> {
        require(
            CALL.len() <= 32 && IMAGE.len() < SHARED as usize && IMAGE.len() > 4,
            "Invalid bundled service image",
        )?;
        let BulkSource::Query {
            callback_table,
            callback_offset,
            stock_callback,
            source_state,
            ..
        } = bulk.source
        else {
            return Err(Error::Card(
                "Owned file service requires the selected query mechanism".into(),
            ));
        };
        require(
            bulk.code_alias == u64::from(ALIAS),
            "Owned service alias differs",
        )?;
        for check in &facts.checks {
            let raw = read(
                session,
                limits,
                (check.address as u32) | ALIAS,
                check.size.try_into().map_err(|_| Error::InvalidLength)?,
            )
            .await?;
            require(
                format!("{:x}", sha2::Sha256::digest(&raw)) == check.sha256,
                "Owned service stock code differs",
            )?;
        }
        let table = word(&read(session, limits, callback_table as u32, 4).await?) & !ALIAS;
        let query = Query {
            slot: table + callback_offset as u32,
            stock: stock_callback as u32,
            outputs: source_state as u32 & !ALIAS,
        };
        require(
            word(&read(session, limits, query.slot, 4).await?) == query.stock,
            "Query callback is occupied",
        )?;
        let configuration =
            word(&read(session, limits, facts.configuration_pointer, 4).await?) & !ALIAS;
        let slots = read(session, limits, facts.worker_slots, 100).await?;
        require(
            slots.chunks_exact(20).any(|row| {
                word(&row[4..]) == 1 && word(row) != 0 && row[8..].iter().all(|b| *b == 0)
            }),
            "CPU1 synchronization worker is unavailable or busy",
        )?;
        require(
            bulk.window.address <= u64::from(configuration | ALIAS)
                && u64::from(configuration | ALIAS) + 256 <= bulk.window.address + bulk.window.size,
            "Configuration scratch outside DRAM",
        )?;
        let bootstrap = configuration + 0x80;
        let original = read(session, limits, bootstrap, 32).await?;
        require(
            read(session, limits, bootstrap | ALIAS, 32).await? == original,
            "Bootstrap aliases disagree",
        )?;
        let mut stub = CALL.to_vec();
        stub.resize(32, 0);
        let mut raw = None;
        let result = async {
            write(session, limits, bootstrap, &stub).await?;
            query.sync(session, limits, facts).await?;
            require(
                read(session, limits, bootstrap | ALIAS, 32).await? == stub,
                "Bootstrap synchronization differs",
            )?;
            let mut capacity = CAPACITY;
            let allocated = loop {
                let allocated = word(
                    &query
                        .call(
                            session,
                            limits,
                            bootstrap,
                            BUFFER + capacity + 32,
                            [0, facts.allocate],
                            false,
                        )
                        .await?,
                );
                if allocated != 0 {
                    break allocated;
                }
                require(capacity > 65536, "Owned service allocation failed")?;
                capacity /= 2;
            };
            let bytes = BUFFER + capacity + 32;
            require(
                allocated != 0
                    && allocated < ALIAS
                    && allocated % 8 == 0
                    && u64::from(allocated | ALIAS) >= bulk.window.address
                    && u64::from(allocated | ALIAS) + u64::from(bytes)
                        <= bulk.window.address + bulk.window.size,
                "Owned service allocation failed or is outside DRAM",
            )?;
            raw = Some(allocated);
            for (at, size) in [
                (configuration, 256),
                (query.slot, 4),
                (query.outputs, 8),
                (facts.configuration_pointer, 4),
            ]
            .into_iter()
            .chain(
                facts
                    .checks
                    .iter()
                    .map(|c| (c.address as u32, c.size as u32)),
            ) {
                require(
                    !(u64::from(allocated) < u64::from(at) + u64::from(size)
                        && u64::from(at) < u64::from(allocated) + u64::from(bytes)),
                    "Allocation overlaps protected state",
                )?;
            }
            let base = (allocated + 31) & !31;
            write(session, limits, base, IMAGE).await?;
            let shared = (base + SHARED) | ALIAS;
            let mut data = vec![0u8; 512];
            let api = [
                0x314c4946,
                3,
                1,
                shared + 64,
                shared + 68,
                shared + 72,
                shared + 76,
                shared + 80,
                shared + 84,
                (base + BUFFER) | ALIAS,
                shared + 148,
                shared + 152,
                shared + 156,
                shared + 160,
                1, // Descriptor feature: one asynchronous open/seek/read/close.
                0,
            ];
            data[..64].copy_from_slice(&words(&api));
            data[64..68].copy_from_slice(&capacity.to_le_bytes());
            data[156..160].copy_from_slice(&u32::MAX.to_le_bytes());
            data[164..244].copy_from_slice(&words(&facts.functions));
            // Initialize through the cacheable view, then publish the complete
            // allocation before any callback uses its uncached shared view.
            write(session, limits, base + SHARED, &data).await?;
            query.sync(session, limits, facts).await?;
            require(
                read(session, limits, shared, data.len() as u32).await? == data,
                "Shared service initialization differs",
            )?;
            let service = Self {
                raw: allocated,
                base,
                bytes,
                query: query.clone(),
                facts: facts.clone(),
            };
            if std::env::var_os("FUJIMOD_MOUNT_TRACE").is_some() {
                eprintln!("{}", serde_json::json!({"service_capacity_bytes":capacity}));
            }
            service.check_image(session, limits).await?;
            write(session, limits, shared + 68, &13u32.to_le_bytes()).await?;
            query
                .call(session, limits, base, shared, [0, 0], false)
                .await?;
            let state = read(session, limits, shared + 68, 16).await?;
            require(
                word(&state) == 0 && word(&state[4..]) == 2 && word(&state[8..]) == 0,
                "Helper instruction synchronization did not complete",
            )?;
            Ok(service)
        }
        .await;
        let cleanup = async {
            require(
                session.state() == SessionState::Ready,
                "Setup completion unknown; helper allocation retained",
            )?;
            require(
                read(session, limits, bootstrap, 32).await? == stub,
                "Bootstrap ownership changed",
            )?;
            if result.is_err() {
                if let Some(raw) = raw {
                    query
                        .call(session, limits, bootstrap, raw, [0, facts.release], false)
                        .await?;
                }
            }
            write(session, limits, bootstrap, &original).await?;
            query.sync(session, limits, facts).await?;
            require(
                read(session, limits, bootstrap | ALIAS, 32).await? == original,
                "Bootstrap restoration differs",
            )
        }
        .await;
        if let Err(error) = cleanup {
            session.invalidate_completion();
            return Err(error);
        }
        result
    }
    pub async fn release(&self, session: &mut Session, limits: DebugMemoryLimits) -> Result<()> {
        self.check(session, limits, None).await?;
        let shared = (self.base + SHARED) | ALIAS;
        let state = read(session, limits, shared + 68, 16).await?;
        if word(&state) != 0
            || word(&state[4..]) == 1
            || word(&read(session, limits, shared + 156, 4).await?) != u32::MAX
        {
            session.invalidate_completion();
            return Err(Error::CompletionUnknown);
        }
        let configuration =
            word(&read(session, limits, self.facts.configuration_pointer, 4).await?) & !ALIAS;
        let bootstrap = configuration + 0x80;
        let original = read(session, limits, bootstrap, 32).await?;
        require(
            read(session, limits, bootstrap | ALIAS, 32).await? == original,
            "Bootstrap aliases disagree",
        )?;
        let mut stub = CALL.to_vec();
        stub.resize(32, 0);
        let result = async {
            write(session, limits, bootstrap, &stub).await?;
            self.query.sync(session, limits, &self.facts).await?;
            self.query
                .call(
                    session,
                    limits,
                    bootstrap,
                    self.raw,
                    [0, self.facts.release],
                    false,
                )
                .await?;
            Ok(())
        }
        .await;
        let cleanup = async {
            require(
                session.state() == SessionState::Ready,
                "Release completion unknown",
            )?;
            require(
                read(session, limits, bootstrap, 32).await? == stub,
                "Bootstrap ownership changed",
            )?;
            write(session, limits, bootstrap, &original).await?;
            self.query.sync(session, limits, &self.facts).await
        }
        .await;
        if let Err(error) = cleanup {
            session.invalidate_completion();
            return Err(error);
        }
        result
    }
}
use sha2::Digest;
#[cfg(test)]
mod tests;
