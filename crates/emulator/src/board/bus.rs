//! Common device dispatch, discovery ledger and causal wakeups.
use fujimod_emulator_devices::bus::{FastReadValue, FastReadWord, HostRequest};
use fujimod_emulator_devices::graph::{Mapping, validate_mappings};
use fujimod_emulator_devices::{Cx, Device, Host};
use serde_json::{Value, json};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
mod checkpoint;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Mode {
    Strict,
    Discovery,
}
impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Self::Strict => "strict",
            Self::Discovery => "discovery",
        }
    }
}
struct Window {
    mappings: Vec<Mapping>,
    device: Box<dyn Device>,
    native_dependencies: Vec<usize>,
}
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Site {
    pub address: Option<u64>,
    pub pc: u64,
    pub kind: Cow<'static, str>,
    pub device: Option<usize>,
    pub phase: Cow<'static, str>,
    pub cpu: Option<usize>,
    pub size: Option<u8>,
    pub write: Option<bool>,
    pub value: Option<u64>,
    pub message: String,
}

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Unknown {
    first: usize,
    count: u64,
    tick: u64,
    pcs: Vec<u64>,
    model: Option<Cow<'static, str>>,
    reads: u64,
    writes: u64,
    sizes: BTreeSet<u8>,
    cpus: BTreeSet<usize>,
    values: BTreeSet<u64>,
    values_omitted: u64,
    reasons: BTreeSet<String>,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transaction {
    pub cpu: usize,
    pub pc: u64,
    pub address: u64,
    pub size: u8,
    pub operation: Cow<'static, str>,
    pub value: u64,
    pub accepted: bool,
    pub tick: u64,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TransactionRange {
    begin: u64,
    end: u64,
}

#[derive(serde::Serialize)]
pub struct TransactionBatch {
    pub transactions: Vec<Transaction>,
    pub omitted: u64,
}

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Transactions {
    limit: usize,
    rows: Vec<Transaction>,
    omitted: u64,
    ranges: Option<Vec<TransactionRange>>,
}
pub struct Bus {
    host_profile: Option<super::host_profile::Profile>,
    transactions: Option<Transactions>,
    continuation: Option<std::rc::Rc<std::cell::RefCell<super::continuation::Stops>>>,
    windows: Vec<Window>,
    // Sparse 1 MiB groups of 4 KiB pages. Several independent register blocks
    // can occupy one page; only that page's candidates participate in dispatch.
    pages: BTreeMap<u64, Box<[Vec<usize>; 256]>>,
    dependents: Vec<Vec<usize>>,
    mode: Mode,
    continue_refusals: bool,
    // One ledger for unsupported accesses and deliberately skipped refusals.
    unknown: BTreeMap<Site, Unknown>,
    pending_pcs: Vec<Site>,
    skipped_deadlines: BTreeMap<usize, u64>,
    // Discovery state belongs to the framework, never to a device or RAM.
    shadow: BTreeMap<u64, u8>,
    touched: Vec<usize>,
    dirty: Vec<bool>,
    wake_queue: Vec<usize>,
    visited: Vec<u64>,
    wake_generation: u64,
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Checkpoint {
    #[serde(skip)]
    devices: Vec<fujimod_emulator_devices::checkpoint::Checkpoint>,
    windows: Vec<(Vec<Mapping>, Vec<usize>)>,
    mode: Mode,
    continue_refusals: bool,
    transactions: Option<Transactions>,
    unknown: BTreeMap<Site, Unknown>,
    shadow: BTreeMap<u64, u8>,
    skipped_deadlines: BTreeMap<usize, u64>,
    touched: Vec<usize>,
    dirty: Vec<bool>,
    wake_queue: Vec<usize>,
    visited: Vec<u64>,
    wake_generation: u64,
}
impl Checkpoint {
    pub(super) fn encoded_bytes(&self) -> usize {
        self.devices
            .iter()
            .map(fujimod_emulator_devices::checkpoint::Checkpoint::bytes)
            .sum()
    }
}
impl Bus {
    pub(super) fn checkpoint(&self, previous: Option<&Checkpoint>) -> Result<Checkpoint, String> {
        if !self.pending_pcs.is_empty() {
            return Err("keyframe requires completed discovery PC recording".into());
        }
        let devices = self
            .windows
            .iter()
            .enumerate()
            .map(|(id, node)| {
                let mut state = node
                    .device
                    .checkpoint()
                    .map_err(|e| format!("device {id} ({}): {e}", node.device.name()))?;
                if let Some(old) = previous.and_then(|p| p.devices.get(id)) {
                    state.share_unchanged(old);
                }
                Ok(state)
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Checkpoint {
            devices,
            windows: self
                .windows
                .iter()
                .map(|w| (w.mappings.clone(), w.native_dependencies.clone()))
                .collect(),
            mode: self.mode,
            continue_refusals: self.continue_refusals,
            transactions: self.transactions.clone(),
            unknown: self.unknown.clone(),
            shadow: self.shadow.clone(),
            skipped_deadlines: self.skipped_deadlines.clone(),
            touched: self.touched.clone(),
            dirty: self.dirty.clone(),
            wake_queue: self.wake_queue.clone(),
            visited: self.visited.clone(),
            wake_generation: self.wake_generation,
        })
    }
    pub(super) fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        if self.mode != saved.mode {
            return Err("keyframe execution mode differs".into());
        }
        if self.windows.len() != saved.devices.len()
            || self
                .windows
                .iter()
                .zip(&saved.windows)
                .any(|(node, (maps, deps))| {
                    &node.mappings != maps || &node.native_dependencies != deps
                })
        {
            return Err("keyframe device graph differs".into());
        }
        for (id, (node, state)) in self.windows.iter().zip(&saved.devices).enumerate() {
            node.device
                .validate_checkpoint(state)
                .map_err(|e| format!("device {id} ({}): {e}", node.device.name()))?;
        }
        Ok(())
    }
    pub(super) fn restore_checkpoint(&mut self, saved: &Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        for (node, state) in self.windows.iter_mut().zip(&saved.devices) {
            node.device.restore_checkpoint(state)?;
        }
        self.mode = saved.mode;
        self.continue_refusals = saved.continue_refusals;
        self.transactions.clone_from(&saved.transactions);
        self.unknown.clone_from(&saved.unknown);
        self.shadow.clone_from(&saved.shadow);
        self.skipped_deadlines.clone_from(&saved.skipped_deadlines);
        self.pending_pcs.clear();
        self.touched.clone_from(&saved.touched);
        self.dirty.clone_from(&saved.dirty);
        self.wake_queue.clone_from(&saved.wake_queue);
        self.visited.clone_from(&saved.visited);
        self.wake_generation = saved.wake_generation;
        Ok(())
    }
    pub(super) fn fingerprint(&self) -> Result<Value, String> {
        let devices = self
            .windows
            .iter()
            .enumerate()
            .map(|(index, node)| {
                let mut value = json!({"kind":node.device.name(),"windows":node.mappings,
                    "native_dependencies":node.native_dependencies});
                value["state"] = node
                    .device
                    .fingerprint()
                    .map_err(|e| format!("device {index} ({}): {e}", node.device.name()))?;
                Ok(value)
            })
            .collect::<Result<Vec<_>, String>>()?;
        let mut state = self.state_with_devices(Value::Array(devices));
        state["skipped_deadlines"] = json!(self.skipped_deadlines);
        // Visit stamps only deduplicate one already-drained worklist. Their
        // arbitrary identities are accelerator bookkeeping, not pending work.
        state["wakeups"] = json!({"touched":self.touched,"dirty":self.dirty,
            "queue":self.wake_queue});
        if !self.pending_pcs.is_empty() {
            return Err("fingerprint requires completed discovery PC recording".into());
        }
        Ok(state)
    }

    /// Read only the selected native device kind; no access or tick is issued.
    pub(super) fn device_ids(&self, kind: &str) -> Vec<usize> {
        self.windows
            .iter()
            .enumerate()
            .filter(|(_, w)| w.device.name() == kind)
            .map(|(id, _)| id)
            .collect()
    }
    pub(super) fn continuation(
        &mut self,
        stops: Option<std::rc::Rc<std::cell::RefCell<super::continuation::Stops>>>,
    ) {
        self.continuation = stops;
    }
    fn publication_before(&self, id: usize) -> Option<Value> {
        self.continuation
            .as_ref()
            .filter(|s| s.borrow().devices.contains(&id))
            .map(|_| self.windows[id].device.save())
    }
    fn publication_after(&self, id: usize, before: Option<Value>) {
        if let Some(before) = before {
            if before != self.windows[id].device.save() {
                if let Some(stops) = &self.continuation {
                    stops.borrow_mut().device(id);
                }
            }
        }
    }
    pub fn device_states(&self, kind: &str) -> Vec<Value> {
        self.windows
            .iter()
            .filter(|node| node.device.name() == kind)
            .map(|node| node.device.diagnostic_state())
            .collect()
    }

    pub fn device_fingerprints(&self, kind: &str) -> Result<Vec<Value>, String> {
        self.windows
            .iter()
            .filter(|node| node.device.name() == kind)
            .map(|node| node.device.fingerprint())
            .collect()
    }

    pub fn device_buffer(&self, kind: &str, name: &str) -> Result<Vec<u8>, String> {
        let mut devices = self.windows.iter().filter(|node| node.device.name() == kind);
        let device = devices.next().ok_or("no matching device")?;
        if devices.next().is_some() {
            return Err("buffer observation requires one matching device".into());
        }
        Ok(device.device.diagnostic_buffer(name)?.to_vec())
    }

    /// Typed stopped-board access. This is never used by register dispatch.
    pub fn device<T: Device>(&self, id: usize) -> Result<&T, String> {
        let node = self.windows.get(id).ok_or("no such device")?;
        let any: &dyn std::any::Any = &*node.device;
        any.downcast_ref()
            .ok_or_else(|| "device has a different type".into())
    }
    pub fn device_dyn(&self, id: usize) -> Result<&dyn Device, String> {
        Ok(&*self.windows.get(id).ok_or("no such device")?.device)
    }
    pub fn device_dyn_mut(&mut self, id: usize) -> Result<&mut dyn Device, String> {
        Ok(&mut *self.windows.get_mut(id).ok_or("no such device")?.device)
    }
    pub fn device_mut<T: Device>(&mut self, id: usize) -> Result<&mut T, String> {
        let node = self.windows.get_mut(id).ok_or("no such device")?;
        let any: &mut dyn std::any::Any = &mut *node.device;
        any.downcast_mut()
            .ok_or_else(|| "device has a different type".into())
    }
    pub fn from_graph(
        mode: Mode,
        graph: fujimod_emulator_devices::graph::DeviceGraph,
    ) -> Result<Self, String> {
        let mut bus = Self::new(mode);
        let mut dependencies = Vec::new();
        for node in graph.nodes {
            let id = bus.register_windows(node.mappings, node.device)?;
            dependencies.push((id, node.native_dependencies));
        }
        for (device, sources) in dependencies {
            bus.native_dependencies(device, sources)?;
        }
        Ok(bus)
    }
    pub fn new(mode: Mode) -> Self {
        Self {
            host_profile: None,
            transactions: None,
            continuation: None,
            windows: vec![],
            pages: BTreeMap::new(),
            dependents: vec![],
            mode,
            continue_refusals: false,
            pending_pcs: vec![],
            skipped_deadlines: BTreeMap::new(),
            unknown: BTreeMap::new(),
            shadow: BTreeMap::new(),
            touched: vec![],
            dirty: vec![],
            wake_queue: vec![],
            visited: vec![],
            wake_generation: 0,
        }
    }
    pub(super) fn start_host_profile(
        &mut self,
        baseline: Vec<super::host_profile::CpuCounters>,
    ) -> Result<(), String> {
        self.host_profile = Some(super::host_profile::Profile::new(baseline)?);
        Ok(())
    }
    pub(super) fn profile_begin_run(&mut self) -> Option<super::host_profile::Stamp> {
        self.host_profile.as_mut().map(|p| p.begin_run())
    }
    pub(super) fn profile_finish_run(&mut self, stamp: Option<super::host_profile::Stamp>) {
        if let (Some(p), Some(stamp)) = (&mut self.host_profile, stamp) {
            p.finish_run(stamp);
        }
    }
    pub(super) fn profile_begin_engine(&self) -> Option<super::host_profile::Stamp> {
        self.host_profile.as_ref().map(|p| p.start())
    }
    pub(super) fn profile_finish_engine(
        &mut self,
        core: usize,
        stamp: Option<super::host_profile::Stamp>,
    ) {
        if let (Some(p), Some(stamp)) = (&mut self.host_profile, stamp) {
            p.record_engine(core, stamp);
        }
    }
    pub(super) fn host_profile(
        &self,
        counters: impl FnOnce() -> Vec<Result<super::host_profile::CpuCounters, String>>,
    ) -> Value {
        self.host_profile
            .as_ref()
            .map(|p| p.report(|id| self.windows[id].device.name(), &counters()))
            .unwrap_or(Value::Null)
    }
    fn record_host_time(
        &mut self,
        id: usize,
        phase: &'static str,
        start: Option<super::host_profile::Stamp>,
    ) {
        if let (Some(profile), Some(start)) = (&mut self.host_profile, start) {
            profile.record(id, phase, start);
        }
    }
    pub fn inputs_changed(&mut self, device: usize) -> Result<(), String> {
        if device >= self.len() {
            return Err("input targets absent device".into());
        }
        self.touch(device);
        Ok(())
    }
    pub fn mode(&self) -> Mode {
        self.mode
    }
    pub fn continue_refusals(&mut self, enabled: bool) -> Result<(), String> {
        if enabled && self.mode != Mode::Discovery {
            return Err("continuing modeled refusals requires discovery".into());
        }
        self.continue_refusals = enabled;
        Ok(())
    }
    fn record_frontier(&mut self, tick: u64, site: Site, model: Option<&'static str>) {
        let first = self.unknown.len();
        let row = self.unknown.entry(site.clone()).or_insert_with(|| Unknown {
            first,
            tick,
            model: model.map(Into::into),
            ..Unknown::default()
        });
        if row.count == 0 {
            self.pending_pcs.push(site.clone());
        }
        row.count += 1;
        row.reasons.insert(site.message.clone());
        if let Some(size) = site.size {
            row.sizes.insert(size);
        }
        if let Some(cpu) = site.cpu {
            row.cpus.insert(cpu);
        }
        match site.write {
            Some(true) => {
                row.writes += 1;
                if let Some(value) = site.value {
                    if row.values.len() < 64 {
                        row.values.insert(value);
                    } else if !row.values.contains(&value) {
                        row.values_omitted = row.values_omitted.saturating_add(1);
                    }
                }
            }
            Some(false) => row.reads += 1,
            None => (),
        }
    }
    pub(super) fn needs_frontier_pcs(&self) -> bool {
        !self.pending_pcs.is_empty()
    }
    pub(super) fn capture_frontier_pcs(&mut self, pcs: &[u64]) {
        for key in self.pending_pcs.drain(..) {
            self.unknown.get_mut(&key).unwrap().pcs = pcs.to_vec();
        }
    }
    fn skip_refusal(&mut self, now: u64, mut site: Site, reason: String) -> Result<(), String> {
        if !self.continue_refusals {
            return Err(reason);
        }
        site.kind = "model-refusal".into();
        site.message = reason;
        let id = site.device.expect("a modeled refusal has an owning device");
        let device = &self.windows[id].device;
        let name = device.name();
        self.record_frontier(now, site, Some(name));
        let device = &self.windows[id].device;
        if let Some(at) = device.next_deadline(now).filter(|&at| at <= now) {
            self.skipped_deadlines.insert(id, at);
        }
        Ok(())
    }
    fn deadline(&self, id: usize, now: u64) -> Option<u64> {
        self.windows[id]
            .device
            .next_deadline(now)
            .filter(|at| *at > now || self.skipped_deadlines.get(&id) != Some(at))
    }
    pub fn len(&self) -> usize {
        self.windows.len()
    }
    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }
    pub(super) fn loop_window(&self) -> bool {
        self.mode == Mode::Strict
            && self.transactions.is_none()
            && self
                .continuation
                .as_ref()
                .is_none_or(|stops| stops.borrow().devices.is_empty())
    }
    /// Published reads that fall inside this CPU's physical MMIO mapping.
    /// Device declarations are relative to their registered windows; return
    /// offsets relative to the requested mapping for the CPU backend.
    pub(super) fn fast_reads(
        &self,
        physical_base: u64,
        bytes: usize,
    ) -> Vec<(u64, FastReadValue)> {
        let Some(physical_end) = physical_base.checked_add(bytes as u64) else {
            return Vec::new();
        };
        let mut reads = Vec::new();
        for node in &self.windows {
            for mapping in &node.mappings {
                let Some(mapping_end) = mapping.base.checked_add(mapping.size) else {
                    continue;
                };
                for FastReadWord { offset, value } in
                    node.device.fast_reads(mapping.base, mapping.size)
                {
                    let Some(address) = mapping.base.checked_add(offset) else {
                        continue;
                    };
                    let width = value.width();
                    let Some(end) = address.checked_add(width) else {
                        continue;
                    };
                    if !offset.is_multiple_of(width)
                        || end > mapping_end
                        || address < physical_base
                        || end > physical_end
                    {
                        continue;
                    }
                    reads.push((address - physical_base, value));
                }
            }
        }
        reads.sort_by_key(|(offset, _)| *offset);
        reads
    }
    pub fn register(
        &mut self,
        base: u64,
        bytes: u64,
        device: Box<dyn Device>,
    ) -> Result<usize, String> {
        self.register_windows(vec![Mapping { base, size: bytes }], device)
    }
    pub fn register_windows(
        &mut self,
        mappings: Vec<Mapping>,
        device: Box<dyn Device>,
    ) -> Result<usize, String> {
        validate_mappings(&mappings, self.windows.iter().flat_map(|w| &w.mappings))?;
        Ok(self.insert(mappings, device))
    }
    /// Non-MMIO hardware participates in the same deadline graph without
    /// allocating a fictitious register window.
    pub fn attach(&mut self, device: Box<dyn Device>) -> usize {
        self.insert(vec![], device)
    }
    fn insert(&mut self, mappings: Vec<Mapping>, device: Box<dyn Device>) -> usize {
        let id = self.windows.len();
        for m in &mappings {
            for page in m.base >> 12..=(m.base + m.size - 1) >> 12 {
                let candidates = &mut self
                    .pages
                    .entry(page >> 8)
                    .or_insert_with(|| Box::new(std::array::from_fn(|_| Vec::new())))
                    [(page & 255) as usize];
                if candidates.last() != Some(&id) {
                    candidates.push(id);
                }
            }
        }
        self.windows.push(Window {
            mappings,
            device,
            native_dependencies: vec![],
        });
        self.dependents.push(vec![]);
        self.dirty.push(false);
        self.visited.push(0);
        // Each node enters these buffers at most once per refresh/wakeup.
        // Reserve during construction, never in guest-driven device dispatch.
        self.touched
            .reserve(self.windows.len() - self.touched.len());
        self.wake_queue
            .reserve(self.windows.len() - self.wake_queue.len());
        self.touch(id);
        id
    }
    fn touch(&mut self, id: usize) {
        if !self.dirty[id] {
            self.dirty[id] = true;
            self.touched.push(id);
        }
    }
    /// A native model consumes state changed by these causing models. A
    /// guest access or due effect on a source checks the dependent's deadline.
    pub fn native_dependencies(
        &mut self,
        device: usize,
        sources: Vec<usize>,
    ) -> Result<(), String> {
        if device >= self.len()
            || sources.iter().any(|&s| s >= self.len() || s == device)
            || sources.iter().copied().collect::<BTreeSet<_>>().len() != sources.len()
        {
            return Err("invalid native device dependencies".into());
        }
        for &source in &self.windows[device].native_dependencies {
            self.dependents[source].retain(|&id| id != device);
        }
        for &source in &sources {
            self.dependents[source].push(device);
            self.dependents[source].sort_unstable();
        }
        self.windows[device].native_dependencies = sources;
        Ok(())
    }
    fn outcome(cx: &mut Cx<'_>) -> Result<bool, String> {
        if cx.is_unmodeled() {
            return Ok(false);
        }
        if let Some(reason) = cx.take_refusal() {
            Err(reason)
        } else {
            Ok(true)
        }
    }
    /// A bounded passive observer. Truncation never changes dispatch or results.
    pub fn observe_transactions(&mut self, limit: Option<usize>) {
        self.transactions = limit.map(|limit| Transactions {
            limit,
            ..Default::default()
        });
    }
    pub fn transactions(&self) -> (&[Transaction], u64) {
        self.transactions
            .as_ref()
            .map(|t| (t.rows.as_slice(), t.omitted))
            .unwrap_or((&[], 0))
    }
    pub fn transactions_active(&self) -> bool {
        self.transactions.is_some()
    }

    pub fn transaction_watch_begin(
        &mut self,
        ranges: Vec<(u64, u64)>,
        limit: usize,
    ) -> Result<(), String> {
        if self.transactions.is_some() {
            return Err("Another bus transaction observer is active".into());
        }
        if ranges.is_empty()
            || limit == 0
            || ranges.iter().any(|&(begin, end)| begin >= end)
        {
            return Err("Bus transaction watch requires bounded address windows and records".into());
        }
        self.transactions = Some(Transactions {
            limit,
            ranges: Some(
                ranges
                    .into_iter()
                    .map(|(begin, end)| TransactionRange { begin, end })
                    .collect(),
            ),
            ..Default::default()
        });
        Ok(())
    }

    pub fn transaction_watch_end(&mut self) -> Result<TransactionBatch, String> {
        if !self.transactions.as_ref().is_some_and(|log| log.ranges.is_some()) {
            return Err("No scoped bus transaction observer is active".into());
        }
        let log = self.transactions.take().unwrap();
        Ok(TransactionBatch {
            transactions: log.rows,
            omitted: log.omitted,
        })
    }

    pub fn access(
        &mut self,
        host: &mut dyn Host,
        cpu: usize,
        pc: u64,
        address: u64,
        size: u8,
        value: Option<u64>,
    ) -> Result<u64, String> {
        let result = self.dispatch(host, cpu, pc, address, size, value);
        if let Some(log) = &mut self.transactions {
            if log.ranges.as_ref().is_some_and(|ranges| {
                !ranges.iter().any(|range| {
                    address < range.end && range.begin < address.saturating_add(u64::from(size))
                })
            }) {
                return result;
            }
            if log.rows.len() < log.limit {
                log.rows.push(Transaction {
                    cpu,
                    pc,
                    address,
                    size,
                    operation: if value.is_some() { "write" } else { "read" }.into(),
                    value: value.unwrap_or_else(|| result.as_ref().copied().unwrap_or(0)),
                    accepted: result.is_ok(),
                    tick: host.now(),
                });
            } else {
                log.omitted = log.omitted.saturating_add(1);
            }
        }
        result
    }
    fn dispatch(
        &mut self,
        host: &mut dyn Host,
        cpu: usize,
        pc: u64,
        address: u64,
        size: u8,
        value: Option<u64>,
    ) -> Result<u64, String> {
        if !matches!(size, 1 | 2 | 4 | 8) {
            return Err("invalid MMIO access width".into());
        }
        address
            .checked_add(size as u64)
            .ok_or("unmodeled MMIO address overflow")?;
        let mask = u64::MAX >> (64 - size as u32 * 8);
        let mut result = 0;
        let index = self.pages.get(&(address >> 20)).and_then(|pages| {
            pages[((address >> 12) & 255) as usize]
                .iter()
                .copied()
                .find(|&id| {
                    self.windows[id].mappings.iter().any(|m| {
                        address >= m.base
                            && address
                                .checked_add(size as u64)
                                .is_some_and(|e| e <= m.base + m.size)
                    })
                })
        });
        let mut supported = false;
        let mut reason = None;
        let mut continuation = None;
        if let Some(id) = index {
            let publication = self.publication_before(id);
            let started = self.host_profile.as_ref().map(|p| p.start());
            self.touch(id);
            let device = &mut self.windows[id].device;
            let mut phase = "advance";
            let outcome = (|| {
                let mut cx = Cx::new(host, cpu, pc, id);
                device.advance(&mut cx);
                let mut supported = false;
                if Self::outcome(&mut cx)? {
                    phase = if value.is_some() { "write" } else { "read" };
                    let rejected = if self.mode == Mode::Discovery {
                        device.access_contract(address, size, value.map(|v| v & mask))
                    } else {
                        None
                    };
                    if let Some(reason) = rejected {
                        cx.unsupported(reason);
                    } else if let Some(value) = value {
                        device.write(&mut cx, address, size, value & mask);
                    } else {
                        result = device.read(&mut cx, address, size) & mask;
                    }
                    supported = Self::outcome(&mut cx)?;
                }
                if !supported {
                    reason = cx.refusal().map(String::from);
                }
                drop(cx);
                if !supported && self.mode == Mode::Discovery {
                    phase = "continuation";
                    let mut cx = Cx::new(host, cpu, pc, id);
                    continuation =
                        device.discovery_access(&mut cx, address, size, value.map(|v| v & mask));
                    if !Self::outcome(&mut cx)? {
                        reason = Some(format!(
                            "{}; continuation refused: {}",
                            reason.as_deref().unwrap_or("unsupported access"),
                            cx.refusal().unwrap_or("unmodeled transport")
                        ));
                        continuation = None;
                    }
                }
                Ok(supported)
            })();
            self.record_host_time(
                id,
                if value.is_some() {
                    "advance-and-write"
                } else {
                    "advance-and-read"
                },
                started,
            );
            self.publication_after(id, publication);
            supported = match outcome {
                Ok(supported) => supported,
                Err(message) => {
                    self.skip_refusal(
                        host.now(),
                        Site {
                            kind: "".into(),
                            device: Some(id),
                            phase: phase.into(),
                            cpu: Some(cpu),
                            pc,
                            address: Some(address),
                            size: Some(size),
                            write: Some(value.is_some()),
                            value: Some(value.unwrap_or(0) & mask),
                            message: String::new(),
                        },
                        message,
                    )?;
                    return Ok(0);
                }
            };
            if supported || continuation.is_some() {
                self.wake_dependents(host, id)?;
            }
        }
        if !supported {
            result = 0;
            if self.mode == Mode::Discovery {
                for byte in 0..size {
                    let address = address
                        .checked_add(byte as u64)
                        .ok_or("unmodeled MMIO address overflow")?;
                    if let Some(value) = value {
                        self.shadow
                            .insert(address, (value >> (byte as u32 * 8)) as u8);
                    } else {
                        result |= u64::from(*self.shadow.get(&address).unwrap_or(&0))
                            << (byte as u32 * 8);
                    }
                }
            }
            let message = reason.unwrap_or_else(|| "unowned MMIO".into());
            self.record_frontier(
                host.now(),
                Site {
                    kind: "unmodeled-mmio".into(),
                    device: index,
                    phase: if value.is_some() { "write" } else { "read" }.into(),
                    cpu: Some(cpu),
                    pc,
                    address: Some(address),
                    size: Some(size),
                    write: Some(value.is_some()),
                    value: Some(value.unwrap_or(result) & mask),
                    message: message.clone(),
                },
                index.map(|id| self.windows[id].device.name()),
            );
            if self.mode == Mode::Strict {
                let written_value = value
                    .map(|value| format!(" value={:#x}", value & mask))
                    .unwrap_or_default();
                return Err(format!(
                    "unmodeled MMIO {}{size} at {address:#x}, CPU{cpu} PC={pc:#x}{written_value}: {message}",
                    if value.is_some() { "write" } else { "read" }
                ));
            }
        }
        Ok(continuation.unwrap_or(result) & mask)
    }
    pub fn event(
        &mut self,
        host: &mut dyn Host,
        device: usize,
        token: Option<u64>,
    ) -> Result<(), String> {
        if device >= self.len() {
            return Err("event targets absent device".into());
        }
        let publication = self.publication_before(device);
        self.touch(device);
        let due = self
            .deadline(device, host.now())
            .is_some_and(|t| t <= host.now());
        let w = &mut self.windows[device];
        let started = self.host_profile.as_ref().map(|p| p.start());
        let mut cx = Cx::new(host, usize::MAX, 0, device);
        if let Some(token) = token {
            w.device.event(&mut cx, token);
        } else if due {
            w.device.advance(&mut cx);
        }
        let outcome = Self::outcome(&mut cx);
        drop(cx);
        self.publication_after(device, publication);
        self.record_host_time(
            device,
            if token.is_some() { "event" } else { "deadline" },
            started,
        );
        if outcome != Ok(true) {
            self.skip_refusal(
                host.now(),
                Site {
                    kind: "".into(),
                    device: Some(device),
                    phase: if token.is_some() { "event" } else { "deadline" }.into(),
                    cpu: None,
                    pc: 0,
                    address: None,
                    size: None,
                    write: None,
                    value: token,
                    message: String::new(),
                },
                outcome
                    .err()
                    .unwrap_or_else(|| "device event is unmodeled".into()),
            )?;
            return Ok(());
        }
        self.wake_dependents(host, device)
    }
    pub fn take_host_requests(&mut self) -> Vec<(usize, HostRequest)> {
        self.windows
            .iter_mut()
            .enumerate()
            .filter_map(|(device, window)| {
                window
                    .device
                    .take_host_request()
                    .map(|request| (device, request))
            })
            .collect()
    }
    pub fn has_host_requests(&self) -> bool {
        self.windows
            .iter()
            .any(|window| window.device.has_host_request())
    }
    pub fn complete_host_request(
        &mut self,
        host: &mut dyn Host,
        device: usize,
        id: u64,
        result: &[u8],
    ) -> Result<(), String> {
        if device >= self.len() {
            return Err("host request targets absent device".into());
        }
        let publication = self.publication_before(device);
        self.touch(device);
        let mut cx = Cx::new(host, usize::MAX, 0, device);
        let result = self.windows[device]
            .device
            .complete_host_request(&mut cx, id, result);
        let outcome = Self::outcome(&mut cx);
        drop(cx);
        self.publication_after(device, publication);
        result?;
        match outcome {
            Ok(true) => {}
            Ok(false) => return Err("host request completion is unmodeled".into()),
            Err(error) => return Err(error),
        }
        self.wake_dependents(host, device)
    }
    fn wake_dependents(&mut self, host: &mut dyn Host, source: usize) -> Result<(), String> {
        self.wake_queue.clear();
        self.wake_generation = self.wake_generation.wrapping_add(1);
        if self.wake_generation == 0 {
            self.visited.fill(0);
            self.wake_generation = 1;
        }
        self.wake_queue.push(source);
        self.visited[source] = self.wake_generation;
        while let Some(source) = self.wake_queue.pop() {
            for index in 0..self.dependents[source].len() {
                let id = self.dependents[source][index];
                if self.visited[id] == self.wake_generation {
                    continue;
                }
                self.visited[id] = self.wake_generation;
                self.touch(id);
                if self
                    .deadline(id, host.now())
                    .is_some_and(|t| t <= host.now())
                {
                    let mut cx = Cx::new(host, usize::MAX, 0, id);
                    let started = self.host_profile.as_ref().map(|p| p.start());
                    self.windows[id].device.advance(&mut cx);
                    let outcome = Self::outcome(&mut cx);
                    drop(cx);
                    self.record_host_time(id, "dependency", started);
                    if outcome != Ok(true) {
                        self.skip_refusal(
                            host.now(),
                            Site {
                                kind: "".into(),
                                device: Some(id),
                                phase: "dependency".into(),
                                cpu: None,
                                pc: 0,
                                address: None,
                                size: None,
                                write: None,
                                value: None,
                                message: String::new(),
                            },
                            outcome
                                .err()
                                .unwrap_or_else(|| "dependent device effect is unmodeled".into()),
                        )?;
                        continue;
                    }
                    self.wake_queue.push(id);
                }
            }
        }
        Ok(())
    }
    pub fn changed_deadlines(
        &mut self,
        now: u64,
    ) -> impl Iterator<Item = (usize, Option<u64>)> + '_ {
        // Stable node order retains deadline insertion order for simultaneous
        // changes, independent of which access first marked a node dirty.
        self.touched.sort_unstable();
        let Self {
            touched,
            dirty,
            windows,
            skipped_deadlines,
            ..
        } = self;
        touched.drain(..).map(move |id| {
            dirty[id] = false;
            let deadline = windows[id]
                .device
                .next_deadline(now)
                .filter(|at| *at > now || skipped_deadlines.get(&id) != Some(at));
            (id, deadline)
        })
    }
    pub fn set_scene(&mut self, input: &Value) -> Result<(), String> {
        let mut consumed = false;
        for (id, w) in self.windows.iter_mut().enumerate() {
            if w.device.set_scene(input)? {
                consumed = true;
                if !self.dirty[id] {
                    self.dirty[id] = true;
                    self.touched.push(id);
                }
            }
        }
        if !consumed {
            return Err("no device consumes scene input".into());
        }
        Ok(())
    }
    pub fn state(&self) -> Value {
        let devices = self.windows.iter().map(|w| {
            let mut value = json!({"windows":w.mappings,"kind":w.device.name(),"native_dependencies":w.native_dependencies});
            value["state"] = w.device.save();
            value
        }).collect();
        self.state_with_devices(Value::Array(devices))
    }
    fn state_with_devices(&self, devices: Value) -> Value {
        let mut frontier = self
            .unknown
            .iter()
            .map(|(site, r)| {
                json!({
                    "first":r.first,"count":r.count,"tick":r.tick,"pcs":r.pcs,"model":r.model,
                    "kind":site.kind,"device":site.device,"phase":site.phase,"cpu":site.cpu,
                    "pc":site.pc,"address":site.address,"size":site.size,"write":site.write,
                    "value":site.value,"message":site.message,
                })
            })
            .collect::<Vec<_>>();
        frontier.sort_by_key(|r| r["first"].as_u64().unwrap());
        let mut state = json!({"mode":self.mode.name(),
            "unmodeled":self.unknown.iter().filter(|(site,_)|site.kind=="unmodeled-mmio").map(|(site,r)|json!({"address":site.address,"pc":site.pc,"reads":r.reads,"writes":r.writes,"sizes":r.sizes,"cpus":r.cpus,"values":r.values,"values_omitted":r.values_omitted,"reasons":r.reasons})).collect::<Vec<_>>(),"discovery_shadow":self.shadow.iter().collect::<Vec<_>>(),
            "continue_refusals":self.continue_refusals,"frontier":frontier});
        state["devices"] = devices;
        state
    }
    pub fn finish(&mut self) -> Result<(), String> {
        for w in &mut self.windows {
            w.device.finish()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn checkpoints_cannot_reclassify_strict_and_discovery_execution() {
        for (mode, saved_mode) in [
            (super::Mode::Strict, super::Mode::Discovery),
            (super::Mode::Discovery, super::Mode::Strict),
        ] {
            let saved = super::Bus::new(saved_mode).checkpoint(None).unwrap();
            let mut target = super::Bus::new(mode);
            let before = target.state();
            assert_eq!(
                target.restore_checkpoint(&saved).unwrap_err(),
                "keyframe execution mode differs"
            );
            assert_eq!(target.state(), before);
        }
    }
    use super::*;
    use fujimod_emulator_devices::Record;
    use std::sync::{Arc, Mutex};

    pub(super) struct Services;
    impl Host for Services {
        fn now(&self) -> u64 {
            7
        }
        fn schedule(&mut self, _: usize, _: u64, _: u64) {
            panic!("unexpected schedule")
        }
        fn set_irq(&mut self, _: u32, _: bool) {
            panic!("unexpected IRQ")
        }
        fn raise_irq(&mut self, _: usize, _: u32) -> Result<(), String> {
            panic!("unexpected IRQ")
        }
        fn ram_read(&self, _: u64, _: &mut [u8]) -> Result<(), String> {
            panic!("unexpected DMA")
        }
        fn ram_write(&mut self, _: u64, _: &[u8]) -> Result<(), String> {
            panic!("unexpected DMA")
        }
        fn record(&mut self, _: Record) {
            panic!("unexpected record")
        }
    }
    #[test]
    fn fingerprint_uses_complete_device_state_without_building_diagnostic_summaries() {
        struct Complete;
        impl Device for Complete {
            fn name(&self) -> &'static str {
                "complete"
            }
            fn save(&self) -> Value {
                panic!("discarded diagnostic allocation")
            }
            fn fingerprint(&self) -> Result<Value, String> {
                Ok(json!({"payload":[1,2,3],"pending":7}))
            }
            fn read(&mut self, _: &mut Cx<'_>, _: u64, _: u8) -> u64 {
                unreachable!()
            }
            fn write(&mut self, _: &mut Cx<'_>, _: u64, _: u8, _: u64) {
                unreachable!()
            }
        }
        let mut bus = Bus::new(Mode::Strict);
        let mut expected = bus.state();
        bus.register(0x1000, 4, Box::new(Complete)).unwrap();
        assert_eq!(bus.device_fingerprints("complete").unwrap(), vec![json!({"payload":[1,2,3],"pending":7})]);
        assert!(bus.device_fingerprints("absent").unwrap().is_empty());
        expected["devices"] = json!([{"kind":"complete","windows":bus.windows[0].mappings,
            "native_dependencies":[],"state":{"payload":[1,2,3],"pending":7}}]);
        expected["skipped_deadlines"] = json!({});
        expected["wakeups"] =
            json!({"touched":bus.touched,"dirty":bus.dirty,"queue":bus.wake_queue});
        assert_eq!(
            serde_json::to_vec(&bus.fingerprint().unwrap()).unwrap(),
            serde_json::to_vec(&expected).unwrap()
        );
    }

    struct Register(u64);
    impl Device for Register {
        fn name(&self) -> &'static str {
            "register"
        }
        fn read(&mut self, _: &mut Cx<'_>, _: u64, _: u8) -> u64 {
            self.0
        }
        fn write(&mut self, _: &mut Cx<'_>, _: u64, _: u8, value: u64) {
            self.0 = value;
        }
    }

    struct Transport {
        writes: u32,
        continuations: u32,
    }
    impl Device for Transport {
        fn name(&self) -> &'static str {
            "transport"
        }
        fn read(&mut self, cx: &mut Cx<'_>, _: u64, _: u8) -> u64 {
            cx.unsupported("unbound part");
            0
        }
        fn write(&mut self, cx: &mut Cx<'_>, _: u64, _: u8, _: u64) {
            self.writes += 1;
            cx.unsupported("unbound part");
        }
        fn access_contract(&self, _: u64, _: u8, _: Option<u64>) -> Option<String> {
            Some("unbound part".into())
        }
        fn discovery_access(
            &mut self,
            _: &mut Cx<'_>,
            _: u64,
            _: u8,
            _: Option<u64>,
        ) -> Option<u64> {
            self.continuations += 1;
            Some(7)
        }
        fn save(&self) -> Value {
            json!({"writes":self.writes,"continuations":self.continuations})
        }
    }
    #[test]
    fn transport_continuations_are_explicit_discovery_and_keep_the_unsupported_ledger() {
        let mut strict = Bus::new(Mode::Strict);
        let mut discovery = Bus::new(Mode::Discovery);
        for bus in [&mut strict, &mut discovery] {
            bus.register(
                0x1000,
                4,
                Box::new(Transport {
                    writes: 0,
                    continuations: 0,
                }),
            )
            .unwrap();
        }
        assert!(
            strict
                .access(&mut Services, 0, 0x80, 0x1000, 4, Some(19))
                .is_err()
        );
        assert_eq!(
            strict.state()["devices"][0]["state"],
            json!({"writes":1,"continuations":0})
        );
        assert_eq!(
            discovery
                .access(&mut Services, 1, 0x88, 0x1000, 4, Some(19))
                .unwrap(),
            7
        );
        assert_eq!(
            discovery
                .access(&mut Services, 1, 0x8c, 0x1000, 4, None)
                .unwrap(),
            7
        );
        let state = discovery.state();
        assert_eq!(
            state["devices"][0]["state"],
            json!({"writes":0,"continuations":2})
        );
        assert_eq!(state["unmodeled"][0]["writes"], 1);
        assert_eq!(state["unmodeled"][1]["reads"], 1);
        assert_eq!(state["unmodeled"][0]["reasons"], json!(["unbound part"]));
    }

    #[test]
    fn bounded_transaction_observation_preserves_dispatch_and_records_refusals() {
        let mut plain = Bus::new(Mode::Strict);
        let mut observed = Bus::new(Mode::Strict);
        for bus in [&mut plain, &mut observed] {
            bus.register(0x1000, 4, Box::new(Register(5))).unwrap();
        }
        observed.observe_transactions(Some(3));
        for (pc, address, value) in [
            (0x20, 0x1000, None),
            (0x24, 0x1000, Some(19)),
            (0x28, 0x2000, None),
            (0x2c, 0x1000, None),
        ] {
            let a = plain.access(&mut Services, 1, pc, address, 4, value);
            let b = observed.access(&mut Services, 1, pc, address, 4, value);
            assert_eq!(a, b);
            assert_eq!(plain.state(), observed.state());
        }
        let (rows, omitted) = observed.transactions();
        assert_eq!(rows.len(), 3);
        assert_eq!(omitted, 1);
        assert_eq!(
            (rows[0].cpu, rows[0].pc, rows[0].address, rows[0].value),
            (1, 0x20, 0x1000, 5)
        );
        assert_eq!((rows[1].operation.as_ref(), rows[1].value), ("write", 19));
        assert!(!rows[2].accepted);
        assert_eq!(observed.transactions().0.len(), 3);
        observed.observe_transactions(None);
        assert!(observed.transactions().0.is_empty());
        assert_eq!(plain.state(), observed.state());
    }

    #[test]
    fn host_profiling_is_observational_and_never_enters_guest_state() {
        let mut plain = Bus::new(Mode::Strict);
        let mut observed = Bus::new(Mode::Strict);
        for bus in [&mut plain, &mut observed] {
            bus.register(0x1000, 4, Box::new(Register(5))).unwrap();
        }
        assert_eq!(
            observed.host_profile(|| panic!("inactive profiler queried CPU")),
            Value::Null
        );
        observed.start_host_profile(Vec::new()).unwrap();
        for value in [None, Some(19), None] {
            assert_eq!(
                plain.access(&mut Services, 0, 0, 0x1000, 4, value),
                observed.access(&mut Services, 0, 0, 0x1000, 4, value)
            );
            assert_eq!(plain.state(), observed.state());
        }
        let before = observed.state();
        let report = observed.host_profile(Vec::new);
        assert_eq!(report["devices"].as_array().unwrap().len(), 2);
        assert_eq!(
            report["devices"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v["calls"].as_u64().unwrap())
                .sum::<u64>(),
            3
        );
        assert_eq!(before, observed.state());
    }

    #[test]
    fn page_dispatch_preserves_subpage_ranges_cross_page_accesses_and_high_addresses() {
        let mut bus = Bus::new(Mode::Strict);
        bus.register(0x1ffc, 8, Box::new(Register(11))).unwrap();
        bus.register(0x2004, 4, Box::new(Register(22))).unwrap();
        bus.register(0x1_0000_2004, 4, Box::new(Register(33)))
            .unwrap();
        let mut host = Services;
        for (address, size, expected) in [
            (0x1ffc, 8, 11),
            (0x2000, 4, 11),
            (0x2004, 4, 22),
            (0x1_0000_2004, 4, 33),
        ] {
            assert_eq!(
                bus.access(&mut host, 0, 0, address, size, None).unwrap(),
                expected
            );
        }
        for address in [0x1ff8, 0x2008, 0x1_0000_2000, u64::MAX - 3] {
            assert!(
                bus.access(&mut host, 0, 0, address, 8, None)
                    .unwrap_err()
                    .contains("unmodeled MMIO")
            );
        }
        assert!(bus.access(&mut host, 0, 0, 0x2000, 8, None).is_err());
        assert!(bus.register(0x2003, 1, Box::new(Register(0))).is_err());
    }

    #[test]
    fn halfword_publications_fit_narrow_windows_and_rebase_at_two_byte_offsets() {
        use std::sync::{Arc, atomic::AtomicU16};
        struct Halfword;
        impl Device for Halfword {
            fn name(&self) -> &'static str {
                "halfword-snapshot"
            }
            fn read(&mut self, _: &mut Cx<'_>, _: u64, _: u8) -> u64 {
                7
            }
            fn write(&mut self, _: &mut Cx<'_>, _: u64, _: u8, _: u64) {}
            fn fast_reads(&self, _: u64, _: u64) -> Vec<FastReadWord> {
                vec![FastReadWord::atomic_u16(0, Arc::new(AtomicU16::new(7)))]
            }
        }
        let mut bus = Bus::new(Mode::Strict);
        bus.register(0x1002, 2, Box::new(Halfword)).unwrap();
        let words = bus.fast_reads(0x1000, 4);
        assert_eq!(words.len(), 1);
        assert_eq!(words[0].0, 2);
        assert_eq!(words[0].1.width(), 2);
        assert_eq!(bus.fast_reads(0x1002, 2).len(), 1);
        assert!(bus.fast_reads(0x1000, 3).is_empty());
        assert!(bus.fast_reads(0x1003, 2).is_empty());
    }

    #[test]
    fn pure_read_snapshots_rebase_from_their_device_windows() {
        use std::sync::Arc;
        use std::sync::atomic::AtomicU32;

        struct Snapshot(Arc<AtomicU32>);
        impl Device for Snapshot {
            fn name(&self) -> &'static str {
                "snapshot"
            }
            fn read(&mut self, _: &mut Cx<'_>, _: u64, _: u8) -> u64 {
                0
            }
            fn write(&mut self, _: &mut Cx<'_>, _: u64, _: u8, _: u64) {}
            fn fast_reads(&self, base: u64, size: u64) -> Vec<FastReadWord> {
                let offset = match base {
                    0x1000 => 0x10,
                    0x9000 => 4,
                    _ => return Vec::new(),
                };
                if offset + 4 > size {
                    return Vec::new();
                }
                vec![FastReadWord::atomic_u32(offset, self.0.clone())]
            }
        }

        let value = Arc::new(AtomicU32::new(7));
        let mut bus = Bus::new(Mode::Strict);
        bus.register_windows(
            vec![
                Mapping {
                    base: 0x1000,
                    size: 0x20,
                },
                Mapping {
                    base: 0x9000,
                    size: 0x20,
                },
            ],
            Box::new(Snapshot(value.clone())),
        )
        .unwrap();

        let whole = bus.fast_reads(0x1000, 0x20);
        assert_eq!(whole.len(), 1);
        assert_eq!(whole[0].0, 0x10);
        assert!(matches!(
            &whole[0].1,
            FastReadValue::AtomicU32(snapshot) if Arc::ptr_eq(snapshot, &value)
        ));

        let subrange = bus.fast_reads(0x1008, 0x10);
        assert_eq!(subrange.len(), 1);
        assert_eq!(subrange[0].0, 8);
        assert!(matches!(
            &subrange[0].1,
            FastReadValue::AtomicU32(snapshot) if Arc::ptr_eq(snapshot, &value)
        ));

        let second = bus.fast_reads(0x9000, 0x20);
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].0, 4);
        assert!(matches!(
            &second[0].1,
            FastReadValue::AtomicU32(snapshot) if Arc::ptr_eq(snapshot, &value)
        ));
        let register_slice = bus.fast_reads(0x9004, 4);
        assert_eq!(register_slice.len(), 1);
        assert_eq!(register_slice[0].0, 0);
        assert!(bus.fast_reads(0x9008, 4).is_empty());
    }

    #[test]
    fn disjoint_windows_share_state_and_one_deadline_slot_without_claiming_gaps() {
        let mut graph = fujimod_emulator_devices::graph::DeviceGraph::new();
        graph
            .register_windows(
                vec![
                    Mapping {
                        base: 0x1000,
                        size: 4,
                    },
                    Mapping {
                        base: 0x1010,
                        size: 4,
                    },
                    Mapping {
                        base: 0x9000,
                        size: 4,
                    },
                ],
                Box::new(Register(11)),
            )
            .unwrap();
        let mut bus = Bus::from_graph(Mode::Strict, graph).unwrap();
        assert_eq!(bus.len(), 1);
        assert_eq!(bus.pages[&0][1], vec![0]);
        bus.access(&mut Services, 0, 0, 0x1010, 4, Some(22))
            .unwrap();
        for address in [0x1000, 0x1010, 0x9000] {
            assert_eq!(
                bus.access(&mut Services, 0, 0, address, 4, None).unwrap(),
                22
            );
        }
        assert!(bus.access(&mut Services, 0, 0, 0x1000, 8, None).is_err());
        assert!(bus.access(&mut Services, 0, 0, 0x1004, 4, None).is_err());
        assert_eq!(
            bus.changed_deadlines(7).collect::<Vec<_>>(),
            vec![(0, None)]
        );
    }

    #[test]
    fn dependent_cycles_wake_once_and_access_buffers_retain_capacity() {
        struct Due {
            due: bool,
            calls: Arc<Mutex<Vec<usize>>>,
        }
        impl Device for Due {
            fn name(&self) -> &'static str {
                "due"
            }
            fn read(&mut self, _: &mut Cx<'_>, _: u64, _: u8) -> u64 {
                0
            }
            fn write(&mut self, _: &mut Cx<'_>, _: u64, _: u8, _: u64) {}
            fn next_deadline(&self, now: u64) -> Option<u64> {
                self.due.then_some(now)
            }
            fn advance(&mut self, cx: &mut Cx<'_>) {
                if self.due {
                    self.due = false;
                    self.calls.lock().unwrap().push(cx.device);
                }
            }
        }
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut bus = Bus::new(Mode::Strict);
        for i in 0..3 {
            bus.register(
                0x1000 + i * 4,
                4,
                Box::new(Due {
                    due: true,
                    calls: calls.clone(),
                }),
            )
            .unwrap();
        }
        bus.native_dependencies(1, vec![0]).unwrap();
        bus.native_dependencies(2, vec![1]).unwrap();
        bus.native_dependencies(0, vec![2]).unwrap();
        assert_eq!(
            bus.changed_deadlines(7).collect::<Vec<_>>(),
            vec![(0, Some(7)), (1, Some(7)), (2, Some(7))]
        );
        let capacity = (bus.touched.capacity(), bus.wake_queue.capacity());
        bus.access(&mut Services, 0, 0, 0x1000, 4, None).unwrap();
        assert_eq!(*calls.lock().unwrap(), vec![0, 1, 2]);
        assert_eq!(
            bus.changed_deadlines(7).collect::<Vec<_>>(),
            vec![(0, None), (1, None), (2, None)]
        );
        for _ in 0..100 {
            bus.access(&mut Services, 0, 0, 0x1000, 4, None).unwrap();
        }
        assert_eq!(
            bus.changed_deadlines(7).collect::<Vec<_>>(),
            vec![(0, None), (1, None)]
        );
        assert_eq!(
            capacity,
            (bus.touched.capacity(), bus.wake_queue.capacity())
        );
        // Replacing a wiring edge removes its former reverse connection.
        bus.native_dependencies(1, vec![2]).unwrap();
        bus.access(&mut Services, 0, 0, 0x1000, 4, None).unwrap();
        assert_eq!(
            bus.changed_deadlines(7).collect::<Vec<_>>(),
            vec![(0, None)]
        );
    }
}

