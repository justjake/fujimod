//! Camera restart over debug USB through stock code, from a firmware's catalog
//! restart facts.
//!
//! The camera must report the selected product and internal version, and
//! every stock routine on the path must match before anything is written. The
//! stock power-off sender is then called once with the power-off request and a
//! wake deadline, either through a borrowed diagnostic query or through the
//! debug console's command callback and a short stub. The stock power-off arms
//! the power controller's alarm and cuts power; the alarm boots the camera
//! again. The host observes the camera return.
use crate::{
    Limits,
    bulk::{Access, BulkError, Result, check_camera, hex},
};
use fujimod_catalog_restart::{DeferredTimer, FirmwareRestart, RecIdle, RequestTransport, WordValue};
use fujimod_usb::{
    ff80::{
        Session, SessionState,
        commands::{
            diagnostic::DiagnosticTag,
            memory::{Address, MemoryRange},
            research::{CommandCode, Request},
        },
    },
    operations::{
        borrowed::BorrowedQueries,
        callback::{self, CallbackSlot, ExpectedOutcome, GuardedWrite, Invocation, Precondition},
        checks,
    },
};
use std::num::NonZeroU32;

/// A deadline at or before the camera's clock arms the stock minimum, one second.
pub const WAKE_DEADLINE: u32 = 1;

/// Debug console command that runs the command callback.
const CONSOLE_COMMAND: CommandCode = CommandCode(0x200001);
/// Console operation word, echoed at the start of the reply body.
const CONSOLE_OPERATION: u32 = 5;
/// Byte offset of the parameters in the stub.
const STUB_PARAMETERS: usize = 16;

/// How the request left the camera.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Requested {
    /// The call returned; the power-off runs after it.
    Accepted,
    /// The session ended during the call: the camera cut power before
    /// answering. The session cannot issue further commands.
    SessionEnded,
}

fn require(condition: bool, message: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(BulkError::Check(message.into()))
    }
}

fn word(bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .rev()
        .fold(0, |value, byte| (value << 8) | u64::from(*byte))
}

fn address(value: u64) -> Result<Address> {
    u32::try_from(value)
        .map(Address)
        .map_err(|_| BulkError::Check(format!("{value:#x} is beyond 32-bit addressing")))
}

