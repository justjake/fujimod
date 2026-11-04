use super::*;
use fujimod_emulator_devices::Host;
#[derive(Default)]
struct TestHost { now:u64, levels:Vec<(u32,bool)>, records:Vec<Record> }
impl Host for TestHost {
    fn now(&self)->u64{self.now}
    fn schedule(&mut self,_:usize,_:u64,_:u64){panic!("deadline-driven device")}
    fn set_irq(&mut self,l:u32,h:bool){self.levels.push((l,h));}
    fn raise_irq(&mut self,_:usize,_:u32)->Result<(),String>{panic!("level IRQ only")}
    fn ram_read(&self,_:u64,_:&mut[u8])->Result<(),String>{panic!("no DMA")}
    fn ram_write(&mut self,_:u64,_:&[u8])->Result<(),String>{panic!("no DMA")}
    fn record(&mut self,r:Record){self.records.push(r);}
}
const BASE:u64=0x1000;
fn model(peer:Peer)->ByteI2c {ByteI2c::new(BASE,17,Profile{reset:VirtualIdleReset{ccr:0,adr:0,csr:0,fsr:0},byte_ticks:7},peer).unwrap()}
fn peer()->Peer {Peer::Register(RegisterPeer{address:0x30,registers:BTreeMap::from([(0x10,0xa5),(0x11,0xb6)])})}
fn write(m:&mut ByteI2c,h:&mut TestHost,o:u64,v:u64){let mut cx=Cx::new(h,3,0xabcdef,4);m.write(&mut cx,BASE+o,1,v);assert!(cx.refusal().is_none(),"{:?}",cx.refusal());}
fn read(m:&mut ByteI2c,h:&mut TestHost,o:u64)->u64{let mut cx=Cx::new(h,2,0x8888,4);let v=m.read(&mut cx,BASE+o,1);assert!(cx.refusal().is_none());v}
fn advance(m:&mut ByteI2c,h:&mut TestHost,at:u64){h.now=at;m.advance(&mut Cx::new(h,usize::MAX,0,4));}
fn init(m:&mut ByteI2c,h:&mut TestHost){for(o,v)in[(24,3),(20,8),(8,0x2c),(4,0x42)]{write(m,h,o,v);}}
fn complete(m:&mut ByteI2c,h:&mut TestHost){let at=m.state.pending.as_ref().unwrap().deadline;advance(m,h,at);}
#[test]
fn empty_address_nack_is_timed_not_poll_driven(){
    let mut m=model(Peer::Empty);let mut h=TestHost::default();init(&mut m,&mut h);
    write(&mut m,&mut h,16,0x60);write(&mut m,&mut h,4,0x52);
    let saved=m.save();for _ in 0..100 {assert_eq!(read(&mut m,&mut h,0),0x80);}
    assert_eq!(m.save(),saved);advance(&mut m,&mut h,6);assert_eq!(m.save(),saved);
    advance(&mut m,&mut h,30);assert_eq!(read(&mut m,&mut h,0),0x99);
    assert_eq!(read(&mut m,&mut h,4),0x53);assert_eq!(h.levels.last(),Some(&(17,true)));
    let count=h.records.len();advance(&mut m,&mut h,100);assert_eq!(h.records.len(),count);
    write(&mut m,&mut h,4,0x42);assert_eq!(h.levels.last(),Some(&(17,false)));
    assert_eq!(read(&mut m,&mut h,0)&0x80,0x80);complete(&mut m,&mut h);assert_eq!(read(&mut m,&mut h,0),0);
}
#[test]
fn register_peer_write_repeated_start_read_ack_last_nack_stop(){
    let mut m=model(peer());let mut h=TestHost::default();init(&mut m,&mut h);
    for b in [0x60,0x10,0x44] {write(&mut m,&mut h,16,b);write(&mut m,&mut h,4,0x52);complete(&mut m,&mut h);}
    assert_eq!(m.state.transport.peer,Peer::Register(RegisterPeer{address:0x30,registers:BTreeMap::from([(0x10,0x44),(0x11,0xb6)])}));
    // Readdressing a write transaction selects a pointer again.
    write(&mut m,&mut h,16,0x60);write(&mut m,&mut h,4,0x72);complete(&mut m,&mut h);
    write(&mut m,&mut h,16,0x10);write(&mut m,&mut h,4,0x52);complete(&mut m,&mut h);
    write(&mut m,&mut h,16,0x61);write(&mut m,&mut h,4,0x72);complete(&mut m,&mut h);
    assert_eq!(read(&mut m,&mut h,0),0x81); // address event, no data yet
    write(&mut m,&mut h,4,0x5a);complete(&mut m,&mut h);assert_eq!(read(&mut m,&mut h,16),0x44);assert_eq!(read(&mut m,&mut h,0),0x80);
    write(&mut m,&mut h,4,0x52);complete(&mut m,&mut h);assert_eq!(read(&mut m,&mut h,16),0xb6);assert_eq!(read(&mut m,&mut h,0),0x90);
    write(&mut m,&mut h,4,0x42);complete(&mut m,&mut h);assert_eq!(m.state.phase,Phase::Idle);
}
#[test]
fn rejected_accesses_do_not_change_state_irq_or_events(){
    let mut m=model(Peer::Empty);let mut h=TestHost::default();init(&mut m,&mut h);
    for (o,size,v) in [(24,4,3),(1,1,0),(24,1,256),(0,1,0),(12,1,1),(4,1,0x46)] {
        let old=m.save();let effects=(h.levels.len(),h.records.len());
        let mut cx=Cx::new(&mut h,0,0,4);m.write(&mut cx,BASE+o,size,v);assert!(cx.refusal().is_some());
        assert_eq!(m.save(),old);assert_eq!((h.levels.len(),h.records.len()),effects);
    }
    write(&mut m,&mut h,16,0x60);write(&mut m,&mut h,4,0x52);
    let old=m.save();let mut cx=Cx::new(&mut h,0,0,4);m.write(&mut cx,BASE+20,1,2);assert!(cx.refusal().is_some());assert_eq!(m.save(),old);
}
#[test]
fn recovery_lines_are_explicit_idle_virtual_levels(){
    let mut m=model(Peer::Empty);let mut h=TestHost::default();
    for (v,expected) in [(0,0x30),(1,0x21),(2,0x12),(3,3),(0,0x30)] {write(&mut m,&mut h,28,v);assert_eq!(read(&mut m,&mut h,28),expected);}
    init(&mut m,&mut h);write(&mut m,&mut h,16,0x60);write(&mut m,&mut h,4,0x52);
    let saved=m.save();let mut cx=Cx::new(&mut h,0,0,4);m.write(&mut cx,BASE+28,1,1);assert!(cx.refusal().is_some());assert_eq!(m.save(),saved);
}
#[test]
fn checkpoints_bind_factory_are_atomic_and_resynchronize_levels(){
    let mut m=model(peer());let mut h=TestHost::default();init(&mut m,&mut h);
    write(&mut m,&mut h,16,0x60);write(&mut m,&mut h,4,0x52);
    let pending=m.save();let mut restored=model(peer());restored.restore(&pending).unwrap();
    assert_eq!(restored.next_deadline(2),Some(2));advance(&mut restored,&mut h,2);assert_eq!(h.levels.last(),Some(&(17,false)));assert_eq!(restored.next_deadline(2),Some(7));
    complete(&mut restored,&mut h);let event=restored.save();
    restored.restore(&event).unwrap();advance(&mut restored,&mut h,8);assert_eq!(h.levels.last(),Some(&(17,true)));
    let mut corruptions=Vec::new();
    let mut bad=event.clone();bad["state"]["pending"]=serde_json::json!({"submitted":0,"deadline":8,"operation":{"Write":1}});corruptions.push(bad);
    let mut bad=event.clone();bad["state"]["transport"]["selected"]=false.into();corruptions.push(bad);
    let mut bad=event.clone();bad["state"]["bsr"]=0.into();corruptions.push(bad);
    let mut bad=event.clone();bad["state"]["transport"]["peer"]["Register"]["registers"]["12"]=1.into();corruptions.push(bad);
    let mut bad=pending.clone();bad["state"]["pending"]["deadline"]=8.into();corruptions.push(bad);
    for bad in corruptions {
        let old=restored.save();assert!(restored.restore(&bad).is_err());assert_eq!(restored.save(),old);
    }
    let mut changed=event.clone();changed["profile"]["byte_ticks"]=8.into();assert!(restored.restore(&changed).is_err());
    assert!(model(Peer::Empty).restore(&event).is_err());
    // Disable cancels old deadline; advancing cannot commit the abandoned byte.
    write(&mut restored,&mut h,16,0x10);write(&mut restored,&mut h,4,0x52);write(&mut restored,&mut h,8,0x0c);
    advance(&mut restored,&mut h,100);assert_eq!(restored.state.phase,Phase::Idle);assert_eq!(restored.next_deadline(100),None);
}
#[test]
fn unknown_synthetic_cell_refuses_before_submission_and_context_is_irrelevant(){
    let mut a=model(peer());let mut b=model(peer());let mut ha=TestHost::default();let mut hb=TestHost::default();
    for(o,v)in[(24,3),(20,8),(8,0x2c),(4,0x42),(16,0x60),(4,0x52)] {
        let mut ca=Cx::new(&mut ha,0,1,4);a.write(&mut ca,BASE+o,1,v);assert!(ca.refusal().is_none());
        let mut cb=Cx::new(&mut hb,3,0xdeadbeef,4);b.write(&mut cb,BASE+o,1,v);assert!(cb.refusal().is_none());
    }
    advance(&mut a,&mut ha,7);advance(&mut b,&mut hb,7);assert_eq!(a.save(),b.save());assert_eq!(ha.levels,hb.levels);
    write(&mut a,&mut ha,16,0x99);let old=a.save();let mut cx=Cx::new(&mut ha,0,0,4);a.write(&mut cx,BASE+4,1,0x52);assert!(cx.refusal().is_some());assert_eq!(a.save(),old);
}

