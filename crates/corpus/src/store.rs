//! SQLite persistence. `Store::open` reads only the header; lookups are SQL
//! queries, and graph operations load an `Index` from the store.
//!
//! Schema (one transaction per save, indexes created after the rows):
//! - `units(firmware, unit, layout JSON)`, `regions(firmware, unit, address, bytes)`,
//!   `firmwares(firmware, functions)`
//! - `functions(id, firmware, unit, address, family, architecture, instructions,
//!   signature, names JSON, annotations JSON, ranges JSON, calls)`
//! - `function_features(id, tokens, constants, ngrams, refs JSON)`
//! - `function_names(name, id)`, `function_strings(id, text)`, `tokens(id, text)`
//! - `families(id, annotations JSON)`, `weak(a, b)`, `weak_families(family, other)`
//! - `data_symbols(firmware, unit, address, length, name, kind, source)`
//!
//! Token ids and call pairs are LEB128 varints, constants are delta varints,
//! n-grams are little-endian u64 words and signatures are 32 raw bytes.
//! Addresses are stored as the two's-complement `i64` of the `u64`.
use crate::*;
use rusqlite::{Connection, OpenFlags, OptionalExtension, Row, params, params_from_iter};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// "FMCS" in the SQLite header's application id field.
const APPLICATION_ID: i64 = 0x464d_4353;
const FORMAT: i64 = 1;
const SCHEMA: &str = "
CREATE TABLE units(firmware TEXT NOT NULL, unit TEXT NOT NULL, layout TEXT NOT NULL,
    PRIMARY KEY(firmware, unit));
CREATE TABLE firmwares(firmware TEXT PRIMARY KEY, functions INTEGER NOT NULL) WITHOUT ROWID;
CREATE TABLE regions(firmware TEXT NOT NULL, unit TEXT NOT NULL, address INTEGER NOT NULL,
    bytes BLOB NOT NULL, PRIMARY KEY(firmware, unit, address));
CREATE TABLE tokens(id INTEGER PRIMARY KEY, text TEXT NOT NULL);
CREATE TABLE families(id INTEGER PRIMARY KEY, annotations TEXT NOT NULL);
CREATE TABLE functions(id INTEGER PRIMARY KEY, firmware TEXT NOT NULL, unit TEXT NOT NULL,
    address INTEGER NOT NULL, family INTEGER, architecture TEXT NOT NULL,
    instructions INTEGER NOT NULL, signature BLOB NOT NULL, names TEXT NOT NULL,
    annotations TEXT NOT NULL, ranges TEXT NOT NULL, calls BLOB NOT NULL);
CREATE TABLE function_features(id INTEGER PRIMARY KEY, tokens BLOB NOT NULL,
    constants BLOB NOT NULL, ngrams BLOB NOT NULL, refs TEXT NOT NULL);
CREATE TABLE function_names(name TEXT NOT NULL, id INTEGER NOT NULL,
    PRIMARY KEY(name, id)) WITHOUT ROWID;
CREATE TABLE function_strings(id INTEGER NOT NULL, text TEXT NOT NULL,
    PRIMARY KEY(id, text)) WITHOUT ROWID;
CREATE TABLE weak(a INTEGER NOT NULL, b INTEGER NOT NULL);
CREATE TABLE weak_families(family INTEGER NOT NULL, other INTEGER NOT NULL,
    PRIMARY KEY(family, other)) WITHOUT ROWID;
CREATE TABLE data_symbols(firmware TEXT NOT NULL, unit TEXT NOT NULL, address INTEGER NOT NULL,
    length INTEGER NOT NULL, name TEXT NOT NULL, kind TEXT NOT NULL, source TEXT NOT NULL);
";
const INDEXES: &str = "
CREATE INDEX functions_key ON functions(firmware, unit, address);
CREATE INDEX functions_address ON functions(firmware, address);
CREATE INDEX functions_family ON functions(family);
CREATE INDEX function_strings_text ON function_strings(text);
CREATE INDEX data_symbols_name ON data_symbols(name);
";
const HEADER: &str = "f.id, f.firmware, f.unit, f.address, f.family, f.architecture, \
    f.instructions, f.signature, f.names, f.annotations, f.ranges, f.calls";

