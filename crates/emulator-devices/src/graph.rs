//! CPU-independent construction of physical device graphs.
use crate::Device;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceWindow {
    pub base: u64,
    pub size: u64,
    pub kind: String,
    #[serde(default)]
    pub options: toml::Table,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Mapping {
    pub base: u64,
    pub size: u64,
}
pub struct Node {
    pub mappings: Vec<Mapping>,
    pub device: Box<dyn Device>,
    pub native_dependencies: Vec<usize>,
}
#[derive(Default)]
pub struct DeviceGraph {
    pub nodes: Vec<Node>,
}
impl DeviceGraph {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn register(
        &mut self,
        base: u64,
        size: u64,
        device: Box<dyn Device>,
    ) -> Result<usize, String> {
        self.register_windows(vec![Mapping { base, size }], device)
    }
    pub fn register_windows(
        &mut self,
        mappings: Vec<Mapping>,
        device: Box<dyn Device>,
    ) -> Result<usize, String> {
        validate_mappings(&mappings, self.nodes.iter().flat_map(|n| &n.mappings))?;
        Ok(self.insert(mappings, device))
    }
    pub fn attach(&mut self, device: Box<dyn Device>) -> usize {
        self.insert(vec![], device)
    }
    fn insert(&mut self, mappings: Vec<Mapping>, device: Box<dyn Device>) -> usize {
        let id = self.nodes.len();
        self.nodes.push(Node {
            mappings,
            device,
            native_dependencies: vec![],
        });
        id
    }
    pub fn at(&self, address: u64) -> Result<usize, String> {
        self.nodes
            .iter()
            .position(|n| {
                n.mappings
                    .iter()
                    .any(|m| address >= m.base && address < m.base + m.size)
            })
            .ok_or_else(|| format!("no device at {address:#x}"))
    }
    pub fn depend_on(&mut self, device: usize, sources: Vec<usize>) -> Result<(), String> {
        if device >= self.nodes.len()
            || sources
                .iter()
                .any(|&s| s >= self.nodes.len() || s == device)
            || sources.iter().copied().collect::<BTreeSet<_>>().len() != sources.len()
        {
            return Err("invalid native device dependencies".into());
        }
        self.nodes[device].native_dependencies = sources;
        Ok(())
    }
}

/// Validate a whole node before adding any of its windows to a graph or bus.
pub fn validate_mappings<'a>(
    mappings: &[Mapping],
    existing: impl Iterator<Item = &'a Mapping> + Clone,
) -> Result<(), String> {
    for (index, m) in mappings.iter().enumerate() {
        let end = m
            .base
            .checked_add(m.size)
            .filter(|_| m.size != 0)
            .ok_or("invalid physical device range")?;
        let overlaps = |p: &Mapping| m.base < p.base + p.size && p.base < end;
        if existing.clone().any(overlaps) || mappings[..index].iter().any(overlaps) {
            return Err(format!(
                "overlapping physical device range {:#x}..{end:#x}",
                m.base
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn registers(base: u64) -> Box<dyn Device> {
        Box::new(crate::keyed_io::KeyedIoGroup::new(base))
    }
    #[test]
    fn physical_graph_rejects_ambiguous_ranges_and_invalid_dependency_edges() {
        let mut graph = DeviceGraph::new();
        let source = graph.attach(registers(0));
        let first = graph.register(0x1000, 0x100, registers(0x1000)).unwrap();
        let second = graph.register(0x1100, 0x100, registers(0x1100)).unwrap();
        assert!(graph.register(0x10ff, 2, registers(0x10ff)).is_err());
        assert!(graph.register(0, u64::MAX, registers(0)).is_err());
        assert!(graph.register(u64::MAX, 1, registers(0)).is_err());
        assert!(graph.register(0, 0, registers(0)).is_err());
        assert_eq!(graph.at(0x10ff).unwrap(), first);
        assert_eq!(graph.at(0x1100).unwrap(), second);
        assert!(graph.at(0).is_err());
        assert!(graph.depend_on(first, vec![first]).is_err());
        assert!(graph.depend_on(first, vec![source, source]).is_err());
        assert!(graph.depend_on(first, vec![3]).is_err());
        graph.depend_on(first, vec![source, second]).unwrap();
        assert!(graph.nodes[source].mappings.is_empty());
        assert_eq!(graph.nodes[first].native_dependencies, vec![source, second]);
    }
    #[test]
    fn multiple_windows_keep_one_owner_and_validate_atomically() {
        let mut graph = DeviceGraph::new();
        let id = graph
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
                        base: 0x5000,
                        size: 4,
                    },
                ],
                registers(0),
            )
            .unwrap();
        for address in [0x1000, 0x1013, 0x5000] {
            assert_eq!(graph.at(address).unwrap(), id);
        }
        assert!(graph.at(0x1004).is_err());
        assert!(
            graph
                .register_windows(
                    vec![
                        Mapping {
                            base: 0x9000,
                            size: 4
                        },
                        Mapping {
                            base: 0x1000,
                            size: 4
                        },
                    ],
                    registers(0)
                )
                .is_err()
        );
        assert_eq!(graph.nodes.len(), 1);
        assert!(graph.at(0x9000).is_err());
        assert!(
            graph
                .register_windows(
                    vec![
                        Mapping {
                            base: 0x9000,
                            size: 8
                        },
                        Mapping {
                            base: 0x9004,
                            size: 4
                        },
                    ],
                    registers(0)
                )
                .is_err()
        );
        assert_eq!(graph.nodes.len(), 1);
    }
}
