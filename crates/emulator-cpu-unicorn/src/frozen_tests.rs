use super::*;
use fujimod_emulator_cpu::memory::Backing;
const ROM:u64=0xf0007000;
fn pointer(b:&Backing)->NonNull<u8>{NonNull::new(b.as_ptr()).unwrap()}
fn words(b:&[u32])->Backing {
    let mut bytes=vec![0;4096];
    for (n,w) in b.iter().enumerate(){bytes[n*4..n*4+4].copy_from_slice(&w.to_le_bytes());}
    Backing::from_bytes(bytes.into_boxed_slice())
}
#[test]
fn frozen_high_address_fetch_data_and_wfi_preserve_canonical_bytes() {
    let source=words(&[0xe3a0002a,0xe59f2008,0xe5812000,0xe320f003,0xeafffffe,0x12345678]);
    let cpu=test_support::library().open(Model::CortexA7,1024*1024).unwrap();
    unsafe{cpu.map_frozen_backing(ROM,4096,5,pointer(&source)).unwrap();}
    cpu.map_zeroed(0x10000,4096,7).unwrap();
    cpu.write_register(arm::CORE[1],&0x10000u32.to_le_bytes()).unwrap();
    cpu.write_register(arm::CPSR,&0xd3u32.to_le_bytes()).unwrap();
    let mut before=vec![0;4096];cpu.read_memory(ROM,&mut before).unwrap();
    cpu.run(Run{begin:ROM,until:None,watchdog_us:0,instructions:Some(20)}).unwrap();
    let mut actual=[0;4];cpu.read_memory(0x10000,&mut actual).unwrap();
    assert_eq!(u32::from_le_bytes(actual),0x12345678);
    let mut after=vec![0;4096];cpu.read_memory(ROM,&mut after).unwrap();assert_eq!(after,before);
}
#[test]
fn frozen_host_mutations_aliases_protection_remap_and_restore_refuse_atomically() {
    let source=words(&[0xe320f003]);let replacement=words(&[0]);
    let cpu=test_support::library().open(Model::CortexA7,1024*1024).unwrap();
    let snapshot=cpu.snapshot(None).unwrap();
    cpu.map_zeroed(ROM-4096,4096,7).unwrap();
    unsafe{cpu.map_frozen_backing(ROM,4096,5,pointer(&source)).unwrap();}
    let mut before=vec![0;8192];cpu.read_memory(ROM-4096,&mut before).unwrap();
    assert!(cpu.write_memory(ROM,&[0;4]).is_err());
    assert!(cpu.write_memory(ROM-2,&[1;4]).is_err());
    assert!(cpu.protect(ROM,4096,7).is_err());
    assert!(cpu.map_zeroed(ROM,4096,7).is_err());
    assert!(cpu.restore(&*snapshot).is_err());
    unsafe{
        assert!(cpu.map_backing(ROM,4096,7,pointer(&replacement)).is_err());
        assert!(cpu.replace_physical(ROM,ROM,4096,7,pointer(&source)).is_err());
        assert!(cpu.map_backing(ROM+4096,4096,7,pointer(&source)).is_err());
        let peer=test_support::library().open(Model::CortexA7,1024*1024).unwrap();
        assert!(peer.map_backing(ROM,4096,5,pointer(&source)).is_err());
        peer.map_frozen_backing(ROM,4096,5,pointer(&source)).unwrap();
        assert!(peer.write_memory(ROM,&[1]).is_err());
    }
    let mut after=vec![0;8192];cpu.read_memory(ROM-4096,&mut after).unwrap();assert_eq!(after,before);
}
#[test]
fn preexisting_aliases_block_freeze_and_cleanup_does_not_leave_stale_ranges() {
    let source=words(&[0xe320f003]);
    let first=test_support::library().open(Model::CortexA7,1024*1024).unwrap();
    let second=test_support::library().open(Model::CortexA7,1024*1024).unwrap();
    unsafe{
        first.map_backing(ROM,4096,5,pointer(&source)).unwrap();
        assert!(second.map_frozen_backing(ROM,4096,5,pointer(&source)).is_err());
        // Ordinary ROM retains privileged host initialization.
        first.write_memory(ROM,&0xe1a00000u32.to_le_bytes()).unwrap();
        first.unmap(ROM,4096).unwrap();
        second.map_frozen_backing(ROM,4096,5,pointer(&source)).unwrap();
        assert!(first.map_backing(ROM,4096,7,pointer(&source)).is_err());
        drop(second);
        first.map_backing(ROM,4096,7,pointer(&source)).unwrap();
    }
}

