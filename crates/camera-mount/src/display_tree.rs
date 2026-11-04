//! Virtual hardware outputs and bounded, immutable image reads while open.
use crate::{Device, Node, ROOT, Result};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};
pub const DISPLAY: u64 = 8;
const MAX_BYTES: usize = 128 << 20;
#[derive(Clone)]
enum Entry {
    Root,
    Output(u32),
    Layers(u32),
    Image { output: u32, name: String },
}
pub struct Tree {
    nodes: BTreeMap<u64, (Node, Entry)>,
    bytes: BTreeMap<u64, Vec<u8>>,
    opens: BTreeMap<u64, usize>,
    listings: BTreeMap<u64, (Instant, u64, Vec<u64>)>,
    next: u64,
    generation: u64,
}
impl Tree {
    pub fn new(outputs: Vec<u32>) -> Self {
        let mut tree = Self {
            nodes: BTreeMap::new(),
            bytes: BTreeMap::new(),
            opens: BTreeMap::new(),
            listings: BTreeMap::new(),
            next: 1 << 40,
            generation: 1,
        };
        if outputs.is_empty() {
            return tree;
        }
        tree.insert(DISPLAY, ROOT, "display".into(), Entry::Root, 0);
        for out in outputs {
            let id = tree.next();
            tree.insert(id, DISPLAY, out.to_string(), Entry::Output(out), 0);
            let child = tree.next();
            tree.insert(child, id, "layer".into(), Entry::Layers(out), 0);
        }
        tree
    }
    fn next(&mut self) -> u64 {
        self.next += 1;
        self.next
    }
    fn insert(&mut self, id: u64, parent: u64, name: String, entry: Entry, size: u64) {
        let directory = !matches!(entry, Entry::Image { .. });
        self.nodes.insert(
            id,
            (
                Node {
                    id,
                    parent,
                    name,
                    path: None,
                    size,
                    directory,
                    writable: false,
                },
                entry,
            ),
        );
    }
    pub fn node(&self, id: u64) -> Option<Node> {
        self.nodes.get(&id).map(|(n, _)| n.clone())
    }
    pub fn contains(&self, id: u64) -> bool {
        self.nodes.contains_key(&id)
    }
    fn store(&mut self, id: u64, bytes: Vec<u8>) -> Result<()> {
        let previous = self.bytes.get(&id).map_or(0, Vec::len);
        let used: usize = self.bytes.values().map(Vec::len).sum();
        if used - previous + bytes.len() > MAX_BYTES {
            return Err(libc::ENOMEM);
        }
        self.nodes.get_mut(&id).ok_or(libc::ESTALE)?.0.size = bytes.len() as u64;
        self.bytes.insert(id, bytes);
        Ok(())
    }
    pub fn directory(
        &mut self,
        device: &mut impl Device,
        id: u64,
        continuation: bool,
    ) -> Result<(u64, Vec<Node>)> {
        let entry = self.nodes.get(&id).ok_or(libc::ESTALE)?.1.clone();
        let output = match entry {
            Entry::Root => None,
            Entry::Output(o) | Entry::Layers(o) => Some(o),
            Entry::Image { .. } => return Err(libc::ENOTDIR),
        };
        if let Some(output) = output {
            let cached = self.listings.get(&id).is_some_and(|(when, _, _)| {
                continuation || when.elapsed() < Duration::from_millis(250)
            });
            if !cached {
                let images = device.display_images(output)?;
                let mut ids = Vec::new();
                if matches!(entry, Entry::Output(_)) {
                    ids.extend(
                        self.nodes
                            .iter()
                            .filter(|(_, (n, e))| n.parent == id && matches!(e, Entry::Layers(_)))
                            .map(|(i, _)| *i),
                    );
                }
                for image in images {
                    let name = match &entry {
                        Entry::Output(_) => {
                            if image.name.contains('/') {
                                continue;
                            }
                            image.name.clone()
                        }
                        Entry::Layers(_) => {
                            let Some(n) = image.name.strip_prefix("layer/") else {
                                continue;
                            };
                            n.to_owned()
                        }
                        _ => unreachable!(),
                    };
                    if name.contains('/') || name == "." || name == ".." {
                        return Err(libc::EIO);
                    }
                    let child = self.next();
                    self.insert(
                        child,
                        id,
                        name,
                        Entry::Image {
                            output,
                            name: image.name,
                        },
                        image.bytes.len() as u64,
                    );
                    self.store(child, image.bytes)?;
                    ids.push(child);
                }
                self.generation += 1;
                self.listings
                    .insert(id, (Instant::now(), self.generation, ids));
            }
            let (_, generation, ids) = self.listings.get(&id).ok_or(libc::EIO)?;
            Ok((
                *generation,
                ids.iter().filter_map(|id| self.node(*id)).collect(),
            ))
        } else {
            Ok((
                1,
                self.nodes
                    .values()
                    .filter(|(n, _)| n.parent == id)
                    .map(|(n, _)| n.clone())
                    .collect(),
            ))
        }
    }
    pub fn open(&mut self, device: &mut impl Device, id: u64) -> Result<()> {
        let Some((_, Entry::Image { output, name })) = self.nodes.get(&id) else {
            return Ok(());
        };
        if self.opens.get(&id).copied().unwrap_or(0) == 0 {
            let output = *output;
            let name = name.clone();
            let bytes = device
                .display_images(output)?
                .into_iter()
                .find(|i| i.name == name)
                .ok_or(libc::ENOENT)?
                .bytes;
            self.store(id, bytes)?;
        }
        *self.opens.entry(id).or_default() += 1;
        Ok(())
    }
    pub fn close(&mut self, id: u64) {
        if let Some(n) = self.opens.get_mut(&id) {
            *n = n.saturating_sub(1);
        }
    }
    pub fn reclaim(&mut self, id: u64) {
        if self.opens.get(&id).copied().unwrap_or(0) == 0
            && matches!(self.nodes.get(&id), Some((_, Entry::Image { .. })))
        {
            self.nodes.remove(&id);
            self.bytes.remove(&id);
            self.opens.remove(&id);
            self.listings.clear();
        }
    }
    pub fn read(&self, id: u64, offset: u64, size: usize) -> Result<Vec<u8>> {
        let data = self.bytes.get(&id).ok_or(libc::EISDIR)?;
        let start = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(data.len());
        Ok(data[start..start.saturating_add(size).min(data.len())].to_vec())
    }
}
