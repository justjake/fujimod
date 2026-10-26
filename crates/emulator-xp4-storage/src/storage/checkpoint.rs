//! Restore physical flash/controller state into the already connected graph.
//! Immutable flash bytes, geometry and interrupt routes belong to the factory.
use super::*;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(untagged, deny_unknown_fields)]
enum Pending {
    Erase {
        at: u64,
        erase_address: u64,
        size: usize,
    },
    Program {
        at: u64,
        program_address: u64,
        bytes: Vec<u8>,
    },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NorState {
    jedec: u32,
    write_enable: bool,
    pending: Option<Pending>,
    erase_bytes: usize,
    page_bytes: usize,
    erase_ticks: u64,
    page_ticks: u64,
    overlay: BTreeMap<usize, Vec<u8>>,
}
fn extent(address: u64, size: usize) -> bool {
    address
        .checked_sub(FLASH)
        .and_then(|a| a.checked_add(size as u64))
        .is_some_and(|end| end <= FLASH_SIZE)
}
impl Nor {
    fn restore_state(&mut self, state: NorState) -> Result<(), String> {
        let g = self.geometry;
        if state.jedec != self.jedec
            || state.erase_bytes != g.erase_bytes
            || state.page_bytes != g.page_bytes
            || state.erase_ticks != g.erase_ticks
            || state.page_ticks != g.page_ticks
            || state.overlay.iter().any(|(page, bytes)| {
                *page >= FLASH_SIZE as usize / g.page_bytes || bytes.len() != g.page_bytes
            })
            || state.pending.is_some() && state.write_enable
        {
            return Err("Invalid XP4 NOR checkpoint or changed physical input".into());
        }
        let pending = match state.pending {
            Some(Pending::Erase {
                at,
                erase_address: address,
                size,
            }) => {
                if ![4096, g.erase_bytes].contains(&size)
                    || address.wrapping_sub(FLASH) % size as u64 != 0
                    || !extent(address, size)
                {
                    return Err("Invalid pending XP4 NOR erase".into());
                }
                Some((at, Mutation::Erase { address, size }))
            }
            Some(Pending::Program {
                at,
                program_address: address,
                bytes,
            }) => {
                if bytes.is_empty()
                    || !extent(address, bytes.len())
                    || address.wrapping_sub(FLASH) as usize % g.page_bytes + bytes.len()
                        > g.page_bytes
                {
                    return Err("Invalid pending XP4 NOR program".into());
                }
                Some((at, Mutation::Program { address, bytes }))
            }
            None => None,
        };
        self.write_enable = state.write_enable;
        self.overlay = state.overlay;
        self.pending = pending;
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FlashState {
    registers: Vec<u32>,
    next: Option<(u64, u64)>,
    operations: u64,
    jedec: u32,
    nor: NorState,
    stream: Option<Request>,
    program_fifo: Option<(u64, Vec<u8>)>,
    irq: u32,
    irq_level: bool,
}
impl SerialFlashController {
    pub(super) fn restore_state(&mut self, value: &Value) -> Result<(), String> {
        let state: FlashState = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        let registers: [u32; 64] = state
            .registers
            .try_into()
            .map_err(|_| "Invalid XP4 flash register count")?;
        if state.irq != self.irq
            || state.jedec != state.nor.jedec
            || state
                .next
                .is_some_and(|(_, token)| ![1, 2, 4].contains(&token))
            || state
                .stream
                .as_ref()
                .is_some_and(|r| !extent(r.address, r.size) || !r.accepted && r.accepted_at != 0)
            || state
                .program_fifo
                .as_ref()
                .is_some_and(|(a, b)| !extent(*a, b.len()))
        {
            return Err("Invalid XP4 serial-flash checkpoint or route".into());
        }
        let mut port = self.port.lock().unwrap();
        port.nor.restore_state(state.nor)?;
        port.request = state.stream;
        port.program_data = state.program_fifo;
        self.regs = registers;
        self.due = state.next;
        self.operations = state.operations;
        self.irq_level = state.irq_level;
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DmaState {
    channels: Vec<Channel>,
    irq_routes: [u32; 16],
    request_mux: BTreeMap<u64, u32>,
    request_mux_changed: [u64; 16],
}
impl DmaController {
    pub(super) fn restore_state(&mut self, value: &Value) -> Result<(), String> {
        let state: DmaState = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        if state.irq_routes != self.routes
            || state.channels.len() != 16
            || state.channels.iter().any(|c| {
                c.due.is_some_and(|at| !c.submitted || at < c.submitted_at)
                    || c.submitted && u64::from(c.regs[4]) + 1 > FLASH_SIZE
            })
            || state.request_mux.iter().any(|(a, v)| {
                !(0xfff4a500..0xfff4a520).contains(a) || a & 3 != 0 || v & !0x3f3f != 0
            })
        {
            return Err("Invalid XP4 DMA checkpoint or routes".into());
        }
        self.channels = state
            .channels
            .try_into()
            .map_err(|_| "Invalid XP4 DMA channel count")?;
        self.mux = state.request_mux;
        self.mux_changed = state.request_mux_changed;
        Ok(())
    }
}