#[test]
fn failed_map_partial_unmap_and_replacement_keep_alias_registry_exact() {
    let source=Backing::zeros(8192);let replacement=Backing::zeros(8192);
    let first=test_support::library().open(Model::CortexA7,1024*1024).unwrap();
    let second=test_support::library().open(Model::CortexA7,1024*1024).unwrap();
    unsafe {
        assert!(first.map_frozen_backing(ROM+4,4096,5,pointer(&source)).is_err());
        first.map_backing(ROM,8192,7,pointer(&source)).unwrap();
        first.unmap(ROM,4096).unwrap();
        second.map_frozen_backing(ROM,4096,5,pointer(&source)).unwrap();
        assert!(first.map_backing(ROM,4096,7,pointer(&source)).is_err());
        // A disjoint half of the same allocation can remain mutable.
        first.write_memory(ROM+4096,&[7]).unwrap();
        first.replace_physical(ROM+4096,ROM+4096,4096,7,pointer(&replacement)).unwrap();
        let tail=NonNull::new(source.as_ptr().add(4096)).unwrap();
        second.map_frozen_backing(ROM+4096,4096,5,tail).unwrap();
    }
}

#[test]
fn unmapping_a_view_does_not_thaw_source_or_reserve_its_old_virtual_address() {
    let source=words(&[0xe320f003]);let other=words(&[0]);
    let cpu=test_support::library().open(Model::CortexA7,1024*1024).unwrap();
    unsafe{
        cpu.map_frozen_backing(ROM,4096,5,pointer(&source)).unwrap();
        cpu.unmap(ROM,4096).unwrap();
        assert!(cpu.map_backing(ROM,4096,7,pointer(&source)).is_err());
        cpu.map_backing(ROM,4096,7,pointer(&other)).unwrap();
        cpu.write_memory(ROM,&[7]).unwrap();
        cpu.unmap(ROM,4096).unwrap();
        cpu.map_frozen_backing(ROM,4096,5,pointer(&source)).unwrap();
        cpu.protect(ROM,4096,1).unwrap();
        assert!(cpu.protect(ROM,4096,7).is_err());
        let mut word=[0;4];cpu.read_memory(ROM,&mut word).unwrap();assert_eq!(u32::from_le_bytes(word),0xe320f003);
        cpu.replace_physical(ROM,ROM,4096,7,pointer(&other)).unwrap();
        cpu.write_memory(ROM,&[8]).unwrap();
        assert!(cpu.map_backing(ROM+4096,4096,7,pointer(&source)).is_err());
    }
}
#[test]
fn guest_store_to_frozen_physical_bytes_refuses_without_changing_source() {
    let source=words(&[0xe320f003]);
    let cpu=test_support::library().open(Model::CortexA7,1024*1024).unwrap();
    unsafe{cpu.map_frozen_backing(ROM,4096,5,pointer(&source)).unwrap();}
    cpu.map_zeroed(0x10000,4096,7).unwrap();
    cpu.write_memory(0x10000,&[0xe5810000u32,0xe320f003].into_iter().flat_map(u32::to_le_bytes).collect::<Vec<_>>()).unwrap();
    cpu.write_register(arm::CORE[0],&7u32.to_le_bytes()).unwrap();
    cpu.write_register(arm::CORE[1],&(ROM as u32).to_le_bytes()).unwrap();
    assert!(cpu.run(Run{begin:0x10000,until:None,watchdog_us:0,instructions:Some(8)}).is_err());
    let mut word=[0;4];cpu.read_memory(ROM,&mut word).unwrap();assert_eq!(u32::from_le_bytes(word),0xe320f003);
}

