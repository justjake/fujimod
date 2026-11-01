//! Camera restart over debug USB through stock code, from a firmware's catalog
//! restart facts.
//!
//! The camera must report the selected product and internal version, and
//! every stock routine on the path must match before anything is written. One
//! borrowed diagnostic query then calls the stock power-off sender with the
//! power-off request and a wake deadline; the stock power-off arms the power
//! controller's alarm and cuts power. The host observes the camera return.
use crate::{
    Limits,
    bulk::{Access, BulkError, Result, check_camera, hex},
};
use fujimod_catalog_restart::{FirmwareRestart, RestartMechanism};
use fujimod_usb::{
    ff80::{
        Session, SessionState,
        commands::{diagnostic::DiagnosticTag, memory::Address},
    },
    operations::{borrowed::BorrowedQueries, checks},
};

/// A deadline at or before the camera's clock arms the stock minimum, one second.
pub const WAKE_DEADLINE: u32 = 1;

/// How the request left the camera.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Requested {
    /// The query returned; the power-off runs after it.
    Accepted,
    /// The session ended during the borrowed query: the camera cut power
    /// before answering. The session cannot issue further commands.
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

/// Request a restart. Without `recover`, the camera must be idle in REC.
/// Nothing is written unless identity, stock code and the stock callback match.
pub async fn request(
    session: &mut Session,
    restart: &FirmwareRestart,
    limits: Limits,
    recover: bool,
) -> Result<Requested> {
    let RestartMechanism::WakeDeadline {
        query,
        sender,
        request,
        request_offset,
        deadline_offset,
        ui,
        ..
    } = &restart.mechanism
    else {
        return Err(BulkError::Check(
            "This firmware's located restart cuts power without a wake".into(),
        ));
    };
    check_camera(session, &restart.product, &restart.internal_version).await?;
    let alias = restart.code_alias;
    let mut access = Access { session, limits };
    for check in &restart.code_checks {
        let bytes = access.raw(check.address + alias, check.size).await?;
        require(
            hex(&checks::sha256_bytes(&bytes)) == check.sha256,
            &format!("Stock code differs: {}", check.name),
        )?;
    }
    // The table pointer may hold the cached or the uncached alias.
    let table = word(&access.raw(query.callback_table + alias, 4).await?);
    let physical = if table >= alias { table - alias } else { table };
    let slot = physical + query.callback_offset;
    require(
        word(&access.raw(slot + alias, 4).await?) == query.stock_callback,
        "Require the stock diagnostic query callback",
    )?;
    if !recover {
        let mode = word(
            &access
                .raw(ui.mode_address + alias, u64::from(ui.mode_size))
                .await?,
        );
        let pending = word(
            &access
                .raw(ui.pending_mode_address + alias, u64::from(ui.mode_size))
                .await?,
        );
        let menu = word(&access.raw(ui.menu_active_address + alias, 4).await?);
        require(
            mode == u64::from(ui.rec_mode) && pending == 0 && menu == 0,
            &format!(
                "Camera is not idle in REC (mode {mode}, pending {pending}, menu {menu}); \
                 request a recovery restart instead"
            ),
        )?;
    }
    let output = query.output_fields;
    let original = access.raw(output, u64::from(query.output_bytes)).await?;
    let mut parameters = vec![0; query.output_bytes as usize];
    for (offset, value) in [
        (*request_offset, *request),
        (*deadline_offset, WAKE_DEADLINE),
    ] {
        let at = offset as usize;
        parameters[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    let tag = u32::try_from(output)
        .map_err(|_| BulkError::Check("Query output fields beyond 32-bit addressing".into()))?;
    let mut borrowed = BorrowedQueries::new(
        address(slot)?,
        address(query.stock_callback)?,
        address(output)?,
        original,
    )?;
    borrowed.call(
        address(*sender)?,
        DiagnosticTag::Research(tag),
        vec![(0, 0)],
        parameters,
    )?;
    match borrowed.invoke(access.session, limits.memory).await {
        Ok(_) => Ok(Requested::Accepted),
        // The whole sequence ran and restored both words; the send was refused.
        Err(error) if access.session.state() == SessionState::Ready => Err(error.into()),
        Err(_) => Ok(Requested::SessionEnded),
    }
}
