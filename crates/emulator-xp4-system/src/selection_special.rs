//! Explicit functional retention for source-valid XP4 selection/special words.
//! No clock routing, reset signal, readiness or event effects are inferred.
use fujimod_emulator_devices::{Cx, Device, Record, Value as Field};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const SELECTION: u64 = 0xfff48600;
pub const SPECIAL52: u64 = 0xfff4860c;
pub const SPECIAL53: u64 = 0xfff48604;
pub const COMBINED_MASK: u32 = 0x01070ff7;
pub const MASKS: [u32; 11] = [0x1000000, 0x40000, 0x20000, 0x10000, 0xc00, 0x300, 0xc0, 0x30, 0x4, 0x2, 0x1];
pub const ENCODINGS: [u32; 26] = [0x0, 0x1000000, 0x0, 0x40000, 0x0, 0x20000, 0x0, 0x10000, 0x0, 0x400, 0x800, 0x0, 0x100, 0x200, 0x0, 0x40, 0x80, 0x0, 0x10, 0x20, 0x0, 0x4, 0x0, 0x2, 0x0, 0x1];
pub const ELIGIBLE: [&[usize]; 11] = [
    &[0, 1],
    &[2, 3],
    &[4, 5],
    &[6, 7],
    &[8, 9, 10],
    &[11, 12, 13],
    &[14, 15, 16],
    &[17, 18, 19],
    &[20, 21],
    &[22, 23],
    &[24, 25],
];

