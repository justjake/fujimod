//! Actions composed once over the four primitives; callers own assertions.
use crate::*;
pub async fn press<R: Remote + ?Sized>(r: &mut R, c: &Control, deadline: u64) -> Result<()> {
    r.set_input(
        &Input {
            control: c.clone(),
            state: InputState::Pressed,
        },
        deadline,
    )
    .await?;
    r.set_input(
        &Input {
            control: c.clone(),
            state: InputState::Released,
        },
        deadline,
    )
    .await
}
pub async fn turn<R: Remote + ?Sized>(
    r: &mut R,
    c: &Control,
    steps: i32,
    deadline: u64,
) -> Result<()> {
    if steps == 0 {
        return Ok(());
    }
    r.set_input(
        &Input {
            control: c.clone(),
            state: InputState::Turn(steps),
        },
        deadline,
    )
    .await
}
/// One finger down at a point and lifted, each step recognized by the firmware.
pub async fn tap<R: Remote + ?Sized>(
    r: &mut R,
    c: &Control,
    x: u32,
    y: u32,
    deadline: u64,
) -> Result<()> {
    drag(r, c, &[(x, y)], deadline).await
}
/// One finger down at the first point, moved through the rest and lifted.
/// Each point is recognized by the firmware before the next; no step waits a
/// fixed duration.
pub async fn drag<R: Remote + ?Sized>(
    r: &mut R,
    c: &Control,
    points: &[(u32, u32)],
    deadline: u64,
) -> Result<()> {
    let (&(x, y), rest) = points
        .split_first()
        .ok_or("A touch gesture requires at least one point")?;
    let mut input = Input {
        control: c.clone(),
        state: InputState::TouchStart { x, y },
    };
    r.set_input(&input, deadline).await?;
    for &(x, y) in rest {
        input.state = InputState::TouchMove { x, y };
        r.set_input(&input, deadline).await?;
    }
    input.state = InputState::TouchEnd;
    r.set_input(&input, deadline).await
}
struct Tracked<'a, R: ?Sized> {
    remote: &'a mut R,
    reads: ReadSet,
    last: Vec<RemoteReadResult>,
}
impl<R: RemoteReader + ?Sized> RemoteReader for Tracked<'_, R> {
    fn now(&self) -> u64 {
        self.remote.now()
    }
    fn read<'a>(&'a mut self, reference: &'a Reference) -> LocalFuture<'a, RemoteReadResult> {
        Box::pin(async move {
            let v = self.remote.read(reference).await?;
            self.reads.extend(v.reads.clone());
            self.last.push(v.clone());
            Ok(v)
        })
    }
    fn read_memory<'a>(&'a mut self, address: u64, size: usize) -> LocalFuture<'a, MemoryRead> {
        Box::pin(async move {
            let value = self.remote.read_memory(address, size).await?;
            self.reads.extend(value.reads.clone());
            Ok(value)
        })
    }
    fn frame(&mut self) -> LocalFuture<'_, Capture> {
        Box::pin(async move {
            let v = self.remote.frame().await?;
            self.reads.extend(v.reads.clone());
            Ok(v)
        })
    }
    fn readiness<'a>(&'a mut self, control: Option<&'a Control>) -> LocalFuture<'a, Readiness> {
        Box::pin(async move {
            let v = self.remote.readiness(control).await?;
            self.reads.extend(v.reads.clone());
            Ok(v)
        })
    }
}
/// Wait until the firmware accepts a new input.
pub async fn wait_ready<R: Remote + ?Sized>(
    r: &mut R,
    control: Option<&Control>,
    deadline: u64,
) -> Result<()> {
    loop {
        let readiness = r.readiness(control).await?;
        if readiness.ready {
            return Ok(());
        }
        if r.now() >= deadline {
            return Err(format!(
                "Timed out waiting for input readiness: {:?}",
                readiness.pending
            ));
        }
        r.wait(&readiness.reads, deadline).await?;
    }
}
pub async fn wait_until<R: Remote + ?Sized>(
    r: &mut R,
    mut predicate: impl for<'a> FnMut(&'a mut dyn RemoteReader) -> LocalFuture<'a, bool>,
    deadline: u64,
    condition: &str,
) -> Result<()> {
    loop {
        let mut view = Tracked {
            remote: r,
            reads: ReadSet::default(),
            last: vec![],
        };
        if predicate(&mut view).await? {
            return Ok(());
        }
        let reads = view.reads;
        let last = view.last;
        if r.now() >= deadline {
            return Err(format!(
                "Timed out waiting for {condition}; last remote reads: {last:?}; read set: {reads:?}"
            ));
        }
        r.wait(&reads, deadline).await?;
    }
}

