//! Fujitsu/Socionext-style byte I2C master. Register/bit facts are corroborated
//! by Linux's i2c-synquacer driver; this is an independent bounded model.
//!
//! All completion latency is explicit virtual time, not measured bus timing.
//! No slave, general-call, ten-bit, stretching, arbitration or electrical model
//! is provided. Unsupported operations refuse before effects. Idle recovery
//! drives virtual pulled-high lines only; it is not a sub-byte waveform model.
use fujimod_emulator_devices::{Cx, Device, Record, Value as Field};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

const INT: u8 = 1;
const INTE: u8 = 2;
const ACK: u8 = 8;
const MSS: u8 = 0x10;
const SCC: u8 = 0x20;
const BEIE: u8 = 0x40;
const BER: u8 = 0x80;

/// Explicit fixture, with no claim about silicon reset values.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VirtualIdleReset {
    pub ccr: u8,
    pub adr: u8,
    pub csr: u8,
    pub fsr: u8,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub reset: VirtualIdleReset,
    /// Positive host virtual ticks per address, data byte, or STOP.
    pub byte_ticks: u64,
}
/// Explicit synthetic register-file protocol: first write byte selects a
/// pointer; later writes and reads increment it. Only declared cells exist.
/// At most 128 byte values may be declared or checkpointed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterPeer {
    pub address: u8,
    pub registers: BTreeMap<u8, u8>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Peer { Empty, Register(RegisterPeer) }
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Transport {
    peer: Peer,
    selected: bool,
    pointer: Option<u8>,
    expect_pointer: bool,
}
impl Transport {
    fn address(&mut self, byte: u8) -> bool {
        self.selected = matches!(&self.peer, Peer::Register(p) if p.address == byte >> 1);
        self.expect_pointer = self.selected && byte & 1 == 0;
        self.selected
    }
    fn check_write(&self, byte: u8) -> Result<(), String> {
        let Peer::Register(p) = &self.peer else { return Err("No selected I2C peer".into()); };
        let cell = if self.expect_pointer { byte } else { self.pointer.ok_or("Register pointer missing")? };
        if !self.selected || !p.registers.contains_key(&cell) { return Err("Undeclared synthetic register write/pointer".into()); }
        Ok(())
    }
    fn write(&mut self, byte: u8) {
        if self.expect_pointer { self.pointer = Some(byte); self.expect_pointer = false; }
        else if let Peer::Register(p) = &mut self.peer {
            let pointer = self.pointer.unwrap(); p.registers.insert(pointer, byte);
            self.pointer = Some(pointer.wrapping_add(1));
        }
    }
    fn read(&self) -> Result<u8, String> {
        if self.selected {
            if let Peer::Register(p) = &self.peer {
                if let Some(v) = self.pointer.and_then(|a| p.registers.get(&a)) { return Ok(*v); }
            }
        }
        Err("Undeclared synthetic register read".into())
    }
    fn consume(&mut self) { self.pointer = self.pointer.map(|p| p.wrapping_add(1)); }
    fn stop(&mut self) { self.selected = false; self.expect_pointer = false; }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Phase { Idle, Write, Read, Nacked, ReadEnded }
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Operation { Address(u8), Write(u8), Read { ack: bool, byte: u8 }, Stop }
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending { submitted: u64, deadline: u64, operation: Operation }
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    ccr: u8, adr: u8, csr: u8, fsr: u8, bcr: u8, bsr: u8, dar: u8,
    staged: bool, rx_ready: bool, low_drives: u8, phase: Phase,
    pending: Option<Pending>, transport: Transport,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot { version: u8, base: u64, irq: u32, profile: Profile, factory: Value, state: State }

pub struct ByteI2c {
    base: u64, irq: u32, profile: Profile, factory: Value, initial_peer: Peer,
    state: State,
    // Host synchronization is not guest state. A restored device requests an
    // immediate advance, which drives even a low line before future work.
    irq_dirty: bool,
}
impl ByteI2c {
    pub fn new(base: u64, irq: u32, profile: Profile, peer: Peer) -> Result<Self, String> {
        if base & 3 != 0 || base.checked_add(0x20).is_none() || profile.byte_ticks == 0
            || profile.reset.ccr & 0x80 != 0 || profile.reset.adr != 0 || profile.reset.csr > 0x3f {
            return Err("Invalid explicit byte I2C idle profile".into());
        }
        if let Peer::Register(p) = &peer {
            if !(8..=0x77).contains(&p.address) || p.registers.is_empty() || p.registers.len() > 128 {
                return Err("Invalid synthetic I2C peer address or register count".into());
            }
        }
        let factory = fujimod_emulator_devices::fingerprint::bytes(&serde_json::to_vec(&peer).unwrap());
        let r = &profile.reset;
        let state = State { ccr:r.ccr, adr:r.adr, csr:r.csr, fsr:r.fsr, bcr:0, bsr:0, dar:0,
            staged:false, rx_ready:false, low_drives:0, phase:Phase::Idle, pending:None,
            transport:Transport { peer:peer.clone(), selected:false, pointer:None, expect_pointer:false } };
        Ok(Self {base, irq, profile, factory, initial_peer:peer, state, irq_dirty:true})
    }
    pub fn windows(&self) -> Vec<(u64,u64)> { vec![(self.base, self.base + 0x20)] }
    fn offset(&self, address:u64, size:u8) -> Result<u64,String> {
        let o = address.checked_sub(self.base).ok_or("Byte I2C address below base")?;
        if size != 1 || o > 0x1c || o & 3 != 0 { return Err("Byte I2C requires a known byte register".into()); }
        Ok(o)
    }
    fn irq(&self) -> bool { self.state.bcr & (INT|INTE) == INT|INTE || self.state.bcr & (BER|BEIE) == BER|BEIE }
    fn sync_irq(&mut self, cx:&mut Cx<'_>) { cx.set_irq(self.irq,self.irq()); self.irq_dirty=false; }
    fn command(&self, value:u8) -> Result<Option<Operation>,String> {
        let s=&self.state;
        if value & 4 != 0 { return Err("I2C general-call mode unsupported".into()); }
        // INT and BER are clear-on-zero, never guest-generated status.
        if value & INT != 0 && s.bcr & INT == 0 || value & BER != 0 && s.bcr & BER == 0 {
            return Err("I2C status cannot be set by software".into());
        }
        let control=value & !(INT|BER);
        if value & INT != 0 {
            if s.pending.is_some() || s.staged || control & !(INTE|BEIE) != s.bcr & !(INT|BER|INTE|BEIE) {
                return Err("I2C retained event permits only interrupt enable changes".into());
            }
            return Ok(None);
        }
        if s.pending.as_ref().is_some_and(|p| p.operation==Operation::Stop) {
            return Err("I2C STOP already pending".into());
        }
        if control & MSS == 0 {
            if s.rx_ready { return Err("I2C STOP requires receive data consumption".into()); }
            if control & (ACK|SCC) != 0 { return Err("Unsupported idle I2C control".into()); }
            return Ok((s.phase != Phase::Idle || s.pending.is_some()).then_some(Operation::Stop));
        }
        if s.ccr & 0x20 == 0 || s.low_drives != 0 || s.pending.is_some() || s.rx_ready {
            return Err("I2C command while disabled, recovering, pending or unread".into());
        }
        if control & SCC != 0 || s.phase == Phase::Idle {
            if !s.staged || !(8..=0x77).contains(&(s.dar>>1)) || control & ACK != 0 {
                return Err("I2C START requires a staged seven-bit address".into());
            }
            return Ok(Some(Operation::Address(s.dar)));
        }
        if s.bcr & INT == 0 { return Err("I2C continuation requires completed event".into()); }
        match s.phase {
            Phase::Write if s.staged && control & ACK == 0 => {
                s.transport.check_write(s.dar)?; Ok(Some(Operation::Write(s.dar)))
            }
            Phase::Read if !s.staged => Ok(Some(Operation::Read {ack:control&ACK!=0,byte:s.transport.read()?})),
            _ => Err("Unsupported I2C phase continuation".into()),
        }
    }
    fn write_checked(&mut self, cx:&mut Cx<'_>, offset:u64, value:u8) -> Result<(),String> {
        let mut next=self.state.clone();
        match offset {
            0 => return Err("I2C bus status is read-only".into()),
            4 => {
                let op=self.command(value)?;
                let pending=if let Some(operation)=op {
                    Some(Pending {submitted:cx.now(),deadline:cx.now().checked_add(self.profile.byte_ticks).ok_or("I2C deadline overflow")?,operation})
                } else {None};
                next.bcr=(value & !(INT|BER|SCC)) | (next.bcr & value & (INT|BER));
                if let Some(p)=&pending {
                    if matches!(p.operation,Operation::Address(_)) {next.bsr=0x80;}
                    next.staged=false;
                }
                next.pending=pending;
            }
            8|12|20|24 => {
                // Disabling is an explicit cancellation, without bus-byte effects.
                if offset==8 && value & 0x20 == 0 && value & 0x80 == 0 {
                    next.pending=None; next.phase=Phase::Idle; next.bcr=0; next.bsr=0;
                    next.staged=false; next.rx_ready=false; next.transport.stop(); next.ccr=value;
                } else {
                    if next.phase!=Phase::Idle || next.pending.is_some() {return Err("Active I2C reconfiguration unsupported".into());}
                    match offset {
                        8 if value & 0x80 == 0 => next.ccr=value,
                        12 if value==0 => next.adr=value,
                        20 if value<=0x3f => next.csr=value,
                        24 => next.fsr=value,
                        _ => return Err("Unsupported I2C configuration value".into()),
                    }
                }
            }
            16 => {
                if next.pending.is_some() || next.rx_ready || next.staged || matches!(next.phase,Phase::ReadEnded|Phase::Nacked) {
                    return Err("I2C data staging in illegal phase".into());
                }
                next.dar=value; next.staged=true;
            }
            28 => {
                if value>3 || next.phase!=Phase::Idle || next.pending.is_some() {return Err("I2C virtual recovery requires idle low-drive bits".into());}
                next.low_drives=value;
            }
            _ => unreachable!(),
        }
        self.state=next; self.sync_irq(cx); Ok(())
    }
    fn validate(&self, s:&State) -> Result<(),String> {
        if s.ccr&0x80!=0 || s.adr!=0 || s.csr>0x3f || s.bcr&0xa4!=0 || s.bsr&!0x99!=0 || s.low_drives>3 {
            return Err("Invalid byte I2C checkpoint register bits".into());
        }
        match (&self.initial_peer,&s.transport.peer) {
            (Peer::Empty,Peer::Empty) if s.transport.pointer.is_none() && !s.transport.selected && !s.transport.expect_pointer => (),
            (Peer::Register(a),Peer::Register(b)) if a.address==b.address && a.registers.keys().eq(b.registers.keys()) && s.transport.pointer.is_none_or(|pointer| a.registers.keys().any(|key| *key==pointer || key.wrapping_add(1)==pointer)) => (),
            _ => return Err("Changed byte I2C peer factory".into()),
        }
        let addressing = s.pending.as_ref().is_some_and(|p| matches!(p.operation,Operation::Address(_)));
        let stopping = s.pending.as_ref().is_some_and(|p| p.operation==Operation::Stop);
        if s.transport.selected && !matches!(s.transport.peer,Peer::Register(_))
            || s.transport.expect_pointer && !matches!(s.transport.peer,Peer::Register(_))
            || s.transport.selected != matches!(s.phase,Phase::Read|Phase::Write|Phase::ReadEnded)
            || s.transport.expect_pointer && (!s.transport.selected || s.phase!=Phase::Write)
            || s.phase!=Phase::Idle && s.pending.is_none() && s.bcr&INT==0
            || s.rx_ready && s.bsr&1!=0
            || s.rx_ready && (s.phase!=Phase::Read && s.phase!=Phase::ReadEnded || s.bcr&INT==0 || s.staged)
            || s.phase==Phase::Idle && s.pending.is_some() && !addressing && !stopping
            || !addressing && !stopping && match s.phase {
                Phase::Idle => s.bsr!=0 || s.bcr&INT!=0 || s.rx_ready,
                Phase::Write => ![0x89,0x88].contains(&s.bsr),
                Phase::Read => ![0x81,0x80].contains(&s.bsr),
                Phase::ReadEnded => s.bsr!=0x90,
                Phase::Nacked => ![0x91,0x99].contains(&s.bsr),
            }
            || s.phase==Phase::Idle && s.pending.is_none() && (s.bsr&0x80!=0 || s.bcr&MSS!=0 || s.transport.selected)
            || s.phase!=Phase::Idle && (s.bsr&0x80==0 || s.ccr&0x20==0 || s.low_drives!=0)
            || s.phase!=Phase::Idle && s.pending.as_ref().is_none_or(|p| p.operation!=Operation::Stop) && s.bcr&MSS==0 {
            return Err("Incoherent byte I2C phase checkpoint".into());
        }
        if let Some(p)=&s.pending {
            if p.submitted.checked_add(self.profile.byte_ticks)!=Some(p.deadline) || s.bcr&INT!=0 || s.rx_ready || s.staged {
                return Err("Invalid byte I2C pending deadline/event".into());
            }
            let legal=match p.operation {
                Operation::Address(b) => s.ccr&0x20!=0 && (8..=0x77).contains(&(b>>1)) && s.bcr&MSS!=0 && s.bsr==0x80 && s.dar==b && s.low_drives==0,
                Operation::Write(b) => s.phase==Phase::Write && s.transport.check_write(b).is_ok(),
                Operation::Read{byte,ack} => s.phase==Phase::Read && s.transport.read()==Ok(byte) && (s.bcr&ACK!=0)==ack,
                Operation::Stop => s.bsr&0x80!=0 && s.bcr&MSS==0,
            };
            if !legal {return Err("Invalid byte I2C pending operation".into());}
        }
        Ok(())
    }
}
impl Device for ByteI2c {
    fn name(&self)-> &'static str {"byte-i2c"}
    fn read(&mut self,cx:&mut Cx<'_>,address:u64,size:u8)->u64 {
        let o=match self.offset(address,size) {Ok(o)=>o,Err(e)=>{cx.fail(e);return 0;}};
        let s=&mut self.state;
        let v=match o {0=>s.bsr,4=>s.bcr,8=>s.ccr,12=>s.adr,16=>s.dar,20=>s.csr,24=>s.fsr,
            28=>s.low_drives | if s.low_drives&1==0 {0x10}else{0} | if s.low_drives&2==0 {0x20}else{0},_=>unreachable!()};
        if o==16 && matches!(s.phase,Phase::Read|Phase::ReadEnded) {
            if !s.rx_ready {cx.fail("I2C receive data not ready");return 0;}
            s.rx_ready=false;
        }
        if self.irq_dirty {self.sync_irq(cx);} u64::from(v)
    }
    fn write(&mut self,cx:&mut Cx<'_>,address:u64,size:u8,value:u64) {
        let result=self.offset(address,size).and_then(|o| {
            if value>255 {Err("I2C value exceeds byte".into())} else {self.write_checked(cx,o,value as u8)}
        });
        if let Err(e)=result {cx.fail(e);}
    }
    fn advance(&mut self,cx:&mut Cx<'_>) {
        if self.irq_dirty {self.sync_irq(cx);}
        let Some(p)=self.state.pending.clone().filter(|p|p.deadline<=cx.now()) else{return;};
        let s=&mut self.state; s.pending=None;
        let mut fields=vec![("base",Field::int(self.base)),("deadline",Field::int(p.deadline)),("tick",Field::int(p.deadline))];
        if s.transport.selected {
            if let Peer::Register(peer)=&s.transport.peer {
                if !matches!(p.operation,Operation::Address(_)) { fields.push(("address7",Field::int(peer.address))); }
            }
        }
        match p.operation {
            Operation::Address(b)=>{
                let ack=s.transport.address(b);
                fields.extend([("operation",Field::constant("address")),("address7",Field::int(b>>1)),
                    ("direction",Field::constant(if b&1==0 {"write"} else {"read"})),("peer_ack",Field::Bool(ack))]);
                s.phase=if !ack {Phase::Nacked} else if b&1==0 {Phase::Write}else{Phase::Read};
                s.bsr=0x81 | if b&1==0 {8}else{0} | if ack {0}else{0x10};
                s.bcr|=INT;
            }
            Operation::Write(b)=>{
                s.transport.write(b);s.bsr=0x88;s.bcr|=INT;
                fields.extend([("operation",Field::constant("write")),("direction",Field::constant("write")),("data",Field::int(b)),("peer_ack",Field::Bool(true))]);
            }
            Operation::Read{ack,byte}=>{
                fields.extend([("operation",Field::constant("read")),("direction",Field::constant("read")),("data",Field::int(byte)),("master_ack",Field::Bool(ack))]);
                s.transport.consume();s.dar=byte;s.rx_ready=true;s.bsr=0x80|if ack{0}else{0x10};
                if !ack {s.phase=Phase::ReadEnded;} s.bcr|=INT;
            }
            Operation::Stop=>{
                fields.push(("operation",Field::constant("stop")));
                s.phase=Phase::Idle;s.bsr=0;s.transport.stop();s.rx_ready=false;}
        }
        cx.at_tick(p.deadline,|cx|cx.record(Record::new("Byte I2C virtual phase completed",fields)));
        self.sync_irq(cx);
    }
    fn next_deadline(&self,now:u64)->Option<u64> {if self.irq_dirty{Some(now)}else{self.state.pending.as_ref().map(|p|p.deadline)}}
    fn save(&self)->Value {serde_json::to_value(Snapshot{version:1,base:self.base,irq:self.irq,profile:self.profile.clone(),factory:self.factory.clone(),state:self.state.clone()}).unwrap()}
    fn restore(&mut self,v:&Value)->Result<(),String> {
        let snapshot:Snapshot=serde_json::from_value(v.clone()).map_err(|e|e.to_string())?;
        if snapshot.version!=1 || snapshot.base!=self.base || snapshot.irq!=self.irq || snapshot.profile!=self.profile || snapshot.factory!=self.factory {
            return Err("Changed byte I2C checkpoint binding".into());
        }
        self.validate(&snapshot.state)?;self.state=snapshot.state;self.irq_dirty=true;Ok(())
    }
}
#[cfg(test)]
mod tests;
