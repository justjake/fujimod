//! JSON conversion only; matching and queries live in fujimod-corpus.
use fujimod_corpus::{self as core, Lookup};
use pyo3::{exceptions::PyValueError, prelude::*, types::PyModule};
use serde_json::Value;
use std::sync::OnceLock;
fn invalid(e: impl std::fmt::Display) -> PyErr {
    PyValueError::new_err(e.to_string())
}
/// A built or decoded index in memory, or a saved store whose lookups run as
/// SQL. A store loads the complete index once, for graph queries or changes.
enum Backend {
    Memory(core::Index),
    Stored {
        store: core::Store,
        loaded: OnceLock<core::Index>,
    },
}
#[pyclass(module = "fujimod.corpus")]
struct CorpusIndex {
    backend: Backend,
}
impl CorpusIndex {
    fn loaded(&self) -> PyResult<&core::Index> {
        match &self.backend {
            Backend::Memory(index) => Ok(index),
            Backend::Stored { store, loaded } => {
                if let Some(index) = loaded.get() {
                    return Ok(index);
                }
                let index = store.load().map_err(invalid)?;
                Ok(loaded.get_or_init(|| index))
            }
        }
    }
    fn memory(&mut self) -> PyResult<&mut core::Index> {
        if let Backend::Stored { store, loaded } = &mut self.backend {
            let index = match loaded.take() {
                Some(index) => index,
                None => store.load().map_err(invalid)?,
            };
            self.backend = Backend::Memory(index);
        }
        match &mut self.backend {
            Backend::Memory(index) => Ok(index),
            Backend::Stored { .. } => unreachable!(),
        }
    }
}
fn arguments<'a>(a: &'a Value) -> impl Fn(&str) -> Option<&'a str> {
    |key| a.get(key).and_then(Value::as_str)
}
fn id(a: &Value) -> PyResult<usize> {
    a.get("id")
        .and_then(Value::as_u64)
        .map(|v| v as usize)
        .ok_or_else(|| invalid("id is required"))
}
fn required<'a>(value: Option<&'a str>, name: &str) -> PyResult<&'a str> {
    value.ok_or_else(|| invalid(format!("{name} required")))
}
fn json<T: serde::Serialize, E: std::fmt::Display>(
    value: std::result::Result<T, E>,
) -> PyResult<Value> {
    serde_json::to_value(value.map_err(invalid)?).map_err(invalid)
}
/// Lookups answered by either backend; `None` for graph queries.
fn lookup<L: Lookup>(index: &L, operation: &str, a: &Value) -> PyResult<Option<Value>> {
    let s = arguments(a);
    let value = match operation {
        "stats" => json(index.stats()),
        "find" => json(index.find(
            s("name"),
            a.get("address").and_then(Value::as_u64),
            s("firmware"),
            s("unit"),
        )),
        "members" => json(index.members(id(a)?, s("firmware"))),
        "family" => json(index.family(id(a)?)),
        "string_frequencies" => {
            json(index.string_frequencies(
                &serde_json::from_value(a["strings"].clone()).map_err(invalid)?,
            ))
        }
        "body_sha256" => json(index.body_sha256(id(a)?)),
        "find_strings" => json(index.find_strings(
            &serde_json::from_value(a["strings"].clone()).map_err(invalid)?,
            s("firmware"),
        )),
        "read" => json(
            index.read(
                required(s("firmware"), "firmware")?,
                required(s("unit"), "unit")?,
                a["address"]
                    .as_u64()
                    .ok_or_else(|| invalid("address required"))?,
                a["size"].as_u64().ok_or_else(|| invalid("size required"))? as usize,
            ),
        ),
        "symbols" => json(index.symbols(s("name"), s("firmware"), s("unit"))),
        "features" => json(index.function(id(a)?)),
        "normalized_tokens" => json(index.normalized_tokens(id(a)?)),
        "calls" => json(index.calls(id(a)?)),
        "data_references" => json(index.data_references(id(a)?)),
        "references_to" => json(
            index.references_to(
                required(s("firmware"), "firmware")?,
                s("unit"),
                a["address"]
                    .as_u64()
                    .ok_or_else(|| invalid("address required"))?,
            ),
        ),
        "names" => {
            let donors = a
                .get("donors")
                .filter(|v| !v.is_null())
                .map(|v| serde_json::from_value(v.clone()))
                .transpose()
                .map_err(invalid)?;
            json(
                index.inherited_names(
                    id(a)?,
                    donors.as_ref(),
                    a.get("normalized_only")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                ),
            )
        }
        "search" => {
            let pattern: Vec<u8> = serde_json::from_value(a["pattern"].clone()).map_err(invalid)?;
            let mask: Option<Vec<u8>> = a
                .get("mask")
                .map(|v| serde_json::from_value(v.clone()))
                .transpose()
                .map_err(invalid)?
                .flatten();
            let pattern = core::BytePattern::new(pattern, mask).map_err(invalid)?;
            json(index.search(required(s("firmware"), "firmware")?, s("unit"), &pattern))
        }
        _ => return Ok(None),
    };
    value.map(Some)
}
/// Queries over the whole family graph of a loaded index.
fn graph(index: &core::Index, operation: &str, a: &Value) -> PyResult<Value> {
    let s = arguments(a);
    let value = match operation {
        "candidates" => {
            serde_json::to_value(index.candidates(id(a)?, required(s("firmware"), "firmware")?))
        }
        "name_support" => serde_json::to_value(
            index.name_support(
                id(a)?,
                a["other"]
                    .as_u64()
                    .ok_or_else(|| invalid("other required"))? as usize,
            ),
        ),
        "sample_links" => serde_json::to_value(index.sample_links(
            a["count"].as_u64().unwrap_or(50) as usize,
            a["seed"].as_u64().unwrap_or(0),
        )),
        "data_witnesses" => serde_json::to_value(
            index.data_witnesses(id(a)?, a["length"].as_u64().unwrap_or(64) as usize),
        ),
        "pairs" => serde_json::to_value(index.pairs(
            required(s("reference"), "reference")?,
            required(s("other"), "other")?,
        )),
        "pairs_for" => {
            serde_json::to_value(index.pairs_for(id(a)?, required(s("other"), "other")?))
        }
        _ => return Err(invalid("unknown corpus query")),
    };
    value.map_err(invalid)
}
#[pymethods]
impl CorpusIndex {
    #[new]
    fn new() -> Self {
        Self {
            backend: Backend::Memory(core::Index::new()),
        }
    }
    /// Opens a saved store without loading it.
    #[staticmethod]
    fn open(py: Python<'_>, path: String) -> PyResult<Self> {
        let store = py.detach(|| core::Store::open(path)).map_err(invalid)?;
        Ok(Self {
            backend: Backend::Stored {
                store,
                loaded: OnceLock::new(),
            },
        })
    }
    fn add_unit(
        &mut self,
        py: Python<'_>,
        metadata: &str,
        regions: Vec<(u64, Vec<u8>)>,
    ) -> PyResult<()> {
        let input = serde_json::from_str(metadata).map_err(invalid)?;
        let regions = regions
            .into_iter()
            .map(|(address, bytes)| core::Region { address, bytes })
            .collect();
        py.detach(|| self.memory()?.add_unit(input, regions).map_err(invalid))
    }
    fn build(&mut self, py: Python<'_>) -> PyResult<()> {
        py.detach(|| {
            self.memory()?.build();
            Ok(())
        })
    }
    fn save(&self, py: Python<'_>, path: String) -> PyResult<()> {
        py.detach(|| self.loaded()?.save(path).map_err(invalid))
    }
    fn merge(&mut self, py: Python<'_>, other: &Self) -> PyResult<()> {
        py.detach(|| {
            let other = other.loaded()?.clone();
            self.memory()?.merge(other).map_err(invalid)
        })
    }
    fn subset(&self, py: Python<'_>, firmwares: Vec<String>) -> PyResult<Self> {
        let firmwares = firmwares.into_iter().collect();
        let index = py.detach(|| match &self.backend {
            Backend::Stored { store, loaded } if loaded.get().is_none() => {
                store.subset(&firmwares).map_err(invalid)
            }
            _ => Ok(self.loaded()?.subset(&firmwares)),
        })?;
        Ok(Self {
            backend: Backend::Memory(index),
        })
    }
    fn query(&self, py: Python<'_>, operation: &str, arguments: &str) -> PyResult<String> {
        let a: Value = serde_json::from_str(arguments).map_err(invalid)?;
        py.detach(|| -> PyResult<String> {
            let answered = match &self.backend {
                Backend::Memory(index) => lookup(index, operation, &a)?,
                Backend::Stored { store, loaded } => match loaded.get() {
                    Some(index) => lookup(index, operation, &a)?,
                    None => lookup(store, operation, &a)?,
                },
            };
            let value = match answered {
                Some(value) => value,
                None => graph(self.loaded()?, operation, &a)?,
            };
            serde_json::to_string(&value).map_err(invalid)
        })
    }
    fn add_symbol(&mut self, symbol: &str) -> PyResult<()> {
        let symbol = serde_json::from_str(symbol).map_err(invalid)?;
        self.memory()?.add_symbol(symbol).map_err(invalid)
    }
    fn add_name(&mut self, id: usize, name: String, source: String) -> PyResult<()> {
        self.memory()?.add_name(id, name, source).map_err(invalid)
    }
    fn annotate(&mut self, family: usize, annotation: &str) -> PyResult<()> {
        let annotation = serde_json::from_str(annotation).map_err(invalid)?;
        self.memory()?.annotate(family, annotation).map_err(invalid)
    }
}
#[pyfunction]
fn align_sequences(
    py: Python<'_>,
    left: Vec<String>,
    right: Vec<String>,
) -> Vec<(Option<usize>, Option<usize>)> {
    py.detach(|| fujimod_corpus::align_sequences(&left, &right))
}

pub(crate) fn register(py: Python<'_>, parent: &Bound<'_, PyModule>) -> PyResult<()> {
    let module = PyModule::new(py, "corpus")?;
    module.add_class::<CorpusIndex>()?;
    module.add_function(wrap_pyfunction!(align_sequences, &module)?)?;
    parent.add_submodule(&module)?;
    Ok(())
}
