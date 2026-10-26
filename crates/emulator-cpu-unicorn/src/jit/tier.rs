//! Generation ownership and bounded compilation policy. The host only loads
//! modules, owns table handles and exposes the engine's declared metadata.
use super::{
    Compiler, Environment, Helper, Module, PreparedFunction,
    ir::{Operation, Value},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
#[derive(tsify::Tsify)]
#[tsify(hashmap_as_object, missing_as_null)]
pub struct JitOptions {
    pub threshold: u32,
    pub batch_size: usize,
    pub max_functions: usize,
    pub max_bytes: usize,
}
impl Default for JitOptions {
    fn default() -> Self {
        Self {
            threshold: 32,
            batch_size: 16,
            max_functions: 32768,
            max_bytes: 64 << 20,
        }
    }
}
impl JitOptions {
    pub fn validate(self) -> Result<Self, String> {
        for (name, value, low, high) in [
            ("threshold", self.threshold as usize, 1, 1000000),
            ("batchSize", self.batch_size, 1, 64),
            ("maxFunctions", self.max_functions, 1, 32768),
            ("maxBytes", self.max_bytes, 65536, 128 << 20),
        ] {
            if !(low..=high).contains(&value) {
                return Err(format!("tcgJit {name} outside bounds"));
            }
        }
        Ok(self)
    }
}
pub struct Metadata {
    pub names: Vec<String>,
    pub environment: Environment,
}
/// Invalidations arrive after the engine has cleared its native TB slot.
/// Slot addresses are writable only until that generation is invalidated.
pub trait Host {
    fn dispatch_layout(&mut self) -> Result<Vec<u32>, String>;
    fn metadata(&mut self, engine: u32) -> Result<Metadata, String>;
    fn helper(&mut self, pointer: u32) -> Result<Option<Helper>, String>;
    fn grow_table(&mut self, count: u32) -> Result<u32, String>;
    fn install(&mut self, module: &Module, slots: &[u32]) -> Result<(), String>;
    fn clear_function(&mut self, slot: u32);
    fn publish(&mut self, address: u32, slot: u32);
    fn configure(&mut self, threshold: u32, dispatcher: u32) -> Result<(), String>;
    fn now_millis(&self) -> f64;
}
#[derive(Clone, Default, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
#[derive(tsify::Tsify)]
#[tsify(hashmap_as_object, missing_as_null)]
pub struct Stats {
    pub offered: u64,
    pub compiled: u64,
    pub unsupported: u64,
    pub failures: u64,
    pub invalidations: u64,
    pub flushes: u64,
    pub batches: u64,
    pub cache_hits: u64,
    pub generated_bytes: usize,
    pub dispatcher_bytes: usize,
    pub compile_milliseconds: f64,
    pub prepare_milliseconds: f64,
    pub capacity_refusals: u64,
    pub last_error: Option<String>,
    #[tsify(type = "Record<string, number>")]
    pub unsupported_operations: BTreeMap<String, u64>,
}
/// Browser tier counters published to the host.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
#[derive(tsify::Tsify)]
#[tsify(hashmap_as_object, missing_as_null)]
pub struct Statistics {
    #[tsify(type = "true")]
    pub enabled: bool,
    #[serde(flatten)]
    pub stats: Stats,
    pub installed: usize,
    pub retained_bytes: usize,
    pub cached_functions: usize,
    pub options: JitOptions,
    /// Decimal counters: these can exceed JavaScript's safe integers.
    pub chained_executions: String,
    pub compiled_executions: String,
    pub entries: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    engine: u32,
    generation: u32,
}
struct Engine {
    compiler: Compiler,
    environment: Environment,
}
struct Record {
    address: u32,
    code: PreparedFunction,
    slot: Option<u32>,
    group: Option<u64>,
}
struct Group {
    bytes: usize,
    live: usize,
}
struct Cached {
    slot: u32,
    group: u64,
    stamp: u64,
}
/// Owns one host for its entire lifetime. Reused function bytes therefore
/// always refer to the same imported memory, helper functions and table.
pub struct Tier<H: Host> {
    state: State,
    host: H,
}
impl<H: Host> Tier<H> {
    pub fn new(options: JitOptions, mut host: H) -> Result<Self, String> {
        Ok(Self {
            state: State::new(options, &mut host)?,
            host,
        })
    }
    pub fn notice(
        &mut self,
        event: u32,
        engine: u32,
        generation: u32,
        address: u32,
        start: u32,
        code: &[u8],
    ) -> Result<(), String> {
        self.state.notice(
            &mut self.host,
            event,
            engine,
            generation,
            address,
            start,
            code,
        )
    }
    pub fn flush(&mut self) -> Result<(), String> {
        self.state.flush(&mut self.host)
    }
    pub fn stats(&self) -> &Stats {
        self.state.stats()
    }
    pub fn options(&self) -> JitOptions {
        self.state.options()
    }
    pub fn installed(&self) -> usize {
        self.state.installed()
    }
    pub fn retained_bytes(&self) -> usize {
        self.state.retained_bytes()
    }
    pub fn cached(&self) -> usize {
        self.state.cached()
    }
}
struct State {
    options: JitOptions,
    stats: Stats,
    engines: BTreeMap<u32, Engine>,
    records: BTreeMap<Key, Record>,
    pending: Vec<Key>,
    free: BTreeSet<u32>,
    empty: BTreeSet<u32>,
    grown: usize,
    installed: usize,
    retained: usize,
    groups: BTreeMap<u64, Group>,
    next_group: u64,
    cache: BTreeMap<Arc<[u8]>, Cached>,
    cache_slots: BTreeMap<u32, Arc<[u8]>>,
    lru: BTreeMap<u64, u32>,
    stamp: u64,
}
impl State {
    pub fn new<H: Host>(options: JitOptions, host: &mut H) -> Result<Self, String> {
        let options = options.validate()?;
        let module = Compiler::dispatcher(&host.dispatch_layout()?)?;
        let dispatcher = host.grow_table(1)?;
        if dispatcher == 0 {
            return Err("JIT dispatcher slot must be nonzero".into());
        }
        host.install(&module, &[dispatcher])?;
        if let Err(error) = host.configure(options.threshold, dispatcher) {
            host.clear_function(dispatcher);
            return Err(error);
        }
        let bytes = module.bytes.len();
        Ok(Self {
            options,
            stats: Stats {
                generated_bytes: bytes,
                dispatcher_bytes: bytes,
                ..Default::default()
            },
            engines: BTreeMap::new(),
            records: BTreeMap::new(),
            pending: Vec::new(),
            free: BTreeSet::new(),
            empty: BTreeSet::new(),
            grown: 0,
            installed: 0,
            retained: bytes,
            groups: BTreeMap::new(),
            next_group: 0,
            cache: BTreeMap::new(),
            cache_slots: BTreeMap::new(),
            lru: BTreeMap::new(),
            stamp: 0,
        })
    }
    pub fn stats(&self) -> &Stats {
        &self.stats
    }
    pub fn options(&self) -> JitOptions {
        self.options
    }
    pub fn installed(&self) -> usize {
        self.installed
    }
    pub fn retained_bytes(&self) -> usize {
        self.retained
    }
    pub fn cached(&self) -> usize {
        self.cache.len()
    }
    fn failure(&mut self, error: &str) {
        self.stats.failures += 1;
        self.stats.last_error = Some(error.to_owned());
    }
    fn unsupported(&mut self, error: String) {
        self.stats.unsupported += 1;
        *self.stats.unsupported_operations.entry(error).or_default() += 1;
    }
    pub fn notice<H: Host>(
        &mut self,
        host: &mut H,
        event: u32,
        engine: u32,
        generation: u32,
        address: u32,
        start: u32,
        code: &[u8],
    ) -> Result<(), String> {
        let result = self.notice_inner(host, event, engine, generation, address, start, code);
        if let Err(error) = &result {
            self.failure(error);
        }
        result
    }
    fn notice_inner<H: Host>(
        &mut self,
        host: &mut H,
        event: u32,
        engine: u32,
        generation: u32,
        address: u32,
        start: u32,
        code: &[u8],
    ) -> Result<(), String> {
        let key = Key { engine, generation };
        match event {
            1 => {
                self.stats.offered += 1;
                if engine == 0 || generation == 0 || address == 0 {
                    return Err("invalid TCG generation offer".into());
                }
                if self.records.contains_key(&key) {
                    self.retire(host, key)?;
                    host.publish(address, 0);
                    return Err("duplicate live TCG generation".into());
                }
                if self.installed + self.pending.len() >= self.options.max_functions {
                    self.stats.capacity_refusals += 1;
                    return Ok(());
                }
                let started = host.now_millis();
                if !self.engines.contains_key(&engine) {
                    let metadata = host.metadata(engine)?;
                    self.engines.insert(
                        engine,
                        Engine {
                            compiler: Compiler::new(&metadata.names)?,
                            environment: metadata.environment,
                        },
                    );
                }
                let target = self.engines.get_mut(&engine).expect("engine was inserted");
                let prepared = match target.compiler.prepare(code, start) {
                    Ok(prepared) => prepared,
                    Err(error) => {
                        self.unsupported(error);
                        self.stats.prepare_milliseconds += (host.now_millis() - started).max(0.0);
                        return Ok(());
                    }
                };
                for instruction in prepared.instructions() {
                    if let Operation::Call {
                        function: Value::Constant(pointer),
                        ..
                    } = instruction.operation
                    {
                        if !target.environment.helpers.contains_key(&pointer) {
                            if let Some(kind) = host.helper(pointer)? {
                                target.environment.helpers.insert(pointer, kind);
                            }
                        }
                    }
                }
                let code = match target.compiler.lower(&prepared, &target.environment) {
                    Ok(code) => code,
                    Err(error) => {
                        self.unsupported(error);
                        self.stats.prepare_milliseconds += (host.now_millis() - started).max(0.0);
                        return Ok(());
                    }
                };
                self.stats.prepare_milliseconds += (host.now_millis() - started).max(0.0);
                if let Some((saved, cached)) = self.cache.remove_entry(code.bytes()) {
                    self.cache_slots.remove(&cached.slot);
                    self.lru.remove(&cached.stamp);
                    assert!(self.free.remove(&cached.slot));
                    self.records.insert(
                        key,
                        Record {
                            address,
                            code: PreparedFunction(saved),
                            slot: Some(cached.slot),
                            group: Some(cached.group),
                        },
                    );
                    self.installed += 1;
                    self.stats.cache_hits += 1;
                    host.publish(address, cached.slot);
                } else {
                    if !self.room(host, code.bytes().len()) {
                        self.stats.capacity_refusals += 1;
                        return Ok(());
                    }
                    self.retained += code.bytes().len();
                    self.records.insert(
                        key,
                        Record {
                            address,
                            code,
                            slot: None,
                            group: None,
                        },
                    );
                    self.pending.push(key);
                    if self.pending.len() >= self.options.batch_size {
                        self.flush_inner(host)?;
                    }
                }
            }
            2 => {
                if self.records.contains_key(&key) {
                    self.retire(host, key)?;
                    self.stats.invalidations += 1;
                }
            }
            3 => {
                let keys = self
                    .records
                    .keys()
                    .filter(|key| key.engine == engine)
                    .copied()
                    .collect::<Vec<_>>();
                for key in keys {
                    self.retire(host, key)?;
                }
                self.engines.remove(&engine);
                self.stats.flushes += 1;
            }
            _ => return Err(format!("unknown TCG cache event {event}")),
        }
        Ok(())
    }
    fn release_group(&mut self, id: u64) {
        let group = self
            .groups
            .get_mut(&id)
            .expect("function module group exists");
        group.live -= 1;
        if group.live == 0 {
            let group = self.groups.remove(&id).expect("group exists");
            self.retained -= group.bytes;
        }
    }
    fn evict<H: Host>(&mut self, host: &mut H, slot: u32) {
        let Some(code) = self.cache_slots.remove(&slot) else {
            return;
        };
        let cached = self
            .cache
            .remove(code.as_ref())
            .expect("cache reverse mapping exists");
        self.lru.remove(&cached.stamp);
        host.clear_function(slot);
        self.empty.insert(slot);
        self.retained -= code.len();
        self.release_group(cached.group);
    }
    fn room<H: Host>(&mut self, host: &mut H, bytes: usize) -> bool {
        while self
            .retained
            .checked_add(bytes)
            .is_none_or(|n| n > self.options.max_bytes)
        {
            let Some((_, slot)) = self.lru.first_key_value() else {
                return false;
            };
            let slot = *slot;
            self.evict(host, slot);
        }
        true
    }
    fn retire<H: Host>(&mut self, host: &mut H, key: Key) -> Result<(), String> {
        let stamp = if self.records[&key].slot.is_some() {
            Some(
                self.stamp
                    .checked_add(1)
                    .ok_or("JIT cache stamp exhausted")?,
            )
        } else {
            None
        };
        let record = self.records.remove(&key).expect("live generation exists");
        self.pending.retain(|pending| *pending != key);
        if let (Some(slot), Some(group)) = (record.slot, record.group) {
            self.installed -= 1;
            self.free.insert(slot);
            if let Some(old) = self.cache.get(record.code.bytes()) {
                let old = old.slot;
                self.evict(host, old);
            }
            self.stamp = stamp.expect("installed generation reserved a cache stamp");
            self.lru.insert(self.stamp, slot);
            self.cache_slots.insert(slot, record.code.0.clone());
            self.cache.insert(
                record.code.0,
                Cached {
                    slot,
                    group,
                    stamp: self.stamp,
                },
            );
        } else {
            self.retained -= record.code.bytes().len();
        }
        Ok(())
    }
    fn slots<H: Host>(&mut self, host: &mut H, count: usize) -> Result<Vec<u32>, String> {
        let missing = count.saturating_sub(self.free.len());
        if self.grown + missing > self.options.max_functions {
            return Err("JIT table capacity invariant failed".into());
        }
        if missing != 0 {
            let first = host.grow_table(missing as u32)?;
            if first == 0 || first.checked_add(missing as u32 - 1).is_none() {
                return Err("invalid JIT table allocation".into());
            }
            for n in 0..missing {
                self.free.insert(first + n as u32);
                self.empty.insert(first + n as u32);
            }
            self.grown += missing;
        }
        let mut slots = Vec::with_capacity(count);
        for _ in 0..count {
            let slot = if let Some(slot) = self.empty.pop_first() {
                slot
            } else {
                let slot = *self
                    .lru
                    .first_key_value()
                    .expect("reserved cached capacity")
                    .1;
                self.evict(host, slot);
                assert!(self.empty.remove(&slot));
                slot
            };
            assert!(self.free.remove(&slot));
            slots.push(slot);
        }
        Ok(slots)
    }
    pub fn flush<H: Host>(&mut self, host: &mut H) -> Result<(), String> {
        let result = self.flush_inner(host);
        if let Err(error) = &result {
            self.failure(error);
        }
        result
    }
    fn flush_inner<H: Host>(&mut self, host: &mut H) -> Result<(), String> {
        let batch = std::mem::take(&mut self.pending);
        if batch.is_empty() {
            return Ok(());
        }
        let started = host.now_millis();
        let result = self.compile_batch(host, &batch);
        self.stats.compile_milliseconds += (host.now_millis() - started).max(0.0);
        if result.is_err() {
            for key in batch {
                self.retire(host, key)?;
            }
        }
        result
    }
    fn compile_batch<H: Host>(&mut self, host: &mut H, batch: &[Key]) -> Result<(), String> {
        let functions = batch
            .iter()
            .map(|key| self.records[key].code.clone())
            .collect::<Vec<_>>();
        let module = self.engines[&batch[0].engine]
            .compiler
            .compile(&functions)?;
        if !self.room(host, module.bytes.len()) {
            self.stats.capacity_refusals += batch.len() as u64;
            for key in batch {
                self.retire(host, *key)?;
            }
            return Ok(());
        }
        let group = self
            .next_group
            .checked_add(1)
            .ok_or("JIT module group exhausted")?;
        let slots = self.slots(host, batch.len())?;
        if let Err(error) = host.install(&module, &slots) {
            for slot in slots {
                host.clear_function(slot);
                self.free.insert(slot);
                self.empty.insert(slot);
            }
            return Err(error);
        }
        self.next_group = group;
        self.groups.insert(
            group,
            Group {
                bytes: module.bytes.len(),
                live: batch.len(),
            },
        );
        self.retained += module.bytes.len();
        for (key, slot) in batch.iter().zip(slots) {
            let record = self
                .records
                .get_mut(key)
                .expect("pending generation is live");
            record.slot = Some(slot);
            record.group = Some(group);
            self.installed += 1;
            host.publish(record.address, slot);
        }
        self.stats.generated_bytes += module.bytes.len();
        self.stats.compiled += batch.len() as u64;
        self.stats.batches += 1;
        Ok(())
    }
}

#[cfg(test)]
#[path = "tier_tests.rs"]
mod tests;