fn valid_selection(value: u32) -> bool {
    value & !COMBINED_MASK == 0 && MASKS.iter().enumerate().all(|(field, mask)|
        ELIGIBLE[field].iter().any(|&choice| ENCODINGS[choice] == value & mask))
}
fn valid_special(value: u32, minimum: u32, maximum: u32) -> bool {
    value & !0x1ff == 0 && (minimum..=maximum).contains(&(value & 0xff))
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Inputs {
    pub selection_reset: u32,
    pub special52_reset: u32,
    pub special53_reset: u32,
}
impl Inputs {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if !valid_selection(self.selection_reset)
            || !valid_special(self.special52_reset, 2, 0xff)
            || !valid_special(self.special53_reset, 0, 0x10)
        {
            return Err("Invalid explicit XP4 selection/special reset inputs".into());
        }
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectionSpecial {
    inputs: Inputs,
    selection: u32,
    special52: u32,
    special53: u32,
    admitted: bool,
}
impl SelectionSpecial {
    pub fn new(inputs: Inputs) -> Result<Self, String> {
        inputs.validate()?;
        Ok(Self { selection: inputs.selection_reset, special52: inputs.special52_reset,
            special53: inputs.special53_reset, inputs, admitted: false })
    }
    pub fn windows(&self) -> Vec<(u64, u64)> {
        [SELECTION, SPECIAL52, SPECIAL53].map(|address| (address, address + 4)).into()
    }
    pub(crate) fn owns(&self, address: u64, size: u8) -> bool {
        size == 4 && matches!(address, SELECTION | SPECIAL52 | SPECIAL53)
    }
    fn admit(&mut self, cx: &mut Cx<'_>) {
        if self.admitted { return; }
        cx.record(Record::new("XP4 experimental selection/special retention; no physical routing or ready effects", [
            ("selection_reset", Field::int(self.inputs.selection_reset)),
            ("special52_reset", Field::int(self.inputs.special52_reset)),
            ("special53_reset", Field::int(self.inputs.special53_reset)),
            ("law", Field::constant("Explicit source-valid field domains and enable-bit retention; not measured reset state")),
        ]));
        self.admitted = true;
    }
    pub(crate) fn validate_state(&self, expected: &Inputs) -> Result<(), String> {
        self.inputs.validate()?;
        if &self.inputs != expected || !valid_selection(self.selection)
            || !valid_special(self.special52, 2, 0xff) || !valid_special(self.special53, 0, 0x10)
        {
            return Err("Invalid XP4 selection/special checkpoint or changed inputs".into());
        }
        Ok(())
    }
}
impl Device for SelectionSpecial {
    fn name(&self) -> &'static str { "xp4-functional-selection-special" }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        if !self.owns(address, size) { cx.unmodeled(); return 0; }
        self.admit(cx);
        u64::from(match address { SELECTION => self.selection, SPECIAL52 => self.special52, _ => self.special53 })
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        if !self.owns(address, size) { cx.unmodeled(); return; }
        if value > u32::MAX as u64 { cx.fail("XP4 selection/special requires a 32-bit value"); return; }
        let value = value as u32;
        let valid = match address {
            SELECTION => valid_selection(value), SPECIAL52 => valid_special(value, 2, 0xff),
            _ => valid_special(value, 0, 0x10),
        };
        if !valid { cx.fail("XP4 selection/special value outside source-valid functional domain"); return; }
        self.admit(cx);
        match address { SELECTION => self.selection = value, SPECIAL52 => self.special52 = value, _ => self.special53 = value }
        cx.record(Record::new("XP4 experimental selection/special retained write", [
            ("address", Field::int(address)), ("value", Field::int(value)),
        ]));
    }
    fn save(&self) -> Value { json!(self) }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: Self = serde_json::from_value(value.clone()).map_err(|error| error.to_string())?;
        state.validate_state(&self.inputs)?; *self = state; Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::Host;
    #[derive(Default)]
    struct Context { records: Vec<Record> }
    impl Host for Context {
        fn now(&self) -> u64 { 0 }
        fn schedule(&mut self, _: usize, _: u64, _: u64) { panic!("no events") }
        fn set_irq(&mut self, _: u32, _: bool) { panic!("no IRQ") }
        fn raise_irq(&mut self, _: usize, _: u32) -> Result<(), String> { panic!("no IRQ") }
        fn ram_read(&self, _: u64, _: &mut [u8]) -> Result<(), String> { panic!("no DMA") }
        fn ram_write(&mut self, _: u64, _: &[u8]) -> Result<(), String> { panic!("no DMA") }
        fn record(&mut self, record: Record) { self.records.push(record); }
    }
    fn inputs() -> Inputs { Inputs { selection_reset: 0, special52_reset: 2, special53_reset: 0 } }
    fn write(model: &mut SelectionSpecial, cx: &mut Context, address: u64, size: u8, value: u64) -> Result<(), String> {
        let mut cx = Cx::new(cx, 0, 0, 0); model.write(&mut cx, address, size, value);
        cx.take_refusal().map_or(Ok(()), Err)
    }
    fn read(model: &mut SelectionSpecial, cx: &mut Context, address: u64, size: u8) -> Result<u64, String> {
        let mut cx = Cx::new(cx, 0, 0, 0); let value = model.read(&mut cx, address, size);
        cx.take_refusal().map_or(Ok(value), Err)
    }
    #[test]
    fn all_twenty_six_choices_preserve_other_fields_for_each_valid_reset_tuple() {
        for (selection, special52, special53) in [(0, 2, 0), (0x1070aa7, 0xff, 0x10), (0x50222, 0x155, 0x105)] {
            let mut model = SelectionSpecial::new(Inputs { selection_reset: selection, special52_reset: special52, special53_reset: special53 }).unwrap();
            let mut cx = Context::default(); let mut count = 0;
            assert_eq!(read(&mut model, &mut cx, SELECTION, 4).unwrap(), u64::from(selection));
            for (field, mask) in MASKS.iter().enumerate() {
                for &choice in ELIGIBLE[field] {
                    let before = model.selection;
                    let value = (before & !mask) | ENCODINGS[choice];
                    write(&mut model, &mut cx, SELECTION, 4, u64::from(value)).unwrap();
                    assert_eq!(read(&mut model, &mut cx, SELECTION, 4).unwrap(), u64::from(value));
                    assert_eq!(model.selection & !mask, before & !mask); count += 1;
                }
            }
            assert_eq!(count, 26); assert_eq!(model.next_deadline(0), None);
            assert_eq!((model.special52, model.special53), (special52, special53));
        }
    }
    #[test]
    fn full_mask_and_each_invalid_two_bit_encoding_refuse_without_mutation() {
        let mut model = SelectionSpecial::new(inputs()).unwrap(); let mut cx = Context::default();
        read(&mut model, &mut cx, SELECTION, 4).unwrap(); let before = model.save();
        for mask in [0xc00, 0x300, 0xc0, 0x30, COMBINED_MASK, 8, 0x80000000] {
            assert!(write(&mut model, &mut cx, SELECTION, 4, u64::from(mask)).is_err());
            assert_eq!(model.save(), before);
            let mut bad = inputs(); bad.selection_reset = mask; assert!(SelectionSpecial::new(bad).is_err());
        }
    }
    #[test]
    fn special_enable_and_low_byte_operations_are_closed_over_every_direct_setter_value() {
        let mut model = SelectionSpecial::new(inputs()).unwrap(); let mut cx = Context::default();
        for (address, minimum, maximum) in [(SPECIAL52, 2, 0xff), (SPECIAL53, 0, 0x10)] {
            for low in minimum..=maximum {
                write(&mut model, &mut cx, address, 4, low).unwrap();
                write(&mut model, &mut cx, address, 4, low | 0x100).unwrap();
                assert_eq!(read(&mut model, &mut cx, address, 4).unwrap(), low | 0x100);
                write(&mut model, &mut cx, address, 4, (low | 0x100) & 0xff).unwrap();
                assert_eq!(read(&mut model, &mut cx, address, 4).unwrap(), low);
            }
        }
        write(&mut model, &mut cx, SPECIAL52, 4, 0x102).unwrap();
        write(&mut model, &mut cx, SPECIAL53, 4, 0x110).unwrap();
    }
    #[test]
    fn exact_three_windows_refuse_b14_widths_gaps_and_out_of_domain_special_values() {
        let mut model = SelectionSpecial::new(inputs()).unwrap(); let mut cx = Context::default();
        let mut windows = model.windows(); windows.sort_unstable();
        assert_eq!(windows, [(SELECTION, SELECTION + 4), (SPECIAL53, SPECIAL53 + 4), (SPECIAL52, SPECIAL52 + 4)]);
        read(&mut model, &mut cx, SELECTION, 4).unwrap(); let before = model.save();
        for address in [SELECTION, SPECIAL52, SPECIAL53] {
            for size in [1, 2, 8] {
                assert!(read(&mut model, &mut cx, address, size).is_err());
                assert!(write(&mut model, &mut cx, address, size, 0).is_err());
            }
            assert!(write(&mut model, &mut cx, address, 4, 1u64 << 32).is_err());
        }
        for address in [0xfff485fc, 0xfff48601, 0xfff48608, 0xfff48610] {
            assert!(read(&mut model, &mut cx, address, 4).is_err());
            assert!(write(&mut model, &mut cx, address, 4, 0).is_err());
        }
        for (address, value) in [(SPECIAL52, 0), (SPECIAL52, 1), (SPECIAL52, 0x101), (SPECIAL52, 0x200),
            (SPECIAL53, 0x11), (SPECIAL53, 0x111), (SPECIAL53, 0x200)] {
            assert!(write(&mut model, &mut cx, address, 4, value).is_err());
        }
        assert_eq!(model.save(), before);
    }
    #[test]
    fn required_inputs_and_checkpoint_domains_are_validated_before_restore() {
        for value in [json!({}), json!({"selection_reset":0,"special52_reset":2}),
            json!({"selection_reset":0,"special52_reset":2,"special53_reset":0,"extra":1})] {
            assert!(serde_json::from_value::<Inputs>(value).is_err());
        }
        let mut model = SelectionSpecial::new(inputs()).unwrap(); let mut cx = Context::default();
        write(&mut model, &mut cx, SELECTION, 4, 0x50222).unwrap();
        write(&mut model, &mut cx, SPECIAL52, 4, 0x102).unwrap();
        let good = model.save(); let mut resumed = SelectionSpecial::new(inputs()).unwrap();
        resumed.restore(&good).unwrap(); assert_eq!(resumed.save(), good);
        for (field, value) in [("selection", 0xc00), ("special52", 1), ("special53", 0x111)] {
            let mut bad = good.clone(); bad[field] = json!(value);
            assert!(resumed.restore(&bad).is_err()); assert_eq!(resumed.save(), good);
        }
        let mut changed = inputs(); changed.selection_reset = 1;
        assert!(SelectionSpecial::new(changed).unwrap().restore(&good).is_err());
        for (selection, a, b) in [(COMBINED_MASK, 2, 0), (0, 1, 0), (0, 2, 17)] {
            assert!(SelectionSpecial::new(Inputs { selection_reset: selection, special52_reset: a, special53_reset: b }).is_err());
        }
    }
    #[test]
    fn register_trace_and_records_are_cpu_pc_independent_without_events() {
        let mut outputs = Vec::new();
        for (cpu, pc) in [(0, 0), (3, 0xdead0000)] {
            let mut model = SelectionSpecial::new(inputs()).unwrap(); let mut cx = Context::default();
            for (address, value) in [(SELECTION, 0x1070aa7), (SPECIAL52, 0x102), (SPECIAL53, 0x110)] {
                let mut context = Cx::new(&mut cx, cpu, pc, 0);
                model.write(&mut context, address, 4, value); assert!(context.take_refusal().is_none());
                assert_eq!(model.read(&mut context, address, 4), value);
            }
            assert_eq!(model.next_deadline(u64::MAX), None);
            outputs.push((model.save(), json!(cx.records)));
        }
        assert_eq!(outputs[0], outputs[1]);
    }
}
