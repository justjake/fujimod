//! Durable execution-owner state. External memory descriptions exclude host
//! addresses and bind to the receiving CPU only after its decode is restored.
use super::*;
use crate::durable::{bytes, decode, encode};
use serde::{Deserialize, Serialize};
#[cfg(test)]
mod tests;

const FORMAT: &str = "rprofile-r5f-v1";
type Map = (u64, Option<u64>, usize, u32);
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    format: String,
    #[serde(with = "bytes")]
    context: Vec<u8>,
    identity_input: u32,
    clock: Option<(u64, u64, (u64, u64))>,
    clock_overshoot: u64,
    policy: String,
    pmu: pmu::Counter,
    tcm: [Tcm; 2],
    maps: Vec<Map>,
    mmio: Vec<(u64, usize)>,
    #[serde(with = "bytes")]
    reservation: Vec<u8>,
    ring: Vec<u64>,
    budget: i64,
    head: u64,
    count: u64,
    cap: u64,
    synced: i64,
    settled_head: u64,
    thumb: bool,
    restarted: Option<u64>,
    user_stop: bool,
    irq: bool,
    hint: Option<u8>,
    exception: Option<(u8, u32)>,
    failure: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Tcm {
    register: u32,
    len: usize,
    #[serde(with = "bytes")]
    pages: Vec<u8>,
}
pub(super) struct Imported {
    owner: Identity,
    state: State,
}
impl Snapshot for Imported {
    fn owner(&self) -> Identity {
        self.owner
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn export(&self) -> Result<Vec<u8>, String> {
        encode(&self.state)
    }
}
pub(super) fn maps_description(maps: &[Mapping]) -> Vec<Map> {
    let mut description: Vec<Map> = Vec::new();
    for map in maps {
        if let Some(last) = description.last_mut()
            && last.0 + last.2 as u64 == map.address
            && last.3 == map.permissions
            && match (last.1, map.physical) {
                (Some(a), Some(b)) => a + last.2 as u64 == b,
                (None, None) => true,
                _ => false,
            }
        {
            last.2 += map.bytes;
        } else {
            description.push((map.address, map.physical, map.bytes, map.permissions));
        }
    }
    description
}
fn exception(value: arm::Exception) -> (u8, u32) {
    use arm::Exception::*;
    match value {
        UndefinedInstruction => (0, 0),
        SupervisorCall => (0, 1),
        PrefetchAbort => (0, 2),
        DataAbort => (0, 3),
        Irq => (0, 4),
        Fiq => (0, 5),
        Breakpoint => (0, 6),
        Unknown(v) => (1, v),
    }
}
fn decode_exception(value: (u8, u32)) -> Result<arm::Exception, String> {
    use arm::Exception::*;
    Ok(match value {
        (0, 0) => UndefinedInstruction,
        (0, 1) => SupervisorCall,
        (0, 2) => PrefetchAbort,
        (0, 3) => DataAbort,
        (0, 4) => Irq,
        (0, 5) => Fiq,
        (0, 6) => Breakpoint,
        (1, v) => Unknown(v),
        _ => return Err("invalid R-profile snapshot exception".into()),
    })
}
fn hint(value: Hint) -> u8 {
    match value {
        Hint::WaitForInterrupt => 0,
        Hint::WaitForEvent => 1,
        Hint::SendEvent => 2,
        Hint::SendEventLocal => 3,
    }
}
fn decode_hint(value: u8) -> Result<Hint, String> {
    Ok(match value {
        0 => Hint::WaitForInterrupt,
        1 => Hint::WaitForEvent,
        2 => Hint::SendEvent,
        3 => Hint::SendEventLocal,
        _ => return Err("invalid R-profile snapshot hint".into()),
    })
}
pub(super) fn export(saved: &Saved) -> Result<Vec<u8>, String> {
    let Saved {
        context,
        identity_input,
        clock,
        clock_overshoot,
        policy,
        pmu,
        tcm,
        maps,
        mmio,
        reservation,
        ring,
        budget,
        head,
        count,
        cap,
        synced,
        settled_head,
        thumb,
        restarted,
        user_stop,
        irq,
        hint: pending_hint,
        exception: pending_exception,
        failure,
    } = saved;
    let tcm = tcm
        .iter()
        .map(|(register, image)| {
            Ok(Tcm {
                register: *register,
                len: image.len(),
                pages: image.to_bytes()?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?
        .try_into()
        .map_err(|_| "R-profile snapshot TCM count differs")?;
    encode(&State {
        format: FORMAT.into(),
        context: context.export()?,
        identity_input: *identity_input,
        clock: clock.map(|c| (c.now, c.phase, c.ratio.parts())),
        clock_overshoot: *clock_overshoot,
        policy: policy.name().into(),
        pmu: *pmu,
        tcm,
        maps: maps_description(maps),
        mmio: mmio.clone(),
        reservation: reservation.to_bytes(),
        ring: ring.to_vec(),
        budget: *budget,
        head: *head,
        count: *count,
        cap: *cap,
        synced: *synced,
        settled_head: *settled_head,
        thumb: *thumb,
        restarted: *restarted,
        user_stop: *user_stop,
        irq: *irq,
        hint: pending_hint.map(hint),
        exception: pending_exception.map(exception),
        failure: failure.clone(),
    })
}
impl Imported {
    fn decode(&self, cpu: &Core, binding: bool) -> Result<Saved, String> {
        let e = &cpu.execution;
        e.checkpoint_boundary()?;
        let state = &self.state;
        if state.format != FORMAT {
            return Err("unknown R-profile snapshot format".into());
        }
        let maps = e.checkpoint_maps();
        if (binding && state.maps != maps_description(&maps)) || state.mmio != e.checkpoint_mmio() {
            return Err("R-profile snapshot external memory/MMIO geometry differs".into());
        }
        let context = e.cpu.import_snapshot(&state.context)?;
        let clock = state
            .clock
            .map(|(now, phase, (n, d))| {
                let ratio = fujimod_emulator_cpu::timeline::Ratio::new(n, d)?;
                ratio.advance_phase(phase, 0)?;
                Ok::<_, String>(Clock { now, phase, ratio })
            })
            .transpose()?;
        let tcm = state
            .tcm
            .iter()
            .zip(&e.tcm)
            .map(|(saved, bank)| {
                if saved.len != bank.memory.len()
                    || super::super::super::tcm::normalize(saved.len, saved.register)?
                        != saved.register
                {
                    return Err("R-profile snapshot TCM configuration differs".to_owned());
                }
                Ok((
                    saved.register,
                    Image::from_bytes(&saved.pages, bank.memory.len())?,
                ))
            })
            .collect::<Result<Vec<_>, String>>()?;
        super::super::super::tcm::validate_disjoint(
            state.tcm[0].len,
            state.tcm[0].register,
            state.tcm[1].len,
            state.tcm[1].register,
        )?;
        Ok(Saved {
            context,
            identity_input: state.identity_input,
            clock,
            clock_overshoot: state.clock_overshoot,
            policy: ExecutionPolicy::parse(&state.policy)?,
            pmu: state.pmu,
            tcm: tcm
                .try_into()
                .map_err(|_| "R-profile snapshot TCM count differs")?,
            maps,
            mmio: state.mmio.clone(),
            reservation: fujimod_emulator_cpu::exclusive::Saved::from_bytes(&state.reservation)?,
            ring: state
                .ring
                .clone()
                .try_into()
                .map_err(|_| "R-profile snapshot accounting ring length differs")?,
            budget: state.budget,
            head: state.head,
            count: state.count,
            cap: state.cap,
            synced: state.synced,
            settled_head: state.settled_head,
            thumb: state.thumb,
            restarted: state.restarted,
            user_stop: state.user_stop,
            irq: state.irq,
            hint: state.hint.map(decode_hint).transpose()?,
            exception: state.exception.map(decode_exception).transpose()?,
            failure: state.failure.clone(),
        })
    }
    pub(super) fn bind(&self, cpu: &Core) -> Result<Saved, String> {
        self.decode(cpu, true)
    }
}
pub(super) fn import(cpu: &Core, bytes: &[u8]) -> Result<Box<dyn Snapshot>, String> {
    let saved = Imported {
        owner: cpu.identity(),
        state: decode(bytes)?,
    };
    saved.decode(cpu, false)?;
    Ok(Box::new(saved))
}