fn section_mmu(cpu:&Core) {
    let table:Vec<u8>=(0..4096u32).flat_map(|n| {
        let descriptor:u32=match n {0=>0xc02,0xf00=>0xf0000c02,_=>0};descriptor.to_le_bytes()
    }).collect();
    cpu.write_memory(0x4000,&table).unwrap();
    let cp=|crn|arm::Coprocessor{cp:15,opc1:0,crn,crm:0,opc2:0,secure:0,wide:false};
    cpu.coprocessor(cp(2),Some(0x4000)).unwrap();
    cpu.coprocessor(cp(3),Some(0xffffffff)).unwrap();
    let control=cpu.coprocessor(cp(1),None).unwrap();
    cpu.coprocessor(cp(1),Some(control|1)).unwrap();
}
#[test]
fn partial_unmap_protect_and_replace_reprotect_every_retained_frozen_fragment() {
    for operation in 0..3 { for page in 0..3 {
        if operation!=1 && page==1 {continue;}
        let initial=vec![0x61;3*4096];let source=Backing::from_bytes(initial.clone().into_boxed_slice());
        let replacement=Backing::zeros(4096);
        let cpu=test_support::library().open(Model::CortexA7,1024*1024).unwrap();
        cpu.map_zeroed(0,0x20000,7).unwrap();
        unsafe{cpu.map_frozen_backing(ROM,3*4096,5,pointer(&source)).unwrap();}
        match operation {
            0=>{cpu.unmap(ROM+4096,4096).unwrap();cpu.map_zeroed(ROM+4096,4096,7).unwrap();}
            1=>cpu.protect(ROM+4096,4096,1).unwrap(),
            _=>unsafe{cpu.replace_physical(ROM+4096,ROM+4096,4096,7,pointer(&replacement)).unwrap();}
        }
        cpu.write_memory(0x10000,&[0xe5810000u32,0xe320f003].into_iter().flat_map(u32::to_le_bytes).collect::<Vec<_>>()).unwrap();
        cpu.write_register(arm::CORE[0],&7u32.to_le_bytes()).unwrap();
        cpu.write_register(arm::CORE[1],&((ROM+page*4096)as u32).to_le_bytes()).unwrap();
        section_mmu(&cpu);
        assert!(cpu.run(Run{begin:0x10000,until:None,watchdog_us:0,instructions:Some(8)}).is_err(),"operation{operation} page{page}");
        let actual=unsafe{std::slice::from_raw_parts(source.as_ptr(),source.len())};
        assert_eq!(actual,initial,"operation{operation} page{page}");
    }}
}
#[test]
fn partial_replacement_and_unmap_leave_new_mutable_views_writable_under_mmu() {
    for replace in [false,true] {
        let source=Backing::from_bytes(vec![0x61;3*4096].into_boxed_slice());let replacement=Backing::zeros(4096);
        let cpu=test_support::library().open(Model::CortexA7,1024*1024).unwrap();
        cpu.map_zeroed(0,0x20000,7).unwrap();
        unsafe{cpu.map_frozen_backing(ROM,3*4096,5,pointer(&source)).unwrap();}
        if replace {unsafe{cpu.replace_physical(ROM+4096,ROM+4096,4096,7,pointer(&replacement)).unwrap();}}
        else {cpu.unmap(ROM+4096,4096).unwrap();cpu.map_zeroed(ROM+4096,4096,7).unwrap();}
        cpu.write_memory(0x10000,&[0xe5810000u32,0xe320f003].into_iter().flat_map(u32::to_le_bytes).collect::<Vec<_>>()).unwrap();
        cpu.write_register(arm::CORE[0],&7u32.to_le_bytes()).unwrap();
        cpu.write_register(arm::CORE[1],&((ROM+4096)as u32).to_le_bytes()).unwrap();
        section_mmu(&cpu);
        cpu.run(Run{begin:0x10000,until:None,watchdog_us:0,instructions:Some(8)}).unwrap();
        let mut result=[0;4];cpu.read_memory(ROM+4096,&mut result).unwrap();assert_eq!(u32::from_le_bytes(result),7);
        assert!(unsafe{std::slice::from_raw_parts(source.as_ptr(),source.len())}.iter().all(|v|*v==0x61));
    }
}
#[test]
fn unique_identity_cleanup_cannot_erase_another_engine_generation_policy() {
    let first=Backing::zeros(4096);let second=Backing::zeros(4096);
    let old=Identity::allocate();let new=Identity::allocate();assert_ne!(old,new);
    // Both generations may have the same guest/native addresses; only unique
    // identities participate in registry ownership and teardown.
    frozen::reserve(old,ROM,first.as_ptr()as usize,4096,true).unwrap().commit();
    frozen::reserve(new,ROM,second.as_ptr()as usize,4096,true).unwrap().commit();
    frozen::drop_core(old);
    assert!(frozen::check(new,ROM,4).is_err());
    frozen::drop_core(old); // A delayed duplicate old cleanup is also harmless.
    assert!(frozen::check(new,ROM,4).is_err());
    frozen::drop_core(new);assert!(frozen::check(new,ROM,4).is_ok());
}
#[test]
fn failed_fragment_operation_poison_prevents_further_execution_or_mutation() {
    let source=words(&[0xe320f003]);
    let cpu=test_support::library().open(Model::CortexA7,1024*1024).unwrap();
    unsafe{cpu.map_frozen_backing(ROM,4096,5,pointer(&source)).unwrap();}
    assert!(cpu.protect(ROM+1,4096,1).is_err());
    assert!(cpu.write_memory(ROM+0x10000,&[1]).unwrap_err().contains("destroy CPU"));
    assert!(cpu.run(Run{begin:ROM,until:None,watchdog_us:0,instructions:Some(1)}).is_err());
    let mut word=[0;4];cpu.read_memory(ROM,&mut word).unwrap();assert_eq!(u32::from_le_bytes(word),0xe320f003);
}

