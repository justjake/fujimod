use super::*;
use fujimod_emulator_devices::test_host::TestHost;

const BASE: u64 = 0xff924000;
const BYTE_NS: u64 = 1000;

/// One register-pointer target at 0x1c: the first written byte selects.
#[derive(Default)]
struct File {
    registers: [u8; 4],
    pointer: Option<u8>,
    log: Vec<String>,
}
impl Bus for File {
    fn start(&mut self, address: u16, read: bool) -> bool {
        self.log.push(format!("start {address:#x} {read}"));
        if !read {
            self.pointer = None;
        }
        address == 0x1c
    }
    fn write(&mut self, byte: u8) -> Result<(), String> {
        match self.pointer {
            None => self.pointer = Some(byte),
            Some(p) => {
                *self.registers.get_mut(p as usize).ok_or("undeclared")? = byte;
                self.pointer = Some(p + 1);
            }
        }
        Ok(())
    }
    fn read(&mut self) -> Result<u8, String> {
        let p = self.pointer.unwrap_or(0);
        self.pointer = Some(p + 1);
        self.registers
            .get(p as usize)
            .copied()
            .ok_or_else(|| "undeclared".into())
    }
    fn stop(&mut self) {
        self.log.push("stop".into());
    }
    fn save(&self) -> Value {
        serde_json::to_value(self.registers).unwrap()
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        self.registers = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        Ok(())
    }
}

fn controller() -> Controller<File> {
    Controller::new(
        Options {
            base: BASE,
            irq: 290,
            byte_ns: BYTE_NS,
        },
        File::default(),
    )
    .unwrap()
}
fn put(c: &mut Controller<File>, h: &mut TestHost, offset: u64, value: u8) {
    let mut cx = h.cx();
    c.write(&mut cx, BASE + offset, 1, value.into());
    assert_eq!(cx.refusal(), None, "{offset:#x}={value:#x}");
}
fn get(c: &mut Controller<File>, h: &mut TestHost, offset: u64) -> u8 {
    let mut cx = h.cx();
    let value = c.read(&mut cx, BASE + offset, 1) as u8;
    assert_eq!(cx.refusal(), None, "{offset:#x}");
    value
}
fn refused(c: &mut Controller<File>, h: &mut TestHost, offset: u64, value: u8) -> String {
    let mut cx = h.cx();
    c.write(&mut cx, BASE + offset, 1, value.into());
    cx.refusal().expect("refused").to_owned()
}
/// The byte in flight completes.
fn complete(c: &mut Controller<File>, h: &mut TestHost) {
    h.now += BYTE_NS;
    let token = c.state.generation;
    let mut cx = h.cx();
    c.event(&mut cx, token);
    assert_eq!(cx.refusal(), None);
}
/// The driver's open: clock enabled, master start of `address` (8-bit form)
/// with both interrupt enables.
fn start(c: &mut Controller<File>, h: &mut TestHost, address: u8) {
    put(c, h, CCR, 0x2c);
    assert_eq!(get(c, h, BSR) & BSR_BB, 0);
    put(c, h, DAR, address);
    put(c, h, BCR, 0x52);
    assert_eq!(get(c, h, BSR) & BSR_BB, BSR_BB);
    complete(c, h);
}

#[test]
fn a_write_then_read_moves_bytes_and_reports_each_byte_status() {
    let mut h = TestHost::new();
    let mut c = controller();
    start(&mut c, &mut h, 0x1c << 1);
    // Address acknowledged, transmitting, first byte, interrupt pending.
    assert_eq!(get(&mut c, &mut h, BSR), BSR_BB | BSR_TRX | BSR_FBT);
    assert_eq!(get(&mut c, &mut h, BCR), 0x53);
    for byte in [1, 0xab] {
        put(&mut c, &mut h, DAR, byte);
        put(&mut c, &mut h, BCR, 0x52);
        assert_eq!(get(&mut c, &mut h, BCR) & BCR_INT, 0);
        complete(&mut c, &mut h);
        assert_eq!(get(&mut c, &mut h, BSR), BSR_BB | BSR_TRX);
    }
    assert_eq!(c.bus().registers, [0, 0xab, 0, 0]);
    put(&mut c, &mut h, BCR, 0x42);
    // Select register 1 again, then a repeated START for a read: one byte
    // with ACK and one without.
    start(&mut c, &mut h, 0x1c << 1);
    put(&mut c, &mut h, DAR, 1);
    put(&mut c, &mut h, BCR, 0x52);
    complete(&mut c, &mut h);
    put(&mut c, &mut h, DAR, 0x1c << 1 | 1);
    put(&mut c, &mut h, BCR, 0x72);
    complete(&mut c, &mut h);
    assert_eq!(get(&mut c, &mut h, BSR), BSR_BB | BSR_FBT);
    put(&mut c, &mut h, BCR, 0x5a);
    complete(&mut c, &mut h);
    assert_eq!(get(&mut c, &mut h, BSR), BSR_BB);
    assert_eq!(get(&mut c, &mut h, DAR), 0xab);
    put(&mut c, &mut h, BCR, 0x52);
    complete(&mut c, &mut h);
    assert_eq!(get(&mut c, &mut h, BSR), BSR_BB | BSR_LRB);
    assert_eq!(get(&mut c, &mut h, DAR), 0);
    // Clearing MSS stops at once; the bus is free.
    put(&mut c, &mut h, BCR, 0x42);
    assert_eq!(get(&mut c, &mut h, BSR), 0);
    assert_eq!(get(&mut c, &mut h, BCR), 0x42);
    assert_eq!(
        c.bus().log,
        ["start 0x1c false", "stop", "start 0x1c false", "start 0x1c true", "stop"]
    );
}

