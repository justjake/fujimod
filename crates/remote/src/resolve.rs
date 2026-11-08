//! Resolve each reference anew against the live descriptor graph and cursor.
use crate::{
    ControlSnapshot, Menu, MenuSnapshot, Reference, RemoteReadResult, Result, Value,
    backend::Backend,
    memory::{Memory, checked_indexed_address, word},
};
use fujimod_catalog_controls::InputTarget;
use fujimod_catalog_remote::RootLayout;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

fn field_u64(bytes: &[u8], offset: u32, width: u8) -> Result<u64> {
    if !matches!(width, 1 | 2 | 4 | 8) {
        return Err("Unsupported menu field width".into());
    }
    let start = usize::try_from(offset).map_err(|_| "Menu field offset exceeds host address space")?;
    let end = start
        .checked_add(usize::from(width))
        .ok_or("Menu field extent overflow")?;
    let field = bytes
        .get(start..end)
        .ok_or("Menu field exceeds its descriptor")?;
    Ok(field
        .iter()
        .enumerate()
        .fold(0u64, |value, (index, byte)| {
            value | (u64::from(*byte) << (index * 8))
        }))
}

fn field_u32(bytes: &[u8], offset: u32, width: u8) -> Result<u32> {
    u32::try_from(field_u64(bytes, offset, width)?)
        .map_err(|_| "Menu scalar exceeds 32 bits".into())
}

fn root_key(bytes: &[u8], offset: usize, width: u8) -> Result<u32> {
    if !matches!(width, 1 | 2 | 4) {
        return Err("Unsupported root key width".into());
    }
    let end = offset
        .checked_add(usize::from(width))
        .ok_or("root key extent overflow")?;
    let field = bytes
        .get(offset..end)
        .ok_or("root key exceeds its entry")?;
    let mut value = 0u32;
    for (shift, byte) in field.iter().enumerate() {
        value |= u32::from(*byte) << (shift * 8);
    }
    Ok(value)
}

fn root_pointer(bytes: &[u8], offset: usize) -> Result<u32> {
    let offset = u32::try_from(offset).map_err(|_| "root pointer offset exceeds 32 bits")?;
    word(bytes, offset)
}