#[test]
fn injected_initial_protection_and_rollback_failure_poison_live_mapping() {
    let source=words(&[0xe320f003]);
    let cpu=test_support::library().open(Model::CortexA7,1024*1024).unwrap();
    let reservation=frozen::reserve(cpu.identity,ROM,source.as_ptr()as usize,4096,true).unwrap();
    // Simulate the exact state before initial protection: a live pointer map
    // whose native backing is still writable. Failure injection targets the
    // production outcome handler rather than altering the native library.
    cpu.check(unsafe{(cpu.api.uc_mem_map_ptr)(cpu.raw,ROM,4096,7,source.as_ptr().cast())}).unwrap();
    let failure=cpu.finish_backing_outcome(reservation,Err("injected protection failure".into()),||Err("injected rollback failure".into())).unwrap_err();
    assert!(failure.contains("destroy CPU"));
    assert!(cpu.run(Run{begin:ROM,until:None,watchdog_us:0,instructions:Some(1)}).is_err());
    assert!(cpu.write_memory(ROM+4096,&[1]).unwrap_err().contains("injected rollback"));
    assert!(cpu.unmap(ROM,4096).is_err());
    let peer=test_support::library().open(Model::CortexA7,1024*1024).unwrap();
    assert!(unsafe{peer.map_backing(ROM,4096,7,pointer(&source))}.is_err());
    let mut word=[0;4];cpu.read_memory(ROM,&mut word).unwrap();assert_eq!(u32::from_le_bytes(word),0xe320f003);
    drop(cpu);
    unsafe{peer.map_backing(ROM,4096,7,pointer(&source)).unwrap();}
}
#[test]
fn injected_initial_protection_failure_with_successful_rollback_releases_reservation() {
    let source=words(&[0xe320f003]);
    let cpu=test_support::library().open(Model::CortexA7,1024*1024).unwrap();
    let reservation=frozen::reserve(cpu.identity,ROM,source.as_ptr()as usize,4096,true).unwrap();
    cpu.check(unsafe{(cpu.api.uc_mem_map_ptr)(cpu.raw,ROM,4096,5,source.as_ptr().cast())}).unwrap();
    assert!(cpu.finish_backing_outcome(reservation,Err("injected protection failure".into()),|| {
        cpu.check(unsafe{(cpu.api.uc_mem_unmap)(cpu.raw,ROM,4096)})
    }).is_err());
    assert!(frozen::ready(cpu.identity).is_ok());assert!(!frozen::any(cpu.identity));
    unsafe{cpu.map_backing(ROM,4096,7,pointer(&source)).unwrap();}
    cpu.write_memory(ROM,&[1]).unwrap();
}