/// Request parameters: the power-off request and the wake deadline.
pub fn parameters(restart: &FirmwareRestart) -> Vec<u8> {
    let m = &restart.mechanism;
    let mut parameters = vec![0; m.parameter_bytes as usize];
    for (offset, value) in [
        (m.request_offset, m.request),
        (m.deadline_offset, WAKE_DEADLINE),
    ] {
        let at = offset as usize;
        parameters[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    parameters
}

/// Bytes for `queue_auto_restart_contract` in the portable ARM32 guard.
///
/// The guard carries no firmware-specific imports: Rust writes this contract
/// only after the selected restart profile and its callback-timer ABI have
/// validated.  The final word is the timer handle, initially zero.
pub fn deferred_guard_contract(restart: &FirmwareRestart, milliseconds: u32) -> Result<Vec<u8>> {
    restart.validate().map_err(BulkError::Check)?;
    let timer = restart
        .mechanism
        .deferred_timer
        .as_ref()
        .ok_or_else(|| BulkError::Check("No deferred callback timer is published".into()))?;
    validate_deferred_timer(timer)?;
    require(milliseconds != 0, "Deferred restart delay must be nonzero")?;
    let mut request = parameters(restart);
    request.resize(12, 0);
    let mut contract = Vec::with_capacity(28);
    for word in [timer.start.address, timer.cancel.address, restart.mechanism.sender] {
        contract.extend_from_slice(&address(word)?.0.to_le_bytes());
    }
    contract.extend_from_slice(&request);
    contract.extend_from_slice(&0u32.to_le_bytes());
    Ok(contract)
}

fn validate_deferred_timer(timer: &DeferredTimer) -> Result<()> {
    require(
        timer.kind == "software-timer-callback-v1"
            && timer.period_unit == "milliseconds"
            && timer.one_shot_periodic == 0
            && timer.callback_arguments == 0,
        "Unsupported deferred callback timer ABI",
    )
}

/// AArch64 stub at `at`: `adr x0, parameters; b sender`, two `brk #0`, then
/// the parameters. The sender returns straight to the console.
pub fn console_stub(at: u64, sender: u64, parameters: &[u8]) -> Result<Vec<u8>> {
    let displacement = sender as i64 - (at as i64 + 4);
    require(
        displacement % 4 == 0 && (-(1 << 27)..(1 << 27)).contains(&displacement),
        "Sender is beyond a direct branch from the stub area",
    )?;
    let adr = 0x1000_0000u32 | (((STUB_PARAMETERS as u32) >> 2) << 5);
    let branch = 0x1400_0000u32 | ((displacement >> 2) as u32 & 0x03ff_ffff);
    let brk = 0xd420_0000u32;
    let mut stub: Vec<u8> = [adr, branch, brk, brk]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    stub.extend_from_slice(parameters);
    Ok(stub)
}

async fn require_idle(access: &mut Access<'_>, ui: &RecIdle, alias: u64) -> Result<()> {
    let size = u64::from(ui.mode_size);
    let mode = word(&access.raw(ui.mode_address + alias, size).await?);
    let pending = match ui.pending_mode_address {
        Some(at) => word(&access.raw(at + alias, size).await?),
        None => 0,
    };
    let menu = match ui.menu_active_address {
        Some(at) => word(&access.raw(at + alias, 4).await?),
        None => 0,
    };
    require(
        mode == u64::from(ui.rec_mode) && pending == 0 && menu == 0,
        &format!(
            "Camera is not idle in REC (mode {mode}, pending {pending}, menu {menu}); \
             request a recovery restart instead"
        ),
    )
}

async fn require_words(access: &mut Access<'_>, words: &[WordValue], message: &str) -> Result<()> {
    for w in words {
        require(
            word(&access.raw(w.address, u64::from(w.size)).await?) == w.value,
            message,
        )?;
    }
    Ok(())
}

/// Request a restart. Without `recover`, the camera must be idle in REC.
/// Nothing is written unless identity, stock code and the stock callback match.
pub async fn request(
    session: &mut Session,
    restart: &FirmwareRestart,
    limits: Limits,
    recover: bool,
) -> Result<Requested> {
    restart.validate().map_err(BulkError::Check)?;
    check_camera(session, &restart.product, &restart.internal_version).await?;
    let alias = restart.code_alias;
    let m = &restart.mechanism;
    let mut access = Access { session, limits };
    for check in &restart.code_checks {
        let bytes = access.raw(check.address + alias, check.size).await?;
        require(
            hex(&checks::sha256_bytes(&bytes)) == check.sha256,
            &format!("Stock code differs: {}", check.name),
        )?;
    }
    match &m.transport {
        RequestTransport::BorrowedQuery {
            callback_table,
            callback_offset,
            stock_callback,
            output_fields,
            output_bytes,
        } => {
            // The table pointer may hold the cached or the uncached alias.
            let table = word(&access.raw(callback_table + alias, 4).await?);
            let physical = if table >= alias { table - alias } else { table };
            let slot = physical + callback_offset;
            require(
                word(&access.raw(slot + alias, 4).await?) == *stock_callback,
                "Require the stock diagnostic query callback",
            )?;
            if !recover {
                require_idle(&mut access, &m.ui, alias).await?;
            }
            let original = access.raw(*output_fields, u64::from(*output_bytes)).await?;
            let mut fields = parameters(restart);
            fields.resize(*output_bytes as usize, 0);
            let tag = u32::try_from(*output_fields).map_err(|_| {
                BulkError::Check("Query output fields beyond 32-bit addressing".into())
            })?;
            let mut borrowed = BorrowedQueries::new(
                address(slot)?,
                address(*stock_callback)?,
                address(*output_fields)?,
                original,
            )?;
            borrowed.call(
                address(m.sender)?,
                DiagnosticTag::Research(tag),
                vec![(0, 0)],
                fields,
            )?;
            match borrowed.invoke(access.session, limits.memory).await {
                Ok(_) => Ok(Requested::Accepted),
                // The whole sequence ran and restored both words; the send was refused.
                Err(error) if access.session.state() == SessionState::Ready => Err(error.into()),
                Err(_) => Ok(Requested::SessionEnded),
            }
        }
        RequestTransport::ConsoleCallback {
            slot,
            stock_callback,
            stub_area,
            stub_bytes,
            translation,
            ready_words,
            data_cache_clean,
            instruction_cache_sync,
        } => {
            require_words(
                &mut access,
                translation,
                "Stub area translation differs from its executable mapping",
            )
            .await?;
            require_words(
                &mut access,
                ready_words,
                "Stock cache maintenance is not ready on every core",
            )
            .await?;
            let stock = stock_callback.to_le_bytes().to_vec();
            require(
                access.raw(*slot, 8).await? == stock,
                "Require the stock console command callback",
            )?;
            if !recover {
                require_idle(&mut access, &m.ui, alias).await?;
            }
            let stub = console_stub(*stub_area, m.sender, &parameters(restart))?;
            require(
                stub.len() <= *stub_bytes as usize,
                "Stub exceeds the stub area",
            )?;
            let area = address(*stub_area)?;
            let empty = vec![0; *stub_bytes as usize];
            let present = access.raw(*stub_area, u64::from(*stub_bytes)).await?;
            let mut placed = stub.clone();
            placed.resize(*stub_bytes as usize, 0);
            require(present == empty || present == placed, "Stub area is in use")?;
            let trigger = {
                let text = b"CMD RESTART\n";
                let mut body: Vec<u8> = [CONSOLE_OPERATION, 0, 0, 0, text.len() as u32]
                    .into_iter()
                    .flat_map(u32::to_le_bytes)
                    .collect();
                body.extend(text);
                Request::new(CONSOLE_COMMAND, body, NonZeroU32::new(256).unwrap())?
            };
            let call = |entry: u64, success: bool| Invocation {
                preconditions: vec![],
                writes: vec![],
                synchronization: vec![],
                slot: CallbackSlot {
                    address: Address(*slot as u32),
                    required: stock.clone(),
                    value: entry.to_le_bytes().to_vec(),
                },
                trigger: trigger.clone(),
                expected: ExpectedOutcome::Completed {
                    body_prefix: CONSOLE_OPERATION.to_le_bytes().to_vec(),
                    statuses: success.then(|| vec![(0, 0)]),
                    length: None,
                },
                limits: limits.memory,
            };
            require(
                u32::try_from(*slot).is_ok(),
                "Console callback slot beyond 32-bit addressing",
            )?;
            // Write the stub, clean it out to memory on every core, then drop
            // stale instructions.
            let mut clean = call(*data_cache_clean, true);
            clean.writes.push(GuardedWrite {
                address: area,
                accepted: vec![vec![0; stub.len()], stub.clone()],
                bytes: stub.clone(),
            });
            callback::invoke(access.session, &clean).await?;
            callback::invoke(access.session, &call(*instruction_cache_sync, false)).await?;
            let mut run = call(*stub_area, true);
            run.preconditions.push(Precondition {
                range: MemoryRange::new(
                    area,
                    NonZeroU32::new(stub.len() as u32).expect("stub has instructions"),
                )?,
                accepted: vec![stub],
            });
            match callback::invoke(access.session, &run).await {
                Ok(_) => Ok(Requested::Accepted),
                Err(error) if access.session.state() == SessionState::Ready => Err(error.into()),
                Err(_) => Ok(Requested::SessionEnded),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_loads_its_parameters_and_branches_to_the_sender() {
        let stub = console_stub(0x20ffc0, 0x02f3eef8, &[0x87, 0, 0, 0, 1, 0, 0, 0]).unwrap();
        let words: Vec<u32> = stub[..16]
            .chunks(4)
            .map(|w| u32::from_le_bytes(w.try_into().unwrap()))
            .collect();
        assert_eq!(words[0], 0x1000_0080, "adr x0, #16");
        assert_eq!(
            (words[1] >> 26, (words[1] & 0x03ff_ffff) << 2),
            (0b000101, 0x02f3eef8 - 0x20ffc4)
        );
        assert_eq!(&stub[16..], &[0x87, 0, 0, 0, 1, 0, 0, 0]);
    }

    #[test]
    fn stub_refuses_a_sender_beyond_direct_branch_range() {
        assert!(console_stub(0x200000, 0x2000_0000, &[0; 8]).is_err());
    }

    #[test]
    fn deferred_guard_contract_uses_the_published_timer_and_wake_abi() {
        let restart: FirmwareRestart = serde_json::from_str(include_str!(
            "../../../catalog/firmware/x70/1.10/restart.generated.json"
        ))
        .unwrap();
        let contract = deferred_guard_contract(&restart, 600_000).unwrap();
        let words: Vec<u32> = contract
            .as_chunks::<4>()
            .0
            .iter()
            .map(|bytes| u32::from_le_bytes(*bytes))
            .collect();
        assert_eq!(words.len(), 7);
        assert_eq!(words[0], 0x93d4bc);
        assert_eq!(words[1], 0x93d6b4);
        assert_eq!(words[2], restart.mechanism.sender as u32);
        assert_eq!(words[3], restart.mechanism.request);
        assert_eq!(words[4], WAKE_DEADLINE);
        assert_eq!(words[5..], [0, 0]);
    }
}