#[derive(Clone, Debug)]
struct Row {
    key: u32,
    child: u64,
    index: u32,
}
#[derive(Clone, Debug)]
struct Node {
    pointer: u64,
    code: u32,
    parent: u64,
    captions: u64,
    rows: Vec<Row>,
}
impl<B: Backend> Memory<'_, B> {
    async fn node(&mut self, pointer: u64, roots: &BTreeMap<u32, u64>) -> Result<Node> {
        let l = &self.facts.firmware.menus.layout;
        l.validate()?;
        let raw = self.bytes(pointer, l.size as usize).await?;
        let code = field_u32(&raw, l.fields.menu_id, l.field_widths.menu_id)?;
        let type_code = field_u32(&raw, l.fields.type_code, l.field_widths.type_code)?;
        let _setting_id = field_u32(&raw, l.fields.setting_id, l.field_widths.setting_id)?;
        let rows_per_page = field_u32(&raw, l.fields.rows_per_page, l.field_widths.rows_per_page)?;
        let _flags_word = field_u32(&raw, l.fields.flags_word, l.field_widths.flags_word)?;
        if !roots.contains_key(&code) || type_code > l.max_kind || rows_per_page > 1024 {
            return Err(format!("Invalid menu descriptor at {pointer:#x}"));
        }
        let array = field_u64(&raw, l.fields.item_array, l.field_widths.item_array)?;
        let mut rows = vec![];
        if array != 0 {
            for index in 0..=1024 {
                let row_address = checked_indexed_address(
                    array,
                    u64::from(index),
                    u64::from(l.row_size),
                    0,
                )?;
                let raw = self.bytes(row_address, l.row_size as usize).await?;
                let key = field_u32(&raw, l.row_key, l.row_key_width)?;
                let child = field_u64(&raw, l.row_child, l.row_child_width)?;
                if key == l.sentinel && child == 0 {
                    break;
                }
                if index == 1024 {
                    return Err(format!("Unterminated rows at {pointer:#x}"));
                }
                rows.push(Row { key, child, index });
            }
        }
        Ok(Node {
            pointer,
            code,
            parent: field_u64(&raw, l.fields.parent_pointer, l.field_widths.parent_pointer)?,
            captions: field_u64(&raw, l.fields.handlers_pointer, l.field_widths.handlers_pointer)?,
            rows,
        })
    }
    async fn graph(&mut self) -> Result<(BTreeMap<u32, u64>, BTreeMap<u64, Node>)> {
        let m = &self.facts.firmware.menus;
        m.layout.validate()?;
        if m.root_count > 4096 {
            return Err("Menu root bound exceeded".into());
        }
        let (_, table_size) = m.checked_root_table_extent()?;
        let raw = self.bytes(m.roots_address, table_size).await?;
        let mut roots = BTreeMap::new();
        match &m.root_layout {
            RootLayout::PointerArray => {
                for index in 0..m.root_count {
                    let offset = usize::try_from(index)
                        .map_err(|_| "root index exceeds host address space")?
                        .checked_mul(4)
                        .ok_or("root pointer offset overflow")?;
                    roots.insert(index, u64::from(root_pointer(&raw, offset)?));
                }
            }
            RootLayout::KeyedEntries {
                stride,
                key_offset,
                key_width,
                pointer_offset,
            } => {
                let stride = usize::try_from(*stride)
                    .map_err(|_| "root stride exceeds host address space")?;
                let key_offset = usize::try_from(*key_offset)
                    .map_err(|_| "root key offset exceeds host address space")?;
                let pointer_offset = usize::try_from(*pointer_offset)
                    .map_err(|_| "root pointer offset exceeds host address space")?;
                for index in 0..m.root_count {
                    let entry = usize::try_from(index)
                        .map_err(|_| "root index exceeds host address space")?
                        .checked_mul(stride)
                        .ok_or("root entry offset overflow")?;
                    let key_at = entry
                        .checked_add(key_offset)
                        .ok_or("root key offset overflow")?;
                    let pointer_at = entry
                        .checked_add(pointer_offset)
                        .ok_or("root pointer offset overflow")?;
                    let key = root_key(&raw, key_at, *key_width)?;
                    let pointer = u64::from(root_pointer(&raw, pointer_at)?);
                    if roots.insert(key, pointer).is_some() {
                        return Err(format!("Duplicate root key {key:#x}"));
                    }
                }
            }
        }
        let root_pointers: BTreeSet<_> = roots
            .values()
            .copied()
            .filter(|pointer| *pointer != 0)
            .collect();
        let mut pending: VecDeque<_> = root_pointers.iter().copied().collect();
        let mut nodes = BTreeMap::new();
        let mut visited = BTreeSet::new();
        while let Some(pointer) = pending.pop_front() {
            if !visited.insert(pointer) {
                continue;
            }
            if visited.len() > 8192 {
                return Err("Live menu graph exceeds descriptor bound".into());
            }
            // Function rows may contain operation values rather than descriptors.
            // A root, or a descriptor explicitly requested later, must validate.
            let node = match self.node(pointer, &roots).await {
                Ok(node) => node,
                Err(error) if root_pointers.contains(&pointer) => return Err(error),
                Err(_) => continue,
            };
            pending.extend(node.rows.iter().filter(|r| r.child != 0).map(|r| r.child));
            nodes.insert(pointer, node);
        }
        Ok((roots, nodes))
    }

    #[cfg(test)]
    pub(crate) async fn validate_graph(&mut self) -> Result<()> {
        self.graph().await.map(|_| ())
    }
    fn root_id(&self, code: u32) -> String {
        self.facts
            .firmware
            .menus
            .roles
            .iter()
            .find(|(_, n)| **n == code)
            .map(|(name, _)| name.clone())
            .unwrap_or_else(|| code.to_string())
    }
    fn code(&self, id: &str) -> Result<u32> {
        self.facts
            .firmware
            .menus
            .roles
            .get(id)
            .copied()
            .or_else(|| id.parse().ok())
            .ok_or_else(|| {
                format!(
                    "Unknown menu {id:?}; known roles: {:?}",
                    self.facts.firmware.menus.roles.keys().collect::<Vec<_>>()
                )
            })
    }
    fn target(
        &self,
        reference: &Menu,
        roots: &BTreeMap<u32, u64>,
        nodes: &BTreeMap<u64, Node>,
    ) -> Result<(u64, Option<u32>)> {
        let mut chain = vec![];
        let mut current = reference;
        while let Some(parent) = current.parent() {
            chain.push(current);
            current = parent;
            if chain.len() > 64 {
                return Err(format!("Menu {} via chain exceeds bound", reference.id()));
            }
        }
        let declarations: Vec<_> = self
            .facts
            .menu_manifest
            .iter()
            .flat_map(|m| &m.items)
            .filter(|i| i.name == current.id() || i.identity == current.id())
            .collect();
        let (mut pointer, mut key) = if declarations.is_empty() {
            let code = self.code(current.id())?;
            let pointer = *roots.get(&code).ok_or_else(|| {
                format!("Menu {} root key {code:#x} is absent from the root table", current.id())
            })?;
            (pointer, current.key())
        } else if declarations.len() == 1 {
            let item = declarations[0];
            if current.key().is_some() {
                return Err(format!(
                    "Mod menu {} already declares its row key",
                    current.id()
                ));
            }
            if let Some(stock) = item.stock_node {
                (stock, None)
            } else {
                let candidates: Vec<_> = nodes
                    .values()
                    .filter(|n| {
                        item.parent == n.pointer
                            || item.parent_item != 0
                                && self.facts.menu_manifest.as_ref().is_some_and(|m| {
                                    m.items.iter().any(|p| {
                                        p.address == item.parent_item
                                            && (p.page == n.pointer || p.open_page == n.pointer)
                                    })
                                })
                    })
                    .filter(|n| n.rows.iter().any(|r| r.key == item.key))
                    .collect();
                if candidates.len() != 1 {
                    return Err(format!(
                        "Menu {}: expected one live parent, found {:?}",
                        current.id(),
                        candidates.iter().map(|n| n.pointer).collect::<Vec<_>>()
                    ));
                }
                (candidates[0].pointer, Some(item.key))
            }
        } else {
            return Err(format!(
                "Menu {} matches {} declarations; use its full identity",
                current.id(),
                declarations.len()
            ));
        };
        for next in chain.into_iter().rev() {
            let node = nodes.get(&pointer).ok_or_else(|| {
                format!(
                    "Menu {} has no live descriptor at {pointer:#x}",
                    reference.id()
                )
            })?;
            let row = unique_row(
                node,
                key.ok_or("via must identify an opening row with key")?,
                reference.id(),
            )?;
            pointer = row.child;
            let child = nodes.get(&pointer).ok_or_else(|| {
                format!(
                    "Menu {} via points to absent descriptor {pointer:#x}",
                    reference.id()
                )
            })?;
            if child.code != self.code(next.id())? {
                return Err(format!(
                    "Menu {} via opens menu {}, not {}",
                    reference.id(),
                    child.code,
                    next.id()
                ));
            }
            key = next.key();
        }
        if !nodes.contains_key(&pointer) {
            return Err(format!(
                "Menu {} has no live descriptor at {pointer:#x}",
                reference.id()
            ));
        }
        Ok((pointer, key))
    }
    fn ancestry(
        &self,
        pointer: u64,
        roots: &BTreeMap<u32, u64>,
        nodes: &BTreeMap<u64, Node>,
        reference: &Menu,
    ) -> Result<(String, Vec<Menu>)> {
        if let Some(via) = reference.parent() {
            let (parent, _) = self.target(via, roots, nodes)?;
            let (root, mut ancestors) = self.ancestry(parent, roots, nodes, via)?;
            ancestors.push(via.clone());
            return Ok((root, ancestors));
        }
        let mut current = pointer;
        let mut reverse = vec![];
        let mut seen = BTreeSet::new();
        loop {
            if !seen.insert(current) {
                return Err(format!("Menu {} has a parent cycle", reference.id()));
            }
            let node = nodes
                .get(&current)
                .ok_or("Menu ancestry has no live descriptor")?;
            if roots.get(&node.code) == Some(&current)
                && self
                    .facts
                    .firmware
                    .menus
                    .root_routes
                    .contains_key(&node.code)
            {
                reverse.reverse();
                let mut ancestors = vec![];
                let mut via = None;
                for (code, key) in reverse {
                    let mut item = Menu::new(self.root_id(code)).with_key(key);
                    if let Some(parent) = via {
                        item = item.via(parent);
                    }
                    via = Some(item.clone());
                    ancestors.push(item);
                }
                return Ok((self.root_id(node.code), ancestors));
            }
            let incoming: Vec<_> = nodes
                .values()
                .flat_map(|n| {
                    n.rows
                        .iter()
                        .filter(move |r| r.child == current)
                        .map(move |r| (n, r))
                })
                .collect();
            let preferred: Vec<_> = incoming
                .iter()
                .copied()
                .filter(|(n, _)| n.pointer == node.parent)
                .collect();
            let choices = if preferred.is_empty() {
                incoming
            } else {
                preferred
            };
            if choices.len() != 1 {
                return Err(format!(
                    "Menu {} at {current:#x}: found {} opening rows; supply via",
                    reference.id(),
                    choices.len()
                ));
            }
            let (parent, row) = choices[0];
            // The full via path is rebuilt below, avoiding address-based references.
            reverse.push((parent.code, row.key));
            current = parent.pointer;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_pointer_values_are_preserved_and_scalars_are_checked() {
        let pointer = 0x0102_0304_0506_0708u64.to_le_bytes();
        assert_eq!(field_u64(&pointer, 0, 8).unwrap(), 0x0102_0304_0506_0708);
        assert!(field_u32(&pointer, 0, 8).is_err());
    }

    #[test]
    fn malformed_root_layouts_refuse_before_reading_memory() {
        futures_lite::future::block_on(async {
            let mut bad_width = crate::testing::facts();
            bad_width.firmware.menus.root_layout = RootLayout::KeyedEntries {
                stride: 8,
                key_offset: 0,
                key_width: 0,
                pointer_offset: 4,
            };
            let mut bad_stride = crate::testing::facts();
            bad_stride.firmware.menus.root_layout = RootLayout::KeyedEntries {
                stride: 0,
                key_offset: 0,
                key_width: 4,
                pointer_offset: 4,
            };
            let mut wrapped_extent = crate::testing::facts();
            wrapped_extent.firmware.menus.root_layout = RootLayout::KeyedEntries {
                stride: 8,
                key_offset: 0,
                key_width: 4,
                pointer_offset: 4,
            };
            wrapped_extent.firmware.menus.roots_address = u64::MAX - 3;

            for facts in [bad_width, bad_stride, wrapped_extent] {
                let mut backend = crate::testing::Synthetic::new();
                let mut memory = Memory {
                    backend: &mut backend,
                    facts: &facts,
                    reads: crate::ReadSet::default(),
                    scope: false,
                    depth: 0,
                };
                assert!(memory.graph().await.is_err());
                assert!(memory.reads.stores.is_empty());
            }
        });
    }
}
fn unique_row<'a>(node: &'a Node, key: u32, id: &str) -> Result<&'a Row> {
    let matches: Vec<_> = node.rows.iter().filter(|r| r.key == key).collect();
    if matches.len() != 1 {
        return Err(format!(
            "Menu {id} at {:#x}: key {key:#x} matched {} rows; found {:?}",
            node.pointer,
            matches.len(),
            node.rows.iter().map(|r| r.key).collect::<Vec<_>>()
        ));
    }
    Ok(matches[0])
}
impl<B: Backend> Memory<'_, B> {
    async fn caption(&mut self, node: &Node, row: &Row) -> Result<String> {
        let width = self.facts.firmware.menus.layout.caption_words;
        if node.captions == 0 {
            return Ok(String::new());
        }
        if width == 0 || width > 64 {
            return Err("Invalid caption extent".into());
        }
        let raw = self
            .bytes(
                checked_indexed_address(
                    node.captions,
                    u64::from(row.index),
                    u64::from(width) * 4,
                    0,
                )?,
                width as usize * 4,
            )
            .await?;
        let label = String::new();
        // Caption records store the compact summary first and the full menu
        // label second. Prefer the full label; summaries can be only an icon
        // or an abbreviation (for example N versus NORMAL).
        for offset in (0..width.min(2)).rev().map(|n| n * 4) {
            let resource = word(&raw, offset)?;
            if resource == 0 || resource == 0x7ffffffe || resource >= 0xff000000 {
                continue;
            }
            let text = self.text(resource).await?;
            if !text.is_empty() {
                return Ok(text);
            }
        }
        Ok(label)
    }
    pub async fn read_reference(&mut self, reference: &Reference) -> Result<RemoteReadResult> {
        self.begin_observation()?;
        let result = self.read_reference_inner(reference).await;
        let mut value = self.finish(result)?;
        value.reads = self.reads.clone();
        Ok(value)
    }
    async fn read_reference_inner(&mut self, reference: &Reference) -> Result<RemoteReadResult> {
        let state = self.state().await?;
        if let Reference::Control(control) = reference {
            let id = control.id();
            self.facts.physical_controls.control(id)?;
            let supported = self.facts.control_ids.inputs.iter().any(|input| {
                (input.electrical.is_some() || input.debug_key.is_some())
                    && matches!(&input.target, InputTarget::Control { control_id }
                        if control_id == id
                            || control_id.strip_prefix(id).is_some_and(|rest| rest.starts_with('.')))
            }) || self
                .facts
                .physical_controls
                .touch(&self.facts.control_ids, id)
                .is_ok();
            return Ok(RemoteReadResult {
                state,
                value: Value::Control(ControlSnapshot {
                    id: id.into(),
                    supported,
                    position: self.backend.control_position(id)?,
                }),
                reads: self.reads.clone(),
            });
        }
        let item = match reference {
            Reference::Menu(item) => item,
            Reference::Option(option) => option.item(),
            _ => unreachable!(),
        };
        let (roots, nodes) = self.graph().await?;
        let (mut pointer, mut key) = self.target(item, &roots, &nodes)?;
        let (root, mut ancestors) = self.ancestry(pointer, &roots, &nodes, item)?;
        if let Reference::Option(option) = reference {
            let parent = &nodes[&pointer];
            if let Some(parent_key) = key {
                pointer = unique_row(parent, parent_key, item.id())?.child;
                ancestors.push(item.clone());
            }
            let node = nodes
                .get(&pointer)
                .ok_or_else(|| format!("Menu {} has no live option page", item.id()))?;
            let declared: Vec<_> = self
                .facts
                .menu_manifest
                .iter()
                .flat_map(|m| &m.items)
                .filter(|i| i.name == item.id() || i.identity == item.id())
                .flat_map(|i| &i.options)
                .filter(|o| o.name == option.name())
                .collect();
            if declared.len() > 1 {
                return Err(format!(
                    "Menu {} option {} matches {} declarations",
                    item.id(),
                    option.name(),
                    declared.len()
                ));
            }
            key = if let Some(choice) = declared.first() {
                Some(choice.key)
            } else {
                let mut matches = vec![];
                let mut found = vec![];
                for row in &node.rows {
                    let label = self.caption(node, row).await?;
                    if label == option.name() {
                        matches.push(row.key);
                    }
                    found.push(label);
                }
                if matches.len() != 1 {
                    return Err(format!(
                        "Menu {} option {:?}: {} matches; found {:?}",
                        item.id(),
                        option.name(),
                        matches.len(),
                        found
                    ));
                }
                matches.first().copied()
            };
        }
        let node = &nodes[&pointer];
        let row = key.map(|k| unique_row(node, k, item.id())).transpose()?;
        let active = state.menu_active && state.node == pointer;
        let status = if let Some(row) = row.filter(|_| active) {
            let c = &self.facts.firmware.menus;
            self.uint(
                checked_indexed_address(
                    c.cursor_address,
                    u64::from(row.index),
                    u64::from(c.cursor.status_stride),
                    u64::from(c.cursor.status_offset),
                )?,
                c.cursor.status_width as usize,
            )
            .await? as u32
        } else {
            0
        };
        let value_index = if active && matches!(reference, Reference::Option(_)) {
            Some(state.cursor)
        } else {
            None
        };
        let value = if let Some(value_row) = value_index.and_then(|n| node.rows.get(n as usize)) {
            Some(self.caption(node, value_row).await?)
        } else {
            None
        };
        let label = if let Some(row) = row {
            self.caption(node, row).await?
        } else if let Some(resource) = self.facts.firmware.menus.root_titles.get(&node.code) {
            self.text(*resource).await?
        } else {
            String::new()
        };
        Ok(RemoteReadResult {
            state: state.clone(),
            value: Value::Menu(MenuSnapshot {
                node: pointer,
                key,
                index: row.map(|r| r.index),
                child: row.and_then(|r| nodes.contains_key(&r.child).then_some(r.child)),
                label,
                exists: true,
                visible: active && status != 3,
                selected: active && row.is_none_or(|r| state.cursor == r.index),
                enabled: status < 2,
                value,
                ancestors,
                root,
            }),
            reads: self.reads.clone(),
        })
    }
}
