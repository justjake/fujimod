//! Explicit physical switch/contact networks. Circuit tables map external
//! positions to electrical levels; no scanner records, firmware state or UI
//! callbacks participate. Shared ladders and compound switches are one circuit.
//! Unlisted contact combinations refuse before any electrical level changes.
use crate::parts::pins::Pin;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Signal {
    Gpio {
        node: String,
        pin: Pin,
        high: bool,
    },
    Analog {
        node: String,
        channel: usize,
        value: u16,
    },
    SerialStatus {
        node: String,
        route: usize,
        register: u8,
        mask: u8,
        value: u8,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Control {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub positions: Vec<String>,
    pub initial: String,
    pub unavailable: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    pub positions: Vec<String>,
    pub signals: Vec<Signal>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Circuit {
    pub controls: Vec<String>,
    pub cases: Vec<Case>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    pub controls: Vec<Control>,
    pub circuits: Vec<Circuit>,
}
#[derive(Clone, Serialize)]
pub struct SwitchPanel {
    spec: Spec,
    positions: BTreeMap<String, String>,
}
impl<'de> Deserialize<'de> for SwitchPanel {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct State {
            spec: Spec,
            positions: BTreeMap<String, String>,
        }
        let state = State::deserialize(deserializer)?;
        let mut panel = Self::new(state.spec).map_err(serde::de::Error::custom)?;
        if panel.positions.keys().ne(state.positions.keys())
            || panel
                .spec
                .controls
                .iter()
                .any(|c| !c.positions.contains(&state.positions[&c.id]))
        {
            return Err(serde::de::Error::custom(
                "switch snapshot positions differ from its circuit",
            ));
        }
        panel.positions = state.positions;
        panel.signals().map_err(serde::de::Error::custom)?;
        Ok(panel)
    }
}
impl SwitchPanel {
    pub fn new(spec: Spec) -> Result<Self, String> {
        let mut controls = BTreeMap::new();
        for control in &spec.controls {
            if control.id.is_empty()
                || control.id.len() > 128
                || control.label.is_empty()
                || controls.insert(&control.id, control).is_some()
                || control.positions.is_empty()
                || control.positions.len() > 64
                || !control.positions.contains(&control.initial)
                || control.positions.iter().collect::<BTreeSet<_>>().len()
                    != control.positions.len()
            {
                return Err("invalid physical control identity/positions".into());
            }
        }
        let mut wired = BTreeSet::new();
        let mut targets = BTreeSet::new();
        for circuit in &spec.circuits {
            let mut count = 1usize;
            let mut names = BTreeSet::new();
            for id in &circuit.controls {
                let control = controls
                    .get(id)
                    .ok_or("circuit references an unknown control")?;
                if control.unavailable.is_some() || !names.insert(id) {
                    return Err("circuit uses an unavailable or duplicate control".into());
                }
                count = count
                    .checked_mul(control.positions.len())
                    .ok_or("circuit state count overflow")?;
                wired.insert(id);
            }
            if names.is_empty()
                || count > 4096
                || circuit.cases.is_empty()
                || circuit.cases.len() > count
            {
                return Err("circuit case count exceeds its declared contact combinations".into());
            }
            let mut combinations = BTreeSet::new();
            let mut destinations = None;
            for case in &circuit.cases {
                if case.positions.len() != circuit.controls.len()
                    || !combinations.insert(&case.positions)
                    || case
                        .positions
                        .iter()
                        .zip(&circuit.controls)
                        .any(|(p, id)| !controls[id].positions.contains(p))
                    || case.signals.is_empty()
                {
                    return Err("invalid or repeated physical circuit case".into());
                }
                let mut pins = BTreeSet::new();
                for signal in &case.signals {
                    // A status register's bits are separate contacts; signals
                    // write only their mask, so disjoint bits are independent.
                    let keys = match signal {
                        Signal::Gpio { node, pin, .. } => {
                            pin.validate()?;
                            if pin.port >= 16 {
                                return Err("input pad has no status word".into());
                            }
                            vec![format!("gpio:{node}:{}:{}", pin.port, pin.pin)]
                        }
                        Signal::Analog { node, channel, .. } => {
                            if *channel >= 32 {
                                return Err("analog input channel exceeds its bound".into());
                            }
                            vec![format!("analog:{node}:{channel}")]
                        }
                        Signal::SerialStatus {
                            node,
                            route,
                            register,
                            mask,
                            value,
                        } => {
                            if *mask == 0 || value & !mask != 0 {
                                return Err("invalid serial status contact mask".into());
                            }
                            (0..8)
                                .filter(|bit| mask >> bit & 1 != 0)
                                .map(|bit| format!("serial_status:{node}:{route}:{register}:{bit}"))
                                .collect()
                        }
                    };
                    for key in keys {
                        if !pins.insert(key) {
                            return Err("circuit drives a target more than once".into());
                        }
                    }
                }
                if destinations.as_ref().is_some_and(|old| old != &pins) {
                    return Err("all circuit cases must drive the same electrical targets".into());
                }
                destinations = Some(pins);
            }
            for (index, id) in circuit.controls.iter().enumerate() {
                if controls[id]
                    .positions
                    .iter()
                    .any(|p| !circuit.cases.iter().any(|case| &case.positions[index] == p))
                {
                    return Err("circuit omits a declared physical position".into());
                }
            }
            for target in destinations.unwrap() {
                if !targets.insert(target) {
                    return Err("independent circuits share an electrical target".into());
                }
            }
        }
        if spec
            .controls
            .iter()
            .any(|c| c.unavailable.is_none() && !wired.contains(&c.id))
        {
            return Err("available physical control has no connected circuit".into());
        }
        let positions = spec
            .controls
            .iter()
            .map(|c| (c.id.clone(), c.initial.clone()))
            .collect();
        let panel = Self { spec, positions };
        panel.signals()?;
        Ok(panel)
    }
    pub fn spec(&self) -> &Spec {
        &self.spec
    }
    pub fn positions(&self) -> &BTreeMap<String, String> {
        &self.positions
    }
    pub fn signals(&self) -> Result<Vec<Signal>, String> {
        let mut result = Vec::new();
        for circuit in &self.spec.circuits {
            let case = circuit
                .cases
                .iter()
                .find(|case| {
                    case.positions
                        .iter()
                        .zip(&circuit.controls)
                        .all(|(position, id)| position == &self.positions[id])
                })
                .ok_or_else(|| {
                    format!("unsupported contact combination: {:?}", circuit.controls)
                })?;
            result.extend(case.signals.iter().cloned());
        }
        Ok(result)
    }
    /// Validate and prepare a complete network transition before touching any
    /// electrical destination. The caller commits only after all destinations
    /// admit the signals, preserving the old panel on a refusal.
    pub fn transition(&self, id: &str, position: &str) -> Result<Self, String> {
        let control = self
            .spec
            .controls
            .iter()
            .find(|c| c.id == id)
            .ok_or("unknown physical control")?;
        if let Some(reason) = &control.unavailable {
            return Err(format!("{id}: {reason}"));
        }
        if !control.positions.iter().any(|p| p == position) {
            return Err(format!("unknown position for {id}"));
        }
        let mut next = self.clone();
        next.positions.insert(id.to_owned(), position.to_owned());
        next.signals()?;
        Ok(next)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn spec() -> Spec {
        Spec {
            controls: vec![
                Control {
                    id: "focus".into(),
                    label: "Focus".into(),
                    kind: "switch".into(),
                    positions: vec!["S".into(), "M".into()],
                    initial: "S".into(),
                    unavailable: None,
                },
                Control {
                    id: "button".into(),
                    label: "Button".into(),
                    kind: "button".into(),
                    positions: vec!["released".into(), "pressed".into()],
                    initial: "released".into(),
                    unavailable: None,
                },
            ],
            circuits: vec![Circuit {
                controls: vec!["focus".into(), "button".into()],
                cases: (0..4)
                    .map(|i| Case {
                        positions: vec![
                            if i & 1 == 0 { "S" } else { "M" }.into(),
                            if i & 2 == 0 { "released" } else { "pressed" }.into(),
                        ],
                        signals: vec![Signal::Analog {
                            node: "adc".into(),
                            channel: 2,
                            value: 100 + i * 100,
                        }],
                    })
                    .collect(),
            }],
        }
    }
    #[test]
    fn compound_ladder_uses_all_positions_and_refused_changes_leave_the_panel_unchanged() {
        let panel = SwitchPanel::new(spec()).unwrap();
        let next = panel
            .transition("focus", "M")
            .unwrap()
            .transition("button", "pressed")
            .unwrap();
        assert_eq!(
            next.signals().unwrap(),
            vec![Signal::Analog {
                node: "adc".into(),
                channel: 2,
                value: 400
            }]
        );
        assert_eq!(panel.positions()["focus"], "S");
        assert!(panel.transition("focus", "C").is_err());
    }
    #[test]
    fn missing_positions_and_independent_shared_ladders_are_rejected() {
        let mut missing = spec();
        missing.circuits[0]
            .cases
            .retain(|case| case.positions[0] != "M");
        assert!(SwitchPanel::new(missing).is_err());
        let mut duplicate = spec();
        duplicate.circuits.push(duplicate.circuits[0].clone());
        assert!(SwitchPanel::new(duplicate).is_err());
    }
    fn status_buttons(masks: [u8; 2]) -> Spec {
        let names = ["play", "power"];
        Spec {
            controls: names
                .iter()
                .map(|id| Control {
                    id: (*id).into(),
                    label: (*id).into(),
                    kind: "button".into(),
                    positions: vec!["released".into(), "pressed".into()],
                    initial: "released".into(),
                    unavailable: None,
                })
                .collect(),
            circuits: names
                .iter()
                .zip(masks)
                .map(|(id, mask)| Circuit {
                    controls: vec![(*id).into()],
                    cases: [("released", mask), ("pressed", 0)]
                        .into_iter()
                        .map(|(position, value)| Case {
                            positions: vec![position.into()],
                            signals: vec![Signal::SerialStatus {
                                node: "spi".into(),
                                route: 1,
                                register: 0x60,
                                mask,
                                value,
                            }],
                        })
                        .collect(),
                })
                .collect(),
        }
    }
    #[test]
    fn disjoint_status_bits_are_independent_contacts_but_shared_bits_refuse() {
        let panel = SwitchPanel::new(status_buttons([2, 1])).unwrap();
        let pressed = panel.transition("play", "pressed").unwrap();
        assert_eq!(
            pressed.signals().unwrap()[0],
            Signal::SerialStatus {
                node: "spi".into(),
                route: 1,
                register: 0x60,
                mask: 2,
                value: 0
            }
        );
        assert!(SwitchPanel::new(status_buttons([3, 1])).is_err());
    }
}