#[test]
fn reachable_checkpoints_and_interrupt_mask_preserve_event() {
    let mut m=model(peer());let mut h=TestHost::default();init(&mut m,&mut h);
    for (o,v) in [(16,0x60),(4,0x52)] {
        write(&mut m,&mut h,o,v);
        model(peer()).restore(&m.save()).unwrap();
    }
    complete(&mut m,&mut h);
    // Retaining INT while disabling INTE changes only the level, no new byte.
    write(&mut m,&mut h,4,0x51);assert_eq!(h.levels.last(),Some(&(17,false)));
    assert!(m.state.pending.is_none());assert_eq!(m.state.bcr&INT,INT);
    write(&mut m,&mut h,4,0x53);assert_eq!(h.levels.last(),Some(&(17,true)));
    for (o,v) in [(16,0x10),(4,0x52)] {write(&mut m,&mut h,o,v);model(peer()).restore(&m.save()).unwrap();}
    complete(&mut m,&mut h);
    for (o,v) in [(16,0x61),(4,0x72)] {write(&mut m,&mut h,o,v);model(peer()).restore(&m.save()).unwrap();}
    complete(&mut m,&mut h);model(peer()).restore(&m.save()).unwrap();
    write(&mut m,&mut h,4,0x52);model(peer()).restore(&m.save()).unwrap();
    complete(&mut m,&mut h);model(peer()).restore(&m.save()).unwrap();
    assert_eq!(read(&mut m,&mut h,16),0xa5);model(peer()).restore(&m.save()).unwrap();
    write(&mut m,&mut h,4,0x42);model(peer()).restore(&m.save()).unwrap();
    complete(&mut m,&mut h);model(peer()).restore(&m.save()).unwrap();
}