fn invalid(what: &str) -> Error {
    Error::Invalid(format!("corrupt index store: {what}"))
}
fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}
struct Varints<'a>(&'a [u8]);
impl Iterator for Varints<'_> {
    type Item = Result<u64>;
    fn next(&mut self) -> Option<Result<u64>> {
        if self.0.is_empty() {
            return None;
        }
        let mut value = 0u64;
        for (i, &byte) in self.0.iter().enumerate().take(10) {
            value |= u64::from(byte & 0x7f) << (7 * i);
            if byte & 0x80 == 0 {
                self.0 = &self.0[i + 1..];
                return Some(Ok(value));
            }
        }
        self.0 = &[];
        Some(Err(invalid("varint")))
    }
}
fn encode_tokens(tokens: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(tokens.len() * 2);
    for &t in tokens {
        put_varint(&mut out, t.into());
    }
    out
}
fn decode_tokens(bytes: &[u8]) -> Result<Vec<u32>> {
    Varints(bytes)
        .map(|v| u32::try_from(v?).map_err(|_| invalid("token id")))
        .collect()
}
fn encode_calls(calls: &[(usize, u64)]) -> Vec<u8> {
    let mut out = Vec::with_capacity(calls.len() * 6);
    for &(offset, target) in calls {
        put_varint(&mut out, offset as u64);
        put_varint(&mut out, target);
    }
    out
}
fn decode_calls(bytes: &[u8]) -> Result<Vec<(usize, u64)>> {
    let values = Varints(bytes).collect::<Result<Vec<_>>>()?;
    if values.len() % 2 != 0 {
        return Err(invalid("calls"));
    }
    Ok(values.chunks(2).map(|p| (p[0] as usize, p[1])).collect())
}
fn encode_deltas(values: &BTreeSet<u64>) -> Vec<u8> {
    let mut out = Vec::new();
    let mut previous = 0;
    for &v in values {
        put_varint(&mut out, v - previous);
        previous = v;
    }
    out
}
fn decode_deltas(bytes: &[u8]) -> Result<BTreeSet<u64>> {
    let mut previous = 0u64;
    Varints(bytes)
        .map(|d| {
            previous = previous
                .checked_add(d?)
                .ok_or_else(|| invalid("constants"))?;
            Ok(previous)
        })
        .collect()
}
fn encode_words(values: &BTreeSet<u64>) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}
fn decode_words(bytes: &[u8]) -> Result<BTreeSet<u64>> {
    let (words, rest) = bytes.as_chunks::<8>();
    if !rest.is_empty() {
        return Err(invalid("n-grams"));
    }
    Ok(words.iter().map(|w| u64::from_le_bytes(*w)).collect())
}
fn architecture_name(architecture: Architecture) -> &'static str {
    match architecture {
        Architecture::Arm32 => "arm32",
        Architecture::Aarch64 => "aarch64",
    }
}
fn architecture(name: &str) -> Result<Architecture> {
    match name {
        "arm32" => Ok(Architecture::Arm32),
        "aarch64" => Ok(Architecture::Aarch64),
        _ => Err(invalid("architecture")),
    }
}
fn stored(address: u64) -> i64 {
    address as i64
}
fn header(row: &Row) -> Result<Entry<'static>> {
    let id: i64 = row.get(0)?;
    let family: Option<i64> = row.get(4)?;
    let signature: Vec<u8> = row.get(7)?;
    let names: String = row.get(8)?;
    let annotations: String = row.get(9)?;
    let ranges: String = row.get(10)?;
    let calls: Vec<u8> = row.get(11)?;
    Ok(Entry {
        id: id as usize,
        family: family.map(|f| f as usize),
        function: Cow::Owned(FunctionHeader {
            key: FunctionKey {
                firmware: row.get(1)?,
                unit: row.get(2)?,
                address: row.get::<_, i64>(3)? as u64,
            },
            architecture: architecture(&row.get::<_, String>(5)?)?,
            names: serde_json::from_str(&names)?,
            annotations: serde_json::from_str(&annotations)?,
            ranges: serde_json::from_str(&ranges)?,
            signature: signature
                .try_into()
                .map_err(|_| invalid("signature length"))?,
            instructions: row.get::<_, i64>(6)? as usize,
            calls: decode_calls(&calls)?,
        }),
    })
}

