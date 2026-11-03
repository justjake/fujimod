//! Source-bound opaque parameter pairs and two declared settling phases.
//! One parent configuration/request device retains all shared request ownership.
use fujimod_emulator_devices::{Cx, Record, Value as Field};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const BASES: [u64; 13] = [
    0xff701000,
    0xff702000,
    0xff703000,
    0xff704000,
    0xff707000,
    0xff708000,
    0xff70c000,
    0xff70a000,
    0xff70b000,
    0xff706000,
    0xff709000,
    0xff705000,
    0xfffae000,
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Inputs {
    pub initial_pairs: Vec<[u32; 2]>,
    pub initial_status: Vec<u32>,
    pub request_ticks: Vec<u64>,
    pub settle_ticks: Vec<u64>,
}
impl Inputs {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.initial_pairs.len() != 13 || self.initial_status.len() != 13
            || self.request_ticks.len() != 13 || self.settle_ticks.len() != 13
            || self.initial_status.iter().any(|value| *value > 1)
            || self.request_ticks.iter().chain(self.settle_ticks.iter()).any(|ticks| !(1..=1_000_000).contains(ticks))
        {
            return Err("Invalid explicit XP4 base-parameter inputs".into());
        }
        Ok(())
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum Phase { Request, Settling }
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    request_at: u64,
    settle_at: u64,
    request_token: u64,
    settle_token: u64,
    phase: Phase,
    pair: [u32; 2],
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BaseParameters {
    inputs: Inputs,
    staged: Vec<[u32; 2]>,
    applied: Vec<[u32; 2]>,
    written: Vec<[bool; 2]>,
    status: Vec<u32>,
    settled: Vec<bool>,
    pending: Vec<Option<Pending>>,
    admitted: bool,
}
impl BaseParameters {
    pub(crate) fn new(inputs: Inputs) -> Result<Self, String> {
        inputs.validate()?;
        Ok(Self { staged: inputs.initial_pairs.clone(), applied: inputs.initial_pairs.clone(),
            status: inputs.initial_status.clone(), inputs, written: vec![[false; 2]; 13],
            settled: vec![false; 13], pending: vec![None; 13], admitted: false })
    }
    pub(crate) fn index(base: u64) -> Option<usize> { BASES.iter().position(|b| *b == base) }
    pub(crate) fn address_base(address: u64) -> Option<u64> {
        BASES.iter().find(|&&base| [base, base + 8, base + 0xf0].contains(&address)).copied()
    }
    pub(crate) fn owns(&self, address: u64, size: u8) -> bool { size == 4 && Self::address_base(address).is_some() }
    pub(crate) fn windows(&self) -> Vec<(u64, u64)> {
        BASES.iter().flat_map(|base| [*base, base + 8, base + 0xf0]).map(|address| (address, address + 4)).collect()
    }
    pub(crate) fn busy(&self, base: u64) -> bool { Self::index(base).is_some_and(|i| self.pending[i].is_some()) }
    pub(crate) fn request_value(&self, base: u64) -> u64 {
        Self::index(base).and_then(|i| self.pending[i].as_ref())
            .map_or(0, |p| if p.phase == Phase::Request { 3 } else { 0 })
    }
    fn admit(&mut self, cx: &mut Cx<'_>) {
        if self.admitted { return; }
        cx.record(Record::new("XP4 experimental base parameters; two declared phases, no physical clock effects", [
            ("law", Field::constant("Opaque u32 pairs; command 3 acknowledgment then bit-0 settling; unmeasured timing")),
        ]));
        for (index, &base) in BASES.iter().enumerate() {
            cx.record(Record::new("XP4 experimental base-parameter inputs", [
                ("base", Field::int(base)), ("initial_first", Field::int(self.inputs.initial_pairs[index][0])),
                ("initial_second", Field::int(self.inputs.initial_pairs[index][1])),
                ("initial_status", Field::int(self.inputs.initial_status[index])),
                ("request_ticks_unmeasured", Field::int(self.inputs.request_ticks[index])),
                ("settle_ticks_unmeasured", Field::int(self.inputs.settle_ticks[index])),
            ]));
        }
        self.admitted = true;
    }
    pub(crate) fn read(&mut self, cx: &mut Cx<'_>, address: u64) -> u64 {
        let base = Self::address_base(address).unwrap(); let index = Self::index(base).unwrap(); self.admit(cx);
        u64::from(if address == base + 0xf0 { self.status[index] }
            else { self.staged[index][usize::from(address == base + 8)] })
    }
    pub(crate) fn write(&mut self, cx: &mut Cx<'_>, address: u64, value: u64) {
        let base = Self::address_base(address).unwrap(); let index = Self::index(base).unwrap();
        if address == base + 0xf0 || value > u32::MAX as u64 || self.pending[index].is_some() {
            cx.fail("XP4 base-parameter write to read-only status, oversized data or busy group"); return;
        }
        let word = usize::from(address == base + 8);
        self.admit(cx); self.staged[index][word] = value as u32; self.written[index][word] = true;
        cx.record(Record::new("XP4 experimental base parameter staged", [
            ("address", Field::int(address)), ("value", Field::int(value)),
        ]));
    }
    pub(crate) fn deadlines(&self, base: u64, now: u64) -> Result<(u64, u64), String> {
        let index = Self::index(base).ok_or("XP4 base has no parameter pair")?;
        if self.written[index] != [true, true] { return Err("XP4 command 3 requires both parameter writes".into()); }
        let request_at = now.checked_add(self.inputs.request_ticks[index]).ok_or("XP4 parameter request deadline overflow")?;
        let settle_at = request_at.checked_add(self.inputs.settle_ticks[index]).ok_or("XP4 parameter settling deadline overflow")?;
        Ok((request_at, settle_at))
    }
    pub(crate) fn reissue(&self, cx: &mut Cx<'_>, base: u64) {
        let pending = self.pending[Self::index(base).unwrap()].as_ref().unwrap();
        cx.record(Record::new("XP4 experimental command 3 reissue preserves both deadlines", [
            ("request_address", Field::int(base + 0x80)), ("request_deadline", Field::int(pending.request_at)),
            ("settle_deadline", Field::int(pending.settle_at)),
        ]));
    }
    pub(crate) fn submit(&mut self, cx: &mut Cx<'_>, base: u64, deadlines: (u64, u64), tokens: (u64, u64)) {
        let index = Self::index(base).unwrap(); let pair = self.staged[index]; self.admit(cx);
        self.status[index] = 1;
        self.pending[index] = Some(Pending { request_at: deadlines.0, settle_at: deadlines.1,
            request_token: tokens.0, settle_token: tokens.1, phase: Phase::Request, pair });
        cx.record(Record::new("XP4 experimental command 3 scheduled", [
            ("request_address", Field::int(base + 0x80)), ("first", Field::int(pair[0])),
            ("second", Field::int(pair[1])), ("request_deadline", Field::int(deadlines.0)),
            ("settle_deadline", Field::int(deadlines.1)),
        ]));
        cx.schedule(deadlines.0, tokens.0); cx.schedule(deadlines.1, tokens.1);
    }
    pub(crate) fn next_deadline(&self) -> Option<u64> {
        self.pending.iter().flatten().map(|p| if p.phase == Phase::Request { p.request_at } else { p.settle_at }).min()
    }
    pub(crate) fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
        for index in 0..13 {
            let Some(pending) = &mut self.pending[index] else { continue; };
            if pending.phase == Phase::Request && pending.request_token == token && pending.request_at <= cx.now() {
                pending.phase = Phase::Settling;
                cx.record(Record::new("XP4 experimental command 3 acknowledged; settling remains busy", [
                    ("request_address", Field::int(BASES[index] + 0x80)),
                    ("deadline", Field::int(pending.request_at)), ("tick", Field::int(cx.now())),
                ]));
            } else if pending.phase == Phase::Settling && pending.settle_token == token && pending.settle_at <= cx.now() {
                let pending = self.pending[index].take().unwrap();
                self.applied[index] = pending.pair; self.status[index] = 0; self.settled[index] = true;
                cx.record(Record::new("XP4 experimental base parameters settled; status bit cleared", [
                    ("status_address", Field::int(BASES[index] + 0xf0)),
                    ("deadline", Field::int(pending.settle_at)), ("tick", Field::int(cx.now())),
                ]));
            }
        }
    }
    pub(crate) fn tokens(&self) -> Vec<u64> {
        self.pending.iter().flatten().flat_map(|p| [p.request_token, p.settle_token]).collect()
    }
    pub(crate) fn validate_state(&self, expected: &Inputs, generation: u64) -> Result<(), String> {
        self.inputs.validate()?;
        if &self.inputs != expected || self.staged.len() != 13 || self.applied.len() != 13
            || self.written.len() != 13 || self.status.len() != 13 || self.pending.len() != 13 || self.settled.len() != 13
        { return Err("Invalid XP4 parameter checkpoint inputs or lengths".into()); }
        let mut tokens = BTreeSet::new();
        for index in 0..13 {
            if (!self.settled[index] && self.applied[index] != self.inputs.initial_pairs[index])
                || (self.settled[index] && (generation < 2 || self.written[index] != [true, true]))
                || (0..2).any(|word| !self.written[index][word] && self.staged[index][word] != self.inputs.initial_pairs[index][word])
            { return Err("Invalid XP4 parameter checkpoint write history".into()); }
            if let Some(p) = &self.pending[index] {
                if self.written[index] != [true, true] || self.status[index] != 1 || p.pair != self.staged[index]
                    || p.request_at < self.inputs.request_ticks[index]
                    || p.request_at.checked_add(self.inputs.settle_ticks[index]) != Some(p.settle_at)
                    || p.request_token == 0 || p.request_token.checked_add(1) != Some(p.settle_token)
                    || p.settle_token > generation || !tokens.insert(p.request_token) || !tokens.insert(p.settle_token)
                { return Err("Invalid XP4 parameter checkpoint phase, deadline or token".into()); }
            } else if self.status[index] != if self.settled[index] { 0 } else { self.inputs.initial_status[index] } {
                return Err("Invalid idle XP4 parameter status checkpoint".into());
            }
        }
        Ok(())
    }
}
