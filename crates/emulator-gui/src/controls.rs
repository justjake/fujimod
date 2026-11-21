//! XP5 physical keys exposed as the shared UI control catalog.

use fujimod_catalog_controls::PhysicalControls;
use fujimod_catalog_resolution::ResolvedConfiguration;
use fujimod_catalog_wiring::{Located, LocatedControl};
use fujimod_emulator_session::contact_driver::{ControlChoice, ControlInput};
use fujimod_emulator_xp5_board::session::{Inputs, Key};
use serde_json::Value;
use std::collections::BTreeMap;

/// XP5 controls resolve through the catalog's located electrical wiring.
pub(crate) struct Xp5Controls {
    controls: PhysicalControls,
    located: Located,
    held: BTreeMap<String, String>,
}

/// Position selected at power-on when the body declares a switch.
fn initial_position(row: &LocatedControl) -> Option<String> {
    row.positions
        .iter()
        .find(|position| row.id == "power_switch" && position.position.eq_ignore_ascii_case("ON"))
        .or_else(|| row.positions.first())
        .map(|position| position.position.clone())
}

impl Xp5Controls {
    pub(crate) fn new(configuration: &ResolvedConfiguration) -> Result<Self, String> {
        let source = &configuration.configuration.body_id;
        let controls = PhysicalControls::decode_document(
            configuration
                .reference
                .physical_controls
                .as_ref()
                .ok_or("selected body has no physical controls")?,
            source,
        )?;
        let located = fujimod_catalog_wiring::locate(&configuration.reference)?;
        let held = located
            .controls
            .iter()
            .filter_map(|row| initial_position(row).map(|position| (row.id.clone(), position)))
            .collect();
        Ok(Self { controls, located, held })
    }

    pub(crate) fn document(&self) -> Result<Value, String> {
        serde_json::to_value(&self.controls).map_err(|error| error.to_string())
    }

    fn row(&self, id: &str) -> Result<&LocatedControl, String> {
        self.located
            .controls
            .iter()
            .find(|row| row.id == id)
            .ok_or_else(|| format!("unknown XP5 control {id}"))
    }

    fn keys<'a>(&self, keys: &'a [String]) -> impl Iterator<Item = Key> + 'a {
        keys.iter().filter_map(|key| xp5_key(key))
    }
}

fn xp5_key(name: &str) -> Option<Key> {
    Key::parse(name).ok().or_else(|| match name {
        "EXE_M" | "OK" => Some(Key::Menu),
        "K_PLAY" => Some(Key::ViewMode),
        "CMD_LEFT" => Some(Key::RearMinus),
        "CMD_RIGHT" => Some(Key::RearPlus),
        _ => None,
    })
}

impl Xp5Controls {
    pub(crate) fn catalog(&self) -> Vec<ControlInput> {
        self.located
            .controls
            .iter()
            .filter_map(|row| {
                let control = self.controls.controls.get(&row.id)?;
                if row.mechanical {
                    return None;
                }
                let positions = row
                    .positions
                    .iter()
                    .filter(|position| self.keys(&position.keys).next().is_some())
                    .map(|position| ControlChoice {
                        id: position.position.clone(),
                        label: control
                            .positions
                            .iter()
                            .find(|declared| declared.id == position.position)
                            .and_then(|declared| declared.label.clone())
                            .unwrap_or_else(|| position.position.clone()),
                    })
                    .collect::<Vec<_>>();
                let stages = row
                    .actions
                    .iter()
                    .filter(|(name, keys)| {
                        !matches!(name.as_str(), "press" | "release" | "decrease" | "increase")
                            && self.keys(keys).next().is_some()
                    })
                    .map(|(name, _)| ControlChoice { id: name.clone(), label: name.clone() })
                    .collect::<Vec<_>>();
                let pressable = row
                    .action("press")
                    .is_some_and(|keys| self.keys(keys).next().is_some());
                let relative = row
                    .action("decrease")
                    .is_some_and(|keys| self.keys(keys).next().is_some())
                    && row
                        .action("increase")
                        .is_some_and(|keys| self.keys(keys).next().is_some());
                let supported = pressable || relative || !positions.is_empty() || !stages.is_empty();
                Some(ControlInput {
                    id: row.id.clone(),
                    label: control.name.clone(),
                    kind: row.kind.clone(),
                    supported: supported && row.unavailable.is_none(),
                    unavailable: row.unavailable.clone().or_else(|| {
                        (!supported).then(|| "No XP5 key binding established for this control".into())
                    }),
                    positions: (!positions.is_empty()).then_some(positions),
                    initial: self.held.get(&row.id).cloned(),
                    relative: relative.then_some(true),
                    width: None,
                    height: None,
                    stages: (!stages.is_empty()).then_some(stages),
                })
            })
            .collect()
    }