/// Write `index` as a new SQLite store at `path`, replacing any file there.
pub(crate) fn write(index: &Index, path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
        _ => {}
    }
    let mut connection = Connection::open(path)?;
    connection.execute_batch(&format!(
        "PRAGMA page_size = 16384; PRAGMA journal_mode = OFF; PRAGMA synchronous = OFF;
         PRAGMA locking_mode = EXCLUSIVE; PRAGMA application_id = {APPLICATION_ID};
         PRAGMA user_version = {FORMAT};"
    ))?;
    let tx = connection.transaction()?;
    tx.execute_batch(SCHEMA)?;
    {
        let mut counts = BTreeMap::<&str, i64>::new();
        for f in &index.functions {
            *counts.entry(f.key.firmware.as_str()).or_default() += 1;
        }
        let mut firmwares = tx.prepare("INSERT INTO firmwares VALUES (?1, ?2)")?;
        for (firmware, count) in counts {
            firmwares.execute(params![firmware, count])?;
        }
        let mut units = tx.prepare("INSERT INTO units VALUES (?1, ?2, ?3)")?;
        let mut regions = tx.prepare("INSERT INTO regions VALUES (?1, ?2, ?3, ?4)")?;
        for u in &index.units {
            let l = &u.layout;
            units.execute(params![l.firmware, l.unit, serde_json::to_string(l)?])?;
            for r in &u.regions {
                regions.execute(params![l.firmware, l.unit, stored(r.address), r.bytes])?;
            }
        }
        let mut tokens = tx.prepare("INSERT INTO tokens VALUES (?1, ?2)")?;
        for (i, t) in index.tokens.iter().enumerate() {
            tokens.execute(params![i as i64, t])?;
        }
        let mut families = tx.prepare("INSERT INTO families VALUES (?1, ?2)")?;
        for f in &index.families {
            families.execute(params![f.id as i64, serde_json::to_string(&f.annotations)?])?;
        }
        let mut functions = tx.prepare(
            "INSERT INTO functions VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        )?;
        let mut features =
            tx.prepare("INSERT INTO function_features VALUES (?1, ?2, ?3, ?4, ?5)")?;
        let mut names = tx.prepare("INSERT OR IGNORE INTO function_names VALUES (?1, ?2)")?;
        let mut strings = tx.prepare("INSERT INTO function_strings VALUES (?1, ?2)")?;
        for (i, f) in index.functions.iter().enumerate() {
            let id = i as i64;
            functions.execute(params![
                id,
                f.key.firmware,
                f.key.unit,
                stored(f.key.address),
                index.membership.get(i).map(|&m| m as i64),
                architecture_name(f.architecture),
                f.instructions as i64,
                &f.signature[..],
                serde_json::to_string(&f.names)?,
                serde_json::to_string(&f.annotations)?,
                serde_json::to_string(&f.ranges)?,
                encode_calls(&f.calls),
            ])?;
            features.execute(params![
                id,
                encode_tokens(&f.tokens),
                encode_deltas(&f.constants),
                encode_words(&f.ngrams),
                serde_json::to_string(&f.references)?,
            ])?;
            for name in &f.names {
                names.execute(params![name, id])?;
            }
            for text in &f.strings {
                strings.execute(params![id, text])?;
            }
        }
        let mut weak = tx.prepare("INSERT INTO weak VALUES (?1, ?2)")?;
        for &(a, b) in &index.weak {
            weak.execute(params![a as i64, b as i64])?;
        }
        let mut adjacent = tx.prepare("INSERT INTO weak_families VALUES (?1, ?2)")?;
        for (&family, others) in &index.weak_families {
            for &other in others {
                adjacent.execute(params![family as i64, other as i64])?;
            }
        }
        let mut symbols =
            tx.prepare("INSERT INTO data_symbols VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)")?;
        for s in &index.data_symbols {
            symbols.execute(params![
                s.firmware,
                s.unit,
                stored(s.address),
                s.length as i64,
                s.name,
                s.kind,
                s.source
            ])?;
        }
    }
    tx.execute_batch(INDEXES)?;
    tx.commit()?;
    Ok(())
}