/// Wait for explicit byte ranges to satisfy caller-supplied equality checks
/// and remain unchanged for `stable_comparisons` consecutive comparisons.
///
/// All policy stays with the caller: Rust only reads bytes, compares spans,
/// and owns the repeat/wait loop. `stable_spans` can be narrower than `ranges`
/// so adjacent bytes fetched for transport efficiency do not affect stability.
pub async fn wait_until_stable_memory<R: Remote + ?Sized>(
    r: &mut R,
    ranges: &[(u64, usize)],
    stable_spans: &[(u64, usize)],
    equals: &[(u64, Vec<u8>)],
    stable_comparisons: usize,
    deadline: u64,
    condition: &str,
) -> Result<Vec<MemoryRead>> {
    if ranges.is_empty() {
        return Err("At least one memory range is required".into());
    }
    if ranges.iter().any(|(_, size)| *size == 0) {
        return Err("Memory ranges must be nonempty".into());
    }
    if stable_comparisons == 0 {
        return Err("stable_comparisons must be nonzero".into());
    }
    for &(address, size) in stable_spans {
        if size == 0
            || !ranges.iter().any(|&(start, len)| {
                address >= start
                    && address
                        .checked_add(size as u64)
                        .is_some_and(|end| end <= start.saturating_add(len as u64))
            })
        {
            return Err(format!(
                "Stable span {address:#x}+{size:#x} is outside requested ranges"
            ));
        }
    }
    for (address, bytes) in equals {
        if bytes.is_empty()
            || !ranges.iter().any(|&(start, len)| {
                *address >= start
                    && address
                        .checked_add(bytes.len() as u64)
                        .is_some_and(|end| end <= start.saturating_add(len as u64))
            })
        {
            return Err(format!(
                "Equality span {address:#x}+{:#x} is outside requested ranges",
                bytes.len()
            ));
        }
    }

    let stable_bytes = stable_spans.iter().try_fold(0usize, |total, (_, size)| {
        total
            .checked_add(*size)
            .ok_or("Stable span size overflows usize")
    })?;
    let mut previous = vec![0; stable_bytes];
    let mut current = Vec::with_capacity(stable_bytes);
    let mut has_previous = false;
    let mut stable = 0usize;
    loop {
        let mut reads = Vec::with_capacity(ranges.len());
        for &(address, size) in ranges {
            reads.push(r.read_memory(address, size).await?);
        }
        let span_bytes = |address: u64, size: usize| -> Result<&[u8]> {
            reads
                .iter()
                .find_map(|read| {
                    let offset = address.checked_sub(read.address)? as usize;
                    let end = offset.checked_add(size)?;
                    (end <= read.bytes.len()).then_some(&read.bytes[offset..end])
                })
                .ok_or_else(|| format!("Read result did not cover {address:#x}+{size:#x}"))
        };
        current.clear();
        for &(address, size) in stable_spans {
            current.extend_from_slice(span_bytes(address, size)?);
        }
        let mut matches = true;
        for (address, expected) in equals {
            if span_bytes(*address, expected.len())? != expected.as_slice() {
                matches = false;
                break;
            }
        }
        if matches {
            stable = if has_previous && previous == current {
                stable.saturating_add(1)
            } else {
                0
            };
            if stable >= stable_comparisons {
                return Ok(reads);
            }
        } else {
            stable = 0;
        }
        std::mem::swap(&mut previous, &mut current);
        has_previous = true;
        let read_set = reads.iter().fold(ReadSet::default(), |mut set, read| {
            set.extend(read.reads.clone());
            set
        });
        if r.now() >= deadline {
            return Err(format!(
                "Timed out waiting for {condition}; last memory reads: {reads:?}"
            ));
        }
        r.wait(&read_set, deadline).await?;
    }
}
/// The physical control for one navigation step, by the body's canonical IDs.
///
/// MENU/OK opens menus and confirms; bodies without a separate MENU/OK button
/// press the selector's center. Directions are the selector's members.
fn navigation_control(facts: &Facts, step: &str) -> Result<Control> {
    let controls = &facts.physical_controls;
    let id = match step {
        "menu" | "ok" if controls.controls.contains_key("menu_ok_button") => {
            "menu_ok_button".into()
        }
        "menu" | "ok" => "selector".into(),
        "back" => "disp_back_button".into(),
        "up" | "down" | "left" | "right" => format!("selector.{step}"),
        other => return Err(format!("Unknown navigation step {other}")),
    };
    controls.control(&id)?;
    Ok(control(id))
}
async fn step<R: Remote + ?Sized>(
    r: &mut R,
    facts: &Facts,
    reference: &Reference,
    role: &str,
    deadline: u64,
) -> Result<RemoteReadResult> {
    let before = r.read(reference).await?;
    press(r, &navigation_control(facts, role)?, deadline).await?;
    loop {
        let value = r.read(reference).await?;
        let mut reads = value.reads.clone();
        let mut pending = vec![];
        // Readiness is observed only once the state has changed, so the wait
        // for the change watches the state reads alone.
        if value.state != before.state {
            let readiness = r.readiness(None).await?;
            if readiness.ready {
                return Ok(value);
            }
            reads.extend(readiness.reads);
            pending = readiness.pending;
        }
        if r.now() >= deadline {
            return Err(format!(
                "Timed out waiting for {role} navigation; last state: {:?}, pending: {pending:?}",
                value.state
            ));
        }
        r.wait(&reads, deadline).await?;
    }
}
async fn root<R: Remote + ?Sized>(
    r: &mut R,
    facts: &Facts,
    name: &str,
    deadline: u64,
) -> Result<()> {
    let reference: Reference = menu(name).into();
    let initial = r.read(&reference).await?;
    if initial.state.menu_active && initial.state.node == initial.menu()?.node {
        return Ok(());
    }
    let code = facts
        .firmware
        .menus
        .roles
        .get(name)
        .copied()
        .or_else(|| name.parse().ok())
        .ok_or("Unknown root role")?;
    let route = facts
        .firmware
        .menus
        .root_routes
        .get(&code)
        .ok_or_else(|| format!("No located entry route for menu {name}"))?;
    for _ in 0..64 {
        if !r.read(&reference).await?.state.menu_active {
            break;
        }
        step(r, facts, &reference, "back", deadline).await?;
    }
    if r.read(&reference).await?.state.menu_active {
        return Err(format!("Could not close menus while entering {name}"));
    }
    step(r, facts, &reference, "menu", deadline).await?;
    // MENU may restore a remembered submenu. Return through its real BACK
    // path to a routed root before selecting another root's tab.
    for _ in 0..64 {
        let value = r.read(&reference).await?;
        if !value.state.menu_active {
            step(r, facts, &reference, "menu", deadline).await?;
            continue;
        }
        if value.state.node == value.menu()?.node {
            return Ok(());
        }
        let mut routed = false;
        for code in facts.firmware.menus.root_routes.keys() {
            let root = r.read(&menu(code.to_string()).into()).await?;
            if root.state.node == root.menu()?.node {
                routed = true;
                break;
            }
        }
        if routed {
            break;
        }
        step(r, facts, &reference, "back", deadline).await?;
    }
    match route.entry.as_str() {
        "shooting_menu" | "setup_menu" => {
            step(r, facts, &reference, "left", deadline).await?;
            let tab = route
                .tab
                .or_else(|| facts.firmware.menus.page_tabs.get(&code).copied())
                .ok_or("Root entry route lacks a tab key")?;
            for _ in 0..64 {
                let value = r.read(&reference).await?;
                if value.state.current_key == Some(tab) {
                    break;
                }
                let current = value
                    .state
                    .current_key
                    .ok_or("Tab rail has no selected row")?;
                step(
                    r,
                    facts,
                    &reference,
                    if current < tab { "down" } else { "up" },
                    deadline,
                )
                .await?;
            }
            if r.read(&reference).await?.state.current_key != Some(tab) {
                return Err(format!("Could not reach tab {tab}"));
            }
            step(r, facts, &reference, "right", deadline).await?;
        }
        other => return Err(format!("Unsupported root entry strategy {other}")),
    }
    let value = r.read(&reference).await?;
    if !value.state.menu_active || value.state.node != value.menu()?.node {
        return Err(format!("Entry route {name} reached {:?}", value.state));
    }
    Ok(())
}
async fn focus<R: Remote + ?Sized>(
    r: &mut R,
    facts: &Facts,
    reference: &Reference,
    deadline: u64,
) -> Result<RemoteReadResult> {
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..1024 {
        let value = r.read(reference).await?;
        let target = value.menu()?;
        if !value.state.menu_active || value.state.node != target.node {
            return Err(format!(
                "Navigation left menu {reference:?}; {:?}",
                value.state
            ));
        }
        let readiness = r.readiness(None).await?;
        if !readiness.ready {
            if r.now() >= deadline {
                return Err(format!(
                    "Timed out waiting for navigation readiness: {:?}, pending: {:?}",
                    value.state, readiness.pending
                ));
            }
            let mut reads = value.reads.clone();
            reads.extend(readiness.reads);
            r.wait(&reads, deadline).await?;
            continue;
        }
        if target.selected {
            return Ok(value);
        }
        if !target.enabled {
            return Err(format!("Menu {reference:?} is disabled"));
        }
        if !seen.insert(value.state.cursor) {
            return Err(format!(
                "Menu {reference:?} is unavailable; repeated cursor {}",
                value.state.cursor
            ));
        }
        let index = target.index.ok_or("Page has no focusable row")?;
        step(
            r,
            facts,
            reference,
            if value.state.cursor < index {
                "down"
            } else {
                "up"
            },
            deadline,
        )
        .await?;
    }
    Err(format!("Navigation exceeded row bound for {reference:?}"))
}
pub async fn navigate<R: Remote + ?Sized>(
    r: &mut R,
    facts: &Facts,
    item: &Menu,
    deadline: u64,
) -> Result<RemoteReadResult> {
    let reference = Reference::Menu(item.clone());
    let initial = r.read(&reference).await?;
    if initial.state.menu_active && initial.state.node == initial.menu()?.node {
        return focus(r, facts, &reference, deadline).await;
    }
    root(r, facts, &initial.menu()?.root, deadline).await?;
    for depth in 0..64 {
        let value = r.read(&reference).await?;
        if value.state.menu_active && value.state.node == value.menu()?.node {
            return focus(r, facts, &reference, deadline).await;
        }
        let ancestor = value
            .menu()?
            .ancestors
            .get(depth)
            .ok_or_else(|| format!("Menu {item:?} has no opening row at depth {depth}"))?
            .clone();
        focus(r, facts, &Reference::Menu(ancestor.clone()), deadline).await?;
        step(r, facts, &Reference::Menu(ancestor), "right", deadline).await?;
    }
    Err(format!("Navigation exceeded ancestor bound for {item:?}"))
}
pub async fn select<R: Remote + ?Sized>(
    r: &mut R,
    facts: &Facts,
    choice: &OptionReference,
    deadline: u64,
) -> Result<()> {
    let reference = Reference::Option(choice.clone());
    let target = r.read(&reference).await?;
    if target.state.node != target.menu()?.node || !target.state.menu_active {
        navigate(r, facts, choice.item(), deadline).await?;
        let parent = r.read(&Reference::Menu(choice.item().clone())).await?;
        if parent.menu()?.key.is_some() {
            step(r, facts, &reference, "right", deadline).await?;
        }
    }
    focus(r, facts, &reference, deadline).await?;
    press(r, &navigation_control(facts, "ok")?, deadline).await
}
pub async fn screenshot<R: Remote + ?Sized>(r: &mut R, deadline: u64) -> Result<Capture> {
    let before = r.frame().await?;
    loop {
        let frame = r.frame().await?;
        if frame.sequence != before.sequence {
            return Ok(frame);
        }
        if r.now() >= deadline {
            return Err(format!(
                "Timed out waiting for next completed {} frame; last sequence={}, time={}",
                frame.output, frame.sequence, frame.time
            ));
        }
        r.wait(&frame.completion_reads, deadline).await?;
    }
}
