//! Destroy the source VM and rebind a whole-board archive to fresh vCPUs.
use fujimod_emulator::board::{
    Board, InterruptMap,
    bus::{Bus, Mode},
    factory::*,
};
use fujimod_emulator_cpu::{Model, Register, timeline::Ratio};
use fujimod_emulator_devices::checkpoint::Checkpoint;
use fujimod_emulator_ip_arm::gicv3::Gic;
fn board() -> Result<Board, String> {
    Graph {
        processors: vec![Processor {
            model: Model::ArmV8A,
            released: true,
            registers: vec![
                (Register::Pc, 0x10000),
                (Register::Pstate, 0x3c5),
                (Register::X(1), 0x11000),
            ],
            mprofile: None,
            rprofile: None,
        }],
        ram: vec![Ram {
            base: 0x10000,
            size: 0x4000,
        }],
        mappings: vec![MemoryMap {
            core: 0,
            address: 0x10000,
            physical: 0x10000,
            size: 0x4000,
            permissions: 7,
        }],
        peripherals: vec![],
        dma_mappings: vec![],
        decoders: vec![],
        instructions_per_tick: Ratio::new(7, 3)?,
    }
    .build(
        Options::new(Engine::Hypervisor).with_translation_cache_size_bytes(1 << 20),
        Bus::new(Mode::Strict),
        Some(InterruptMap {
            cores: 1,
            gic: Box::new(Gic::new(vec![0], 128)?),
            registers: fujimod_emulator::board::InterruptRegisters::GicV3 {
                distributor: 0x8000000,
                redistributor: 0x80a0000,
                stride: 0x20000,
            },
        }),
    )
}
pub fn run() -> Result<(), String> {
    let (bytes, later) = {
        let mut source = board()?;
        // add x0,x0,#1; str w0,[x1]; b loop
        let code = [0x91000400u32, 0xb9000020, 0x17fffffe]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>();
        source.write_memory(0, 0x10000, &code)?;
        source.cpu(0)?.write_register(Register::V(9), &[0xa7; 16])?;
        source.watch_pcs(&[0x10000])?;
        source.run_until(13, 17)?;
        let bytes = source.checkpoint(None)?.export()?.to_bytes()?;
        source.run_until(120, 17)?;
        let later = source.checkpoint(None)?.export()?.to_bytes()?;
        (bytes, later)
    };
    let mut target = board()?;
    target.watch_pcs(&[0x10000])?;
    let empty = target.checkpoint(None)?.export()?.to_bytes()?;
    let imported = target.import_keyframe(&Checkpoint::from_bytes(&bytes)?)?;
    assert_eq!(
        target.checkpoint(None)?.export()?.to_bytes()?,
        empty,
        "import mutated the VM"
    );
    target.restore_keyframe(&imported)?;
    assert_eq!(
        target.checkpoint(None)?.export()?.to_bytes()?,
        bytes,
        "fresh HVF restore differs"
    );
    target.run_until(120, 17)?;
    assert_eq!(
        target.checkpoint(None)?.export()?.to_bytes()?,
        later,
        "fresh HVF continuation differs"
    );
    println!(
        "PASS: durable board archive restored into a fresh HVF VM and continued through tick 120"
    );
    Ok(())
}