/// A saved index. Opening reads only the SQLite header; units are cached once read.
pub struct Store {
    connection: Mutex<Connection>,
    units: Mutex<HashMap<(String, String), Arc<Unit>>>,
}
impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        let application: i64 =
            connection.pragma_query_value(None, "application_id", |r| r.get(0))?;
        let version: i64 = connection.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if application != APPLICATION_ID || version != FORMAT {
            return Err(Error::Invalid("unsupported index format".into()));
        }
        Ok(Self {
            connection: Mutex::new(connection),
            units: Mutex::default(),
        })
    }
    fn connection(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.connection.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// The complete index with its saved families.
    pub fn load(&self) -> Result<Index> {
        let (mut index, membership) = self.parts(None)?;
        let c = self.connection();
        let mut families = Vec::new();
        let mut statement = c.prepare("SELECT id, annotations FROM families ORDER BY id")?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let id = row.get::<_, i64>(0)? as usize;
            if id != families.len() {
                return Err(invalid("family ids"));
            }
            families.push(Family {
                id,
                members: vec![],
                annotations: serde_json::from_str(&row.get::<_, String>(1)?)?,
            });
        }
        if membership.iter().all(Option::is_some) && !families.is_empty() {
            for (id, family) in membership.iter().enumerate() {
                let family = family.unwrap();
                families
                    .get_mut(family)
                    .ok_or_else(|| invalid("membership"))?
                    .members
                    .push(id);
            }
            index.membership = membership.into_iter().flatten().collect();
        }
        index.families = families;
        index.weak = c
            .prepare("SELECT a, b FROM weak ORDER BY rowid")?
            .query_map([], |r| {
                Ok((r.get::<_, i64>(0)? as usize, r.get::<_, i64>(1)? as usize))
            })?
            .collect::<rusqlite::Result<_>>()?;
        drop(rows);
        drop(statement);
        drop(c);
        index.reindex();
        Ok(index)
    }
    /// Units, functions and symbols of `firmwares`, with families rebuilt among them.
    pub fn subset(&self, firmwares: &BTreeSet<String>) -> Result<Index> {
        let (mut index, _) = self.parts(Some(firmwares))?;
        index.build();
        Ok(index)
    }
    /// An unbuilt index of the selected firmwares (all when `None`) and the
    /// saved family of each loaded function.
    fn parts(&self, firmwares: Option<&BTreeSet<String>>) -> Result<(Index, Vec<Option<usize>>)> {
        let selected = |fw: &str| firmwares.is_none_or(|f| f.contains(fw));
        let c = self.connection();
        let mut units = Vec::new();
        {
            let mut statement = c.prepare("SELECT layout FROM units ORDER BY rowid")?;
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                let layout: UnitLayout = serde_json::from_str(&row.get::<_, String>(0)?)?;
                if selected(&layout.firmware) {
                    units.push(self.unit(&c, layout)?);
                }
            }
        }
        let tokens = c
            .prepare("SELECT text FROM tokens ORDER BY id")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        let query = format!(
            "SELECT {HEADER}, x.tokens, x.constants, x.ngrams, x.refs FROM functions f \
             JOIN function_features x ON x.id = f.id {} ORDER BY f.id",
            if firmwares.is_some() {
                "WHERE f.firmware = ?1"
            } else {
                ""
            }
        );
        let mut statement = c.prepare(&query)?;
        let mut loaded = Vec::<(usize, Option<usize>, Function)>::new();
        let selections: Vec<Option<&String>> = match firmwares {
            None => vec![None],
            Some(firmwares) => firmwares.iter().map(Some).collect(),
        };
        for firmware in selections {
            let mut rows = match firmware {
                None => statement.query([])?,
                Some(firmware) => statement.query([firmware])?,
            };
            while let Some(row) = rows.next()? {
                let entry = header(row)?;
                let refs: String = row.get(15)?;
                loaded.push((
                    entry.id,
                    entry.family,
                    Function {
                        header: entry.function.into_owned(),
                        tokens: decode_tokens(&row.get::<_, Vec<u8>>(12)?)?,
                        constants: decode_deltas(&row.get::<_, Vec<u8>>(13)?)?,
                        ngrams: decode_words(&row.get::<_, Vec<u8>>(14)?)?,
                        references: serde_json::from_str(&refs)?,
                        strings: BTreeSet::new(),
                    },
                ));
            }
        }
        loaded.sort_by_key(|(id, _, _)| *id);
        let position: HashMap<usize, usize> = loaded
            .iter()
            .enumerate()
            .map(|(p, (id, _, _))| (*id, p))
            .collect();
        {
            let mut statement = c.prepare("SELECT id, text FROM function_strings")?;
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                if let Some(&p) = position.get(&(row.get::<_, i64>(0)? as usize)) {
                    loaded[p].2.strings.insert(row.get(1)?);
                }
            }
        }
        let data_symbols = self
            .symbols_with(&c, None, None, None)?
            .into_iter()
            .filter(|s| selected(&s.firmware))
            .collect();
        let membership = loaded.iter().map(|(_, m, _)| *m).collect();
        let functions = loaded.into_iter().map(|(_, _, f)| f).collect();
        Ok((
            Index::from_parts(units, functions, data_symbols, tokens),
            membership,
        ))
    }
    fn unit(&self, c: &Connection, layout: UnitLayout) -> Result<Arc<Unit>> {
        let key = (layout.firmware.clone(), layout.unit.clone());
        if let Some(found) = self
            .units
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
        {
            return Ok(found.clone());
        }
        let regions = c
            .prepare(
                "SELECT address, bytes FROM regions WHERE firmware = ?1 AND unit = ?2 \
                 ORDER BY address",
            )?
            .query_map(params![layout.firmware, layout.unit], |r| {
                Ok(Region {
                    address: r.get::<_, i64>(0)? as u64,
                    bytes: r.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut regions = regions;
        regions.sort_by_key(|r| r.address);
        let unit = Arc::new(Unit { layout, regions });
        self.units
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, unit.clone());
        Ok(unit)
    }
    fn symbols_with(
        &self,
        c: &Connection,
        name: Option<&str>,
        firmware: Option<&str>,
        unit: Option<&str>,
    ) -> Result<Vec<DataSymbol>> {
        Ok(c.prepare_cached(
            "SELECT firmware, unit, address, length, name, kind, source FROM data_symbols \
                 WHERE (?1 IS NULL OR name = ?1) AND (?2 IS NULL OR firmware = ?2) \
                 AND (?3 IS NULL OR unit = ?3) ORDER BY rowid",
        )?
        .query_map(params![name, firmware, unit], |r| {
            Ok(DataSymbol {
                firmware: r.get(0)?,
                unit: r.get(1)?,
                address: r.get::<_, i64>(2)? as u64,
                length: r.get::<_, i64>(3)? as usize,
                name: r.get(4)?,
                kind: r.get(5)?,
                source: r.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?)
    }
    fn ids(&self, sql: &str, parameters: impl rusqlite::Params) -> Result<Vec<usize>> {
        Ok(self
            .connection()
            .prepare_cached(sql)?
            .query_map(parameters, |r| Ok(r.get::<_, i64>(0)? as usize))?
            .collect::<rusqlite::Result<_>>()?)
    }
}

impl Lookup for Store {
    type Error = Error;

    fn stats(&self) -> Result<Stats> {
        let c = self.connection();
        let firmwares: BTreeMap<String, usize> = c
            .prepare_cached("SELECT firmware, functions FROM firmwares")?
            .query_map([], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as usize)))?
            .collect::<rusqlite::Result<_>>()?;
        let families: i64 = c.query_row("SELECT count(*) FROM families", [], |r| r.get(0))?;
        Ok(Stats {
            functions: firmwares.values().sum(),
            families: families as usize,
            firmwares,
        })
    }
    fn entry(&self, id: usize) -> Result<Option<Entry<'_>>> {
        let c = self.connection();
        let mut statement =
            c.prepare_cached(&format!("SELECT {HEADER} FROM functions f WHERE f.id = ?1"))?;
        let mut rows = statement.query([id as i64])?;
        rows.next()?.map(header).transpose()
    }
    fn select(
        &self,
        firmware: Option<&str>,
        unit: Option<&str>,
        address: Option<u64>,
    ) -> Result<Vec<Entry<'_>>> {
        // Only present key parts become conditions, so SQLite can use the key indexes.
        let mut conditions = vec!["1".to_string()];
        let mut values = Vec::<rusqlite::types::Value>::new();
        for (column, value) in [
            ("firmware", firmware.map(|v| v.to_string().into())),
            ("unit", unit.map(|v| v.to_string().into())),
            ("address", address.map(|v| stored(v).into())),
        ] {
            if let Some(value) = value {
                values.push(value);
                conditions.push(format!("f.{column} = ?{}", values.len()));
            }
        }
        let c = self.connection();
        let mut statement = c.prepare_cached(&format!(
            "SELECT {HEADER} FROM functions f WHERE {} ORDER BY f.id",
            conditions.join(" AND ")
        ))?;
        let mut rows = statement.query(params_from_iter(values))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(header(row)?);
        }
        Ok(out)
    }
    fn named(&self, name: &str) -> Result<Vec<usize>> {
        self.ids(
            "SELECT id FROM function_names WHERE name = ?1 ORDER BY id",
            [name],
        )
    }
    fn family(&self, family: usize) -> Result<Option<Cow<'_, Family>>> {
        let annotations: Option<String> = self
            .connection()
            .prepare_cached("SELECT annotations FROM families WHERE id = ?1")?
            .query_row([family as i64], |r| r.get(0))
            .optional()?;
        let Some(annotations) = annotations else {
            return Ok(None);
        };
        Ok(Some(Cow::Owned(Family {
            id: family,
            members: self.ids(
                "SELECT id FROM functions WHERE family = ?1 ORDER BY id",
                [family as i64],
            )?,
            annotations: serde_json::from_str(&annotations)?,
        })))
    }
    fn weak_families(&self, family: usize) -> Result<Vec<usize>> {
        self.ids(
            "SELECT other FROM weak_families WHERE family = ?1 ORDER BY other",
            [family as i64],
        )
    }
    fn resolve(&self, caller: &FunctionKey, address: u64) -> Result<Option<usize>> {
        let local = self.ids(
            "SELECT id FROM functions WHERE firmware = ?1 AND unit = ?2 AND address = ?3",
            params![caller.firmware, caller.unit, stored(address)],
        )?;
        if let Some(&id) = local.first() {
            return Ok(Some(id));
        }
        let anywhere = self.ids(
            "SELECT id FROM functions WHERE firmware = ?1 AND address = ?2 LIMIT 2",
            params![caller.firmware, stored(address)],
        )?;
        Ok((anywhere.len() == 1).then(|| anywhere[0]))
    }
    fn function(&self, id: usize) -> Result<Option<Cow<'_, Function>>> {
        let Some(entry) = self.entry(id)? else {
            return Ok(None);
        };
        let c = self.connection();
        let (tokens, constants, ngrams, refs): (Vec<u8>, Vec<u8>, Vec<u8>, String) = c
            .prepare_cached(
                "SELECT tokens, constants, ngrams, refs FROM function_features WHERE id = ?1",
            )?
            .query_row([id as i64], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?;
        let strings = c
            .prepare_cached("SELECT text FROM function_strings WHERE id = ?1")?
            .query_map([id as i64], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(Some(Cow::Owned(Function {
            header: entry.function.into_owned(),
            tokens: decode_tokens(&tokens)?,
            constants: decode_deltas(&constants)?,
            ngrams: decode_words(&ngrams)?,
            references: serde_json::from_str(&refs)?,
            strings,
        })))
    }
    fn token(&self, token: u32) -> Result<String> {
        self.connection()
            .prepare_cached("SELECT text FROM tokens WHERE id = ?1")?
            .query_row([i64::from(token)], |r| r.get(0))
            .optional()?
            .ok_or_else(|| invalid("token"))
    }
    fn units(&self, firmware: &str, unit: Option<&str>) -> Result<Vec<Arc<Unit>>> {
        let c = self.connection();
        let layouts = c
            .prepare_cached(
                "SELECT layout FROM units WHERE firmware = ?1 AND (?2 IS NULL OR unit = ?2) \
                 ORDER BY rowid",
            )?
            .query_map(params![firmware, unit], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        layouts
            .iter()
            .map(|layout| self.unit(&c, serde_json::from_str(layout)?))
            .collect()
    }
    fn string_holders(&self, strings: &BTreeSet<String>) -> Result<Vec<(String, usize)>> {
        let mut out = Vec::new();
        for text in strings {
            for id in self.ids(
                "SELECT id FROM function_strings WHERE text = ?1 ORDER BY id",
                [text],
            )? {
                out.push((text.clone(), id));
            }
        }
        Ok(out)
    }
    fn referrers(
        &self,
        firmware: &str,
        unit: Option<&str>,
        address: u64,
    ) -> Result<Vec<(usize, Reference)>> {
        // A serialized reference spells its target as `"target":N,`; the
        // substring only preselects rows, the parsed target decides.
        let c = self.connection();
        let mut statement = c.prepare_cached(
            "SELECT f.id, x.refs FROM functions f JOIN function_features x ON x.id = f.id \
             WHERE f.firmware = ?1 AND (?2 IS NULL OR f.unit = ?2) AND instr(x.refs, ?3) > 0 \
             ORDER BY f.id",
        )?;
        let mut rows =
            statement.query(params![firmware, unit, format!("\"target\":{address},")])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let id = row.get::<_, i64>(0)? as usize;
            let references: Vec<Reference> = serde_json::from_str(&row.get::<_, String>(1)?)?;
            out.extend(
                references
                    .into_iter()
                    .filter(|r| r.target == address)
                    .map(|r| (id, r)),
            );
        }
        Ok(out)
    }
    fn symbols(
        &self,
        name: Option<&str>,
        firmware: Option<&str>,
        unit: Option<&str>,
    ) -> Result<Vec<DataSymbol>> {
        self.symbols_with(&self.connection(), name, firmware, unit)
    }
}
