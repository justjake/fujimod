//! Resolve each reference anew against the live descriptor graph and cursor.
use crate::{
    ControlSnapshot, Menu, MenuSnapshot, Reference, RemoteReadResult, Result, Value,
    backend::Backend,
    memory::{Memory, word},
};
use fujimod_catalog_controls::InputTarget;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

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
    async fn node(&mut self, pointer: u64) -> Result<Node> {
        let l = &self.facts.firmware.menus.layout;
        if !(32..=256).contains(&l.size) || !(8..=256).contains(&l.row_size) {
            return Err("Unsupported menu descriptor extent".into());
        }
        let raw = self.bytes(pointer, l.size as usize).await?;
        let code = word(&raw, l.fields.menu_id)?;
        if code >= self.facts.firmware.menus.root_count
            || word(&raw, l.fields.type_code)? > l.max_kind
            || word(&raw, l.fields.rows_per_page)? > 1024
        {
            return Err(format!("Invalid menu descriptor at {pointer:#x}"));
        }
        let array = u64::from(word(&raw, l.fields.item_array)?);
        let mut rows = vec![];
        if array != 0 {
            for index in 0..=1024 {
                let raw = self
                    .bytes(array + u64::from(index * l.row_size), l.row_size as usize)
                    .await?;
                let key = word(&raw, l.row_key)?;
                let child = u64::from(word(&raw, l.row_child)?);
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
            parent: u64::from(word(&raw, l.fields.parent_pointer)?),
            captions: u64::from(word(&raw, l.fields.handlers_pointer)?),
            rows,
        })
    }
    async fn graph(&mut self) -> Result<(Vec<u64>, BTreeMap<u64, Node>)> {
        let m = &self.facts.firmware.menus;
        if m.root_count > 4096 {
            return Err("Menu root bound exceeded".into());
        }
        let raw = self
            .bytes(m.roots_address, m.root_count as usize * 4)
            .await?;
        let roots = (0..m.root_count)
            .map(|i| word(&raw, i * 4).map(u64::from))
            .collect::<Result<Vec<_>>>()?;
        let mut pending: VecDeque<_> = roots.iter().copied().filter(|p| *p != 0).collect();
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
            let node = match self.node(pointer).await {
                Ok(node) => node,
                Err(error) if roots.contains(&pointer) => return Err(error),
                Err(_) => continue,
            };
            pending.extend(node.rows.iter().filter(|r| r.child != 0).map(|r| r.child));
            nodes.insert(pointer, node);
        }
        Ok((roots, nodes))
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
        roots: &[u64],
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
            let pointer = *roots
                .get(code as usize)
                .ok_or_else(|| format!("Menu {} is outside the root table", current.id()))?;
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
        roots: &[u64],
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
            if roots.get(node.code as usize) == Some(&current)
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
                node.captions + u64::from(row.index * width) * 4,
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
                c.cursor_address
                    + u64::from(c.cursor.status_offset)
                    + u64::from(row.index * c.cursor.status_stride),
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