#[test]
fn an_absent_address_is_not_acknowledged_and_the_error_stop_is_accepted() {
    let mut h = TestHost::new();
    let mut c = controller();
    start(&mut c, &mut h, 0x6a << 1);
    assert_eq!(get(&mut c, &mut h, BSR), BSR_BB | BSR_TRX | BSR_FBT | BSR_LRB);
    // Data after a NACK is refused; the driver's STOP, unchanged clock rewrite
    // and second STOP are accepted.
    put(&mut c, &mut h, DAR, 0x12);
    assert!(refused(&mut c, &mut h, BCR, 0x52).contains("phase"));
    put(&mut c, &mut h, BCR, 0x40);
    put(&mut c, &mut h, CCR, 0x2c);
    put(&mut c, &mut h, BCR, 0x40);
    assert_eq!(get(&mut c, &mut h, BSR), 0);
    assert_eq!(c.bus().log, ["start 0x6a false", "stop"]);
}

#[test]
fn unmodeled_commands_refuse() {
    let mut h = TestHost::new();
    let mut c = controller();
    // START needs the clock enable and a staged address.
    put(&mut c, &mut h, DAR, 0x38);
    assert!(refused(&mut c, &mut h, BCR, 0x52).contains("disabled"));
    put(&mut c, &mut h, CCR, 0x20);
    put(&mut c, &mut h, BCR, 0x52);
    // Nothing is accepted while the address byte is in flight.
    assert!(refused(&mut c, &mut h, BCR, 0x40).contains("in flight"));
    assert!(refused(&mut c, &mut h, DAR, 1).contains("in flight"));
    assert!(refused(&mut c, &mut h, CCR, 0x2c).contains("changed"));
    complete(&mut c, &mut h);
    // A byte command without a newly staged byte, and status bits, refuse.
    assert!(refused(&mut c, &mut h, BCR, 0x52).contains("phase"));
    assert!(refused(&mut c, &mut h, BCR, 0x53).contains("status"));
    assert!(refused(&mut c, &mut h, BCR, 0x56).contains("general-call"));
    // The slave address register and word accesses are not modeled.
    let mut cx = h.cx();
    c.read(&mut cx, BASE + ADR, 1);
    assert!(cx.is_unmodeled());
    let mut cx = h.cx();
    c.read(&mut cx, BASE + BSR, 4);
    assert!(cx.is_unmodeled());
}

#[test]
fn receive_data_is_read_once_per_byte() {
    let mut h = TestHost::new();
    let mut c = controller();
    start(&mut c, &mut h, 0x1c << 1 | 1);
    let mut cx = h.cx();
    c.read(&mut cx, BASE + DAR, 1);
    assert!(cx.refusal().unwrap().contains("before a byte"));
    put(&mut c, &mut h, BCR, 0x52);
    complete(&mut c, &mut h);
    get(&mut c, &mut h, DAR);
    let mut cx = h.cx();
    c.read(&mut cx, BASE + DAR, 1);
    assert!(cx.refusal().is_some());
}

#[test]
fn idle_line_recovery_drives_and_reads_the_lines() {
    let mut h = TestHost::new();
    let mut c = controller();
    assert_eq!(get(&mut c, &mut h, BC2R), BC2R_SCLS | BC2R_SDAS);
    put(&mut c, &mut h, BC2R, BC2R_SCLL);
    assert_eq!(get(&mut c, &mut h, BC2R), BC2R_SCLL | BC2R_SDAS);
    put(&mut c, &mut h, BC2R, BC2R_SDAL);
    assert_eq!(get(&mut c, &mut h, BC2R), BC2R_SDAL | BC2R_SCLS);
    assert!(refused(&mut c, &mut h, BC2R, 4).contains("recovery"));
}

#[test]
fn checkpoints_restore_the_controller_and_its_bus() {
    let mut h = TestHost::new();
    let mut c = controller();
    start(&mut c, &mut h, 0x1c << 1);
    put(&mut c, &mut h, DAR, 2);
    put(&mut c, &mut h, BCR, 0x52);
    complete(&mut c, &mut h);
    put(&mut c, &mut h, DAR, 0x5a);
    put(&mut c, &mut h, BCR, 0x52);
    complete(&mut c, &mut h);
    let saved = c.save();
    put(&mut c, &mut h, BCR, 0x42);
    c.restore(&saved).unwrap();
    assert_eq!(get(&mut c, &mut h, BSR), BSR_BB | BSR_TRX);
    assert_eq!(c.bus().registers[2], 0x5a);
    let mut broken = saved.clone();
    broken["controller"]["bsr"] = 0.into();
    assert!(c.restore(&broken).is_err());
}