#[cfg(test)]
mod discovery_tests {
    use super::tests::Services;
    use super::*;
    struct Missing;
    impl Device for Missing {
        fn name(&self) -> &'static str {
            "missing"
        }
        fn read(&mut self, cx: &mut Cx<'_>, _: u64, _: u8) -> u64 {
            cx.unsupported("unimplemented electrical response");
            99
        }
        fn write(&mut self, cx: &mut Cx<'_>, _: u64, _: u8, _: u64) {
            cx.unsupported("unimplemented electrical response");
        }
    }
    struct Failed;
    impl Device for Failed {
        fn name(&self) -> &'static str {
            "failed"
        }
        fn read(&mut self, cx: &mut Cx<'_>, _: u64, _: u8) -> u64 {
            cx.fail("DMA buffer is not mapped");
            0
        }
        fn write(&mut self, _: &mut Cx<'_>, _: u64, _: u8, _: u64) {}
    }
    #[test]
    fn discovery_retains_unsupported_evidence_and_byte_addressed_shadows_but_not_operational_failures()
     {
        for modeled in [false, true] {
            let mut bus = Bus::new(Mode::Discovery);
            if modeled {
                bus.register(0x1000, 16, Box::new(Missing)).unwrap();
            }
            assert_eq!(
                bus.access(&mut Services, 0, 0x44, 0x1000, 4, None).unwrap(),
                0
            );
            bus.access(&mut Services, 1, 0x48, 0x1000, 4, Some(0xaabbccdd))
                .unwrap();
            bus.access(&mut Services, 1, 0x4c, 0x1001, 1, Some(0x77))
                .unwrap();
            assert_eq!(
                bus.access(&mut Services, 0, 0x50, 0x1000, 4, None).unwrap(),
                0xaabb77dd
            );
            assert_eq!(
                bus.access(&mut Services, 0, 0x54, 0x1002, 2, None).unwrap(),
                0xaabb
            );
            let state = bus.state();
            assert_eq!(state, bus.state());
            assert_eq!(state["unmodeled"].as_array().unwrap().len(), 5);
            assert_eq!(state["unmodeled"][1]["cpus"], json!([1]));
            assert_eq!(
                state["unmodeled"][0]["reasons"],
                json!([if modeled {
                    "unimplemented electrical response"
                } else {
                    "unowned MMIO"
                }])
            );
            bus.register(0x2000, 4, Box::new(Failed)).unwrap();
            assert_eq!(
                bus.access(&mut Services, 0, 0x58, 0x2000, 4, None)
                    .unwrap_err(),
                "DMA buffer is not mapped"
            );
            assert!(
                bus.access(&mut Services, 0, 0, u64::MAX, 2, Some(0))
                    .is_err()
            );
        }
    }
}