    pub(crate) fn positions(&self) -> BTreeMap<String, String> {
        self.held.clone()
    }

    pub(crate) fn powered(&self) -> bool {
        self.held
            .get("power_switch")
            .is_none_or(|position| !position.eq_ignore_ascii_case("OFF"))
    }

    pub(crate) fn set_position(
        &mut self,
        session: &mut fujimod_emulator_session::Session,
        inputs: &Inputs,
        id: &str,
        position: &str,
    ) -> Result<(), String> {
        let row = self.row(id)?;
        let target = row
            .positions
            .iter()
            .find(|target| target.position == position)
            .ok_or_else(|| format!("{id} has no position {position}"))?;
        if let Some(old) = self
            .held
            .get(id)
            .and_then(|old| row.positions.iter().find(|candidate| candidate.position == *old))
        {
            let at = session.board.now();
            for key in self.keys(&old.keys) {
                inputs.key(session, at, key, false)?;
            }
        }
        if target.active.unwrap_or(true) {
            let at = session.board.now();
            for key in self.keys(&target.keys) {
                inputs.key(session, at, key, true)?;
            }
        }
        self.held.insert(id.into(), position.into());
        Ok(())
    }

    pub(crate) fn press(
        &mut self,
        session: &mut fujimod_emulator_session::Session,
        inputs: &Inputs,
        id: &str,
        stage: Option<&str>,
    ) -> Result<(), String> {
        let row = self.row(id)?;
        let keys = row
            .action(stage.unwrap_or("press"))
            .ok_or_else(|| format!("{id} has no press action"))?;
        self.enqueue(session, inputs, keys, true)
    }

    pub(crate) fn release(
        &mut self,
        session: &mut fujimod_emulator_session::Session,
        inputs: &Inputs,
        id: &str,
    ) -> Result<(), String> {
        let row = self.row(id)?;
        let mut keys = row
            .action("press")
            .into_iter()
            .flatten()
            .cloned()
            .collect::<Vec<_>>();
        for (name, action) in &row.actions {
            if !matches!(name.as_str(), "press" | "release" | "decrease" | "increase") {
                keys.extend(action.iter().cloned());
            }
        }
        self.enqueue(session, inputs, &keys, false)
    }

    pub(crate) fn rotate(
        &mut self,
        session: &mut fujimod_emulator_session::Session,
        inputs: &Inputs,
        id: &str,
        steps: i32,
    ) -> Result<(), String> {
        let row = self.row(id)?;
        let keys = row
            .action(if steps < 0 { "decrease" } else { "increase" })
            .ok_or_else(|| format!("{id} has no relative step action"))?;
        for _ in 0..steps.unsigned_abs() {
            self.enqueue(session, inputs, keys, true)?;
        }
        Ok(())
    }

    pub(crate) fn powered_switch(
        &mut self,
        session: &mut fujimod_emulator_session::Session,
        inputs: &Inputs,
        on: bool,
    ) -> Result<(), String> {
        let position = self
            .row("power_switch")?
            .positions
            .iter()
            .find(|position| position.position.eq_ignore_ascii_case(if on { "ON" } else { "OFF" }))
            .map(|position| position.position.clone())
            .ok_or("XP5 power switch declares no ON/OFF positions")?;
        self.set_position(session, inputs, "power_switch", &position)
    }

    fn enqueue(
        &self,
        session: &mut fujimod_emulator_session::Session,
        inputs: &Inputs,
        keys: &[String],
        pressed: bool,
    ) -> Result<(), String> {
        let at = session.board.now();
        for key in self.keys(keys) {
            inputs.key(session, at, key, pressed)?;
        }
        Ok(())
    }
}
