//! Named keys on a session: control poses through the contact panel, or raw
//! body connections.
use std::collections::VecDeque;

use fujimod_catalog::reference::{ConnectionSignal, ResolvedSelection};
use fujimod_catalog_contacts::{Located, LocatedControl};
use fujimod_emulator_session::Session;

use crate::{Nodes, connect};

/// What a key name operates.
#[derive(Clone, Debug)]
enum Target {
    /// A control action or position, through the contact panel.
    Control { control: String, choice: Choice },
    /// A body connection's GPIO contact, driven directly.
    Gpio { port: u8, pin: u8, active: bool },
}

#[derive(Clone, Debug)]
enum Choice {
    Action(String),
    Position(String),
    /// A shutter stage: the half press, or the full press including it.
    Shutter(&'static str),
}

fn key_matches(keys: &[String], name: &str) -> bool {
    keys.iter().any(|key| key.eq_ignore_ascii_case(name))
}

fn control_target(row: &LocatedControl, name: &str) -> Option<Target> {
    if row.kind == "shutter" {
        for (stage, position) in [("half_press", "half"), ("full_press", "full")] {
            if row
                .action(stage)
                .is_some_and(|keys| key_matches(keys, name))
            {
                return Some(Target::Control {
                    control: row.id.clone(),
                    choice: Choice::Shutter(position),
                });
            }
        }
        return None;
    }
    if let Some(position) = row
        .positions
        .iter()
        .find(|position| key_matches(&position.keys, name))
    {
        return Some(Target::Control {
            control: row.id.clone(),
            choice: Choice::Position(position.position.clone()),
        });
    }
    row.actions
        .iter()
        .find(|(_, keys)| key_matches(keys, name))
        .map(|(action, _)| Target::Control {
            control: row.id.clone(),
            choice: Choice::Action(action.clone()),
        })
}

fn target(located: &Located, selection: &ResolvedSelection, name: &str) -> Result<Target, String> {
    if let Some(target) = located
        .controls
        .iter()
        .find_map(|row| control_target(row, name))
    {
        return Ok(target);
    }
    let connection = selection
        .body
        .connections
        .iter()
        .find(|connection| connection.id == name)
        .ok_or_else(|| format!("{name} names no control key or body connection"))?;
    match connection.signals.as_slice() {
        [
            ConnectionSignal::Gpio {
                port,
                pin,
                active_level,
            },
        ] => Ok(Target::Gpio {
            port: *port,
            pin: *pin,
            active: *active_level,
        }),
        _ => Err(format!("body connection {name} is not one GPIO contact")),
    }
}

/// The panel control and position a control choice selects, and the
/// position its release returns to.
fn pose(
    session: &Session,
    control: &str,
    choice: &Choice,
) -> Result<(String, String, Option<String>), String> {
    let panel = session
        .controls
        .as_ref()
        .ok_or("physical controls are not connected")?;
    let exists = |id: &str| panel.spec().controls.iter().any(|known| known.id == id);
    match choice {
        Choice::Shutter(stage) => Ok((control.into(), (*stage).into(), Some("released".into()))),
        Choice::Position(position) => Ok((control.into(), position.clone(), None)),
        Choice::Action(action) => {
            let split = format!("{control}.{action}");
            let id = if exists(&split) {
                split
            } else if exists(control) {
                control.into()
            } else {
                return Err(format!("control {control} offers no {action} contact"));
            };
            Ok((id, "pressed".into(), Some("released".into())))
        }
    }
}

/// Check that a key name operates a control or one GPIO body connection.
pub fn check_key(
    located: &Located,
    selection: &ResolvedSelection,
    name: &str,
) -> Result<(), String> {
    target(located, selection, name).map(|_| ())
}

/// Press or release a named key now. Control keys connect the session's
/// contact panel on first use, which needs the firmware's loaded configuration.
pub fn set_key(
    session: &mut Session,
    selection: &ResolvedSelection,
    located: &Located,
    name: &str,
    pressed: bool,
    ticks_per_second: u64,
) -> Result<(), String> {
    apply(
        session,
        selection,
        &target(located, selection, name)?,
        pressed,
        ticks_per_second,
    )
}

fn apply(
    session: &mut Session,
    selection: &ResolvedSelection,
    target: &Target,
    pressed: bool,
    ticks_per_second: u64,
) -> Result<(), String> {
    match target {
        Target::Gpio { port, pin, active } => {
            let node = Nodes::new(&session.definition)?.gpio;
            session.gpio(&node, *port, *pin, *active == pressed)
        }
        Target::Control { control, choice } => {
            connect(session, selection, ticks_per_second)?;
            let (id, position, released) = pose(session, control, choice)?;
            match (pressed, released) {
                (true, _) => session.set_position(&id, &position),
                (false, Some(released)) => session.set_position(&id, &released),
                (false, None) => Ok(()),
            }
        }
    }
}

/// Key edges at guest times relative to a start tick.
pub struct Keys {
    selection: ResolvedSelection,
    ticks_per_second: u64,
    pending: VecDeque<(u64, Target, bool)>,
}

impl Keys {
    /// Resolve every edge's key name now; `edges` are (delay ticks, name, pressed).
    pub fn new(
        selection: &ResolvedSelection,
        edges: impl IntoIterator<Item = (u64, String, bool)>,
        ticks_per_second: u64,
    ) -> Result<Self, String> {
        let mut edges: Vec<(u64, String, bool)> = edges.into_iter().collect();
        let pending = if edges.is_empty() {
            VecDeque::new()
        } else {
            let located = fujimod_catalog_contacts::locate(selection)?;
            edges.sort_by_key(|(delay, _, pressed)| (*delay, *pressed));
            edges
                .into_iter()
                .map(|(delay, name, pressed)| {
                    Ok((delay, target(&located, selection, &name)?, pressed))
                })
                .collect::<Result<_, String>>()?
        };
        Ok(Self {
            selection: selection.clone(),
            ticks_per_second,
            pending,
        })
    }

    /// Apply every edge due by now; returns the tick of the next edge.
    pub fn advance(&mut self, session: &mut Session, start: u64) -> Result<Option<u64>, String> {
        while let Some((delay, target, pressed)) = self.pending.front() {
            let at = start.saturating_add(*delay);
            if session.board.now() < at {
                return Ok(Some(at));
            }
            apply(
                session,
                &self.selection,
                target,
                *pressed,
                self.ticks_per_second,
            )?;
            self.pending.pop_front();
        }
        Ok(None)
    }
}

/// Drive one GPIO input of the session's GPIO block directly.
pub fn set_gpio(session: &mut Session, port: u8, pin: u8, high: bool) -> Result<(), String> {
    let node = Nodes::new(&session.definition)?.gpio;
    session.gpio(&node, port, pin, high)
}