#[test]
fn stop_cancels_inflight_address_without_peer_or_irq_completion() {
    let mut m=model(peer());let mut h=TestHost::default();init(&mut m,&mut h);
    write(&mut m,&mut h,16,0x60);write(&mut m,&mut h,4,0x52);
    h.now=2;write(&mut m,&mut h,4,0x42);model(peer()).restore(&m.save()).unwrap();
    advance(&mut m,&mut h,7);assert!(!m.state.transport.selected);assert_eq!(m.state.bcr&INT,0);
    advance(&mut m,&mut h,9);assert_eq!(m.state.phase,Phase::Idle);assert_eq!(m.state.bsr,0);
    assert_eq!(m.state.transport.pointer,None);assert_eq!(h.levels.last(),Some(&(17,false)));
}

#[test]
fn completed_event_and_address_data_corruptions_are_atomic() {
    let mut m=model(peer());let mut h=TestHost::default();init(&mut m,&mut h);
    write(&mut m,&mut h,16,0x60);write(&mut m,&mut h,4,0x52);complete(&mut m,&mut h);
    let write_event=m.save();assert_eq!(write_event["state"]["bsr"],0x89);
    let mut bad=write_event.clone();bad["state"]["bcr"]=0x52.into();
    let effects=(h.records.len(),h.levels.len());assert!(m.restore(&bad).is_err());assert_eq!(m.save(),write_event);
    assert_eq!((h.records.len(),h.levels.len()),effects);
    write(&mut m,&mut h,16,0x10);write(&mut m,&mut h,4,0x52);complete(&mut m,&mut h);
    write(&mut m,&mut h,16,0x61);write(&mut m,&mut h,4,0x72);complete(&mut m,&mut h);
    let read_address=m.save();assert_eq!(read_address["state"]["bsr"],0x81);
    let mut bad=read_address.clone();bad["state"]["rx_ready"]=true.into();
    assert!(m.restore(&bad).is_err());assert_eq!(m.save(),read_address);
    let mut bad=read_address.clone();bad["state"]["bcr"]=0x52.into();
    assert!(m.restore(&bad).is_err());assert_eq!(m.save(),read_address);
}
fn field<'a>(r:&'a Record,key:&str)->Option<&'a Field>{r.fields.iter().find(|(k,_)|k==key).map(|(_,v)|v)}
#[test]
fn late_phase_records_identify_bus_inputs_and_declared_completion_time() {
    let mut m=model(Peer::Empty);let mut h=TestHost::default();init(&mut m,&mut h);
    write(&mut m,&mut h,16,0x61);write(&mut m,&mut h,4,0x52);advance(&mut m,&mut h,100);
    let r=h.records.last().unwrap();
    for (key,value) in [("operation",Field::constant("address")),("address7",Field::int(0x30)),("direction",Field::constant("read")),("peer_ack",Field::Bool(false)),("deadline",Field::int(7)),("tick",Field::int(7))] {assert_eq!(field(r,key),Some(&value));}
    let mut m=model(peer());let mut h=TestHost::default();init(&mut m,&mut h);
    for b in [0x60,0x10] {write(&mut m,&mut h,16,b);write(&mut m,&mut h,4,0x52);complete(&mut m,&mut h);}
    let r=h.records.last().unwrap();assert_eq!(field(r,"data"),Some(&Field::int(0x10)));assert_eq!(field(r,"peer_ack"),Some(&Field::Bool(true)));
    write(&mut m,&mut h,16,0x61);write(&mut m,&mut h,4,0x72);complete(&mut m,&mut h);
    write(&mut m,&mut h,4,0x52);complete(&mut m,&mut h);
    let r=h.records.last().unwrap();assert_eq!(field(r,"master_ack"),Some(&Field::Bool(false)));assert_eq!(field(r,"data"),Some(&Field::int(0xa5)));assert_eq!(field(r,"address7"),Some(&Field::int(0x30)));
    read(&mut m,&mut h,16);write(&mut m,&mut h,4,0x42);complete(&mut m,&mut h);
    let stop=h.records.last().unwrap();
    assert_eq!(field(stop,"operation"),Some(&Field::constant("stop")));
    assert_eq!(field(stop,"base"),Some(&Field::int(BASE)));
    assert_eq!(field(stop,"deadline"),Some(&Field::int(h.now)));
    assert_eq!(field(stop,"direction"),None);
}
