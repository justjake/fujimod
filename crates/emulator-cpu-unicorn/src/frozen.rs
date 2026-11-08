//! Retained mapping policy, including aliases on separate native engines.
//! Allocation owners retain interior-mutable storage; freezing is enforced here,
//! independently of the backend's privileged ROM-write behavior.
use fujimod_emulator_cpu::Identity;
use std::sync::{Mutex, atomic::{AtomicU64, Ordering}};
#[derive(Clone)]
struct Mapping { id:u64, core:Identity, address:u64, pointer:usize, bytes:usize, frozen:bool, mapped:bool }
static MAPPINGS:Mutex<Vec<Mapping>>=Mutex::new(Vec::new());
static POISON:Mutex<Vec<(Identity,String)>>=Mutex::new(Vec::new());
static NEXT:AtomicU64=AtomicU64::new(1);
fn overlaps(a:u64,n:usize,b:u64,m:usize)->bool {
    a < b.saturating_add(m as u64) && b < a.saturating_add(n as u64)
}
pub(super) fn ready(core:Identity)->Result<(),String> {
    match POISON.lock().unwrap().iter().find(|(owner,_)|*owner==core) {
        Some((_,message))=>Err(message.clone()), None=>Ok(())
    }
}
pub(super) fn poison(core:Identity,message:String)->String {
    let mut errors=POISON.lock().unwrap();
    if !errors.iter().any(|(owner,_)|*owner==core) {errors.push((core,message.clone()));}
    message
}
pub(super) fn views(core:Identity)->Vec<(u64,usize)> {
    let mut ranges=MAPPINGS.lock().unwrap().iter().filter(|m|m.core==core && m.frozen && m.mapped)
        .map(|m|(m.address,m.address+m.bytes as u64)).collect::<Vec<_>>();
    ranges.sort_unstable();let mut merged:Vec<(u64,u64)>=vec![];
    for (start,end) in ranges {
        if let Some(last)=merged.last_mut() {if start<=last.1 {last.1=last.1.max(end);continue;}}
        merged.push((start,end));
    }
    merged.into_iter().map(|(start,end)|(start,(end-start)as usize)).collect()
}
pub(super) fn check(core:Identity,address:u64,bytes:usize)->Result<(),String> {
    ready(core)?;
    address.checked_add(bytes as u64).ok_or("memory range overflow")?;
    if bytes!=0 && MAPPINGS.lock().unwrap().iter().any(|m|m.core==core && m.frozen && m.mapped && overlaps(address,bytes,m.address,m.bytes)) {
        return Err("mutation of frozen backing is refused".into());
    }
    Ok(())
}
pub(super) fn any(core:Identity)->bool {MAPPINGS.lock().unwrap().iter().any(|m|m.core==core && m.frozen)}
pub(super) struct Reservation { id:u64, committed:bool }
impl Reservation { pub(super) fn id(&self)->u64 {self.id} pub(super) fn commit(mut self) {self.committed=true;} }
impl Drop for Reservation {fn drop(&mut self) {if !self.committed {MAPPINGS.lock().unwrap().retain(|m|m.id!=self.id);}}}
pub(super) fn reserve(core:Identity,address:u64,pointer:usize,bytes:usize,frozen:bool)->Result<Reservation,String> {
    reserve_inner(core,address,pointer,bytes,frozen,false)
}
pub(super) fn reserve_replacement(core:Identity,address:u64,pointer:usize,bytes:usize)->Result<Reservation,String> {
    reserve_inner(core,address,pointer,bytes,false,true)
}
fn reserve_inner(core:Identity,address:u64,pointer:usize,bytes:usize,frozen:bool,replacing:bool)->Result<Reservation,String> {
    ready(core)?;
    if !replacing {check(core,address,bytes)?;}
    address.checked_add(bytes as u64).ok_or("memory range overflow")?;
    pointer.checked_add(bytes).ok_or("backing pointer range overflow")?;
    let mut mappings=MAPPINGS.lock().unwrap();
    if mappings.iter().any(|m|overlaps(pointer as u64,bytes,m.pointer as u64,m.bytes) && (m.frozen != frozen)) {
        return Err("frozen backing cannot have ordinary aliases".into());
    }
    let id=NEXT.fetch_add(1,Ordering::Relaxed);
    mappings.push(Mapping{id,core,address,pointer,bytes,frozen,mapped:true});
    Ok(Reservation{id,committed:false})
}
pub(super) fn remove(core:Identity,address:u64,bytes:usize) {remove_except(core,address,bytes,None)}
pub(super) fn remove_except(core:Identity,address:u64,bytes:usize,keep:Option<u64>) {
    let end=address+bytes as u64;
    let mut mappings=MAPPINGS.lock().unwrap();let old=std::mem::take(&mut *mappings);
    for m in old {
        if !m.mapped || keep==Some(m.id) || m.core!=core || !overlaps(address,bytes,m.address,m.bytes) {mappings.push(m);continue;}
        if m.frozen {
            let mut lease=m.clone();lease.mapped=false;
            if !mappings.iter().any(|old|!old.mapped && old.core==lease.core && old.pointer==lease.pointer && old.bytes==lease.bytes) {
                mappings.push(lease);
            }
        }
        if m.address<address {let mut left=m.clone();left.bytes=(address-m.address)as usize;mappings.push(left);}
        let old_end=m.address+m.bytes as u64;
        if end<old_end {let mut right=m;right.pointer+=(end-right.address)as usize;right.address=end;right.bytes=(old_end-end)as usize;mappings.push(right);}
    }
}
pub(super) fn drop_core(core:Identity) {
    MAPPINGS.lock().unwrap().retain(|m|m.core!=core);
    POISON.lock().unwrap().retain(|(owner,_)|*owner!=core);
}
