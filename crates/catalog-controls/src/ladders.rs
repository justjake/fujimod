//! Momentary controls read as bands of one sampled value.
//!
//! A ladder is one wire the firmware samples, an ADC channel or a
//! power-controller register, and each rung is the band the firmware accepts
//! as exactly one control held alone. Every rung names its evidence; a control
//! without an established band has no rung, so nothing can approximate it.

use crate::{ControlBehavior, ControlRef, PhysicalControls};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Controls read as disjoint bands of one sampled value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct ControlLadder {
    /// Where the firmware reads the ladder.
    pub wire: LadderWire,
    /// Number of distinct values the firmware reads on this wire.
    pub levels: u16,
    /// Value read while no control of this ladder is held, when established.
    #[serde(default)]
    pub released: Option<u16>,
    /// Evidence for the wire, its resolution and any released value.
    pub basis: String,
    /// One rung per control; bands are disjoint.
    pub rungs: Vec<LadderRung>,
}

/// The sampled value a ladder is read from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum LadderWire {
    /// One channel of the SoC's ADC.
    Adc {
        /// ADC channel number.
        channel: u8,
    },
    /// One register of the body's serial power controller.
    PowerControllerRegister {
        /// Register number.
        register: u8,
    },
}

/// The band the firmware accepts as one control held alone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct LadderRung {
    /// Exact control identity, including a pad direction, held on this rung.
    pub control_id: String,
    /// Lowest value the firmware accepts as this control alone.
    pub band_low: u16,
    /// Highest value the firmware accepts as this control alone.
    pub band_high: u16,
    /// Value presented while the control is held; inside the band.
    pub sample: u16,
    /// Evidence for the band, the control identity and the sample.
    pub basis: String,
}

impl LadderRung {
    /// Whether the firmware accepts `value` as this control alone.
    pub fn contains(&self, value: u16) -> bool {
        (self.band_low..=self.band_high).contains(&value)
    }
}

/// Whether a rung can hold this identity: a momentary press or a pad direction.
fn momentary(selected: ControlRef<'_>) -> bool {
    let control = selected.control;
    if control.mechanical {
        return false;
    }
    match (&control.behavior, selected.direction) {
        (Some(ControlBehavior::Pad { .. }), Some(_)) => true,
        (Some(ControlBehavior::Button { .. }), None) => true,
        (
            Some(
                ControlBehavior::Pad { .. }
                | ControlBehavior::Dial { .. }
                | ControlBehavior::Ring { .. }
                | ControlBehavior::Rocker { .. },
            ),
            None,
        ) => control.pushable,
        _ => false,
    }
}

impl PhysicalControls {
    /// The ladder and rung holding a control identity, when one is declared.
    pub fn ladder_rung(&self, control_id: &str) -> Option<(&ControlLadder, &LadderRung)> {
        self.ladders.iter().find_map(|ladder| {
            ladder
                .rungs
                .iter()
                .find(|rung| rung.control_id == control_id)
                .map(|rung| (ladder, rung))
        })
    }

    pub(crate) fn validate_ladders(&self) -> Result<(), String> {
        let mut wires = BTreeSet::new();
        let mut held = BTreeSet::new();
        for ladder in &self.ladders {
            let wire = ladder.wire;
            if !wires.insert(wire) {
                return Err(format!("{wire:?} is declared as two ladders"));
            }
            if ladder.levels < 2 || ladder.basis.trim().is_empty() || ladder.rungs.is_empty() {
                return Err(format!(
                    "{wire:?} ladder needs at least two levels, a basis and a rung"
                ));
            }
            if ladder
                .released
                .is_some_and(|released| released >= ladder.levels)
            {
                return Err(format!(
                    "{wire:?} released value exceeds its {} levels",
                    ladder.levels
                ));
            }
            for (position, rung) in ladder.rungs.iter().enumerate() {
                let id = &rung.control_id;
                if rung.basis.trim().is_empty() {
                    return Err(format!("{wire:?} rung {id} has no basis"));
                }
                if rung.band_low > rung.band_high
                    || rung.band_high >= ladder.levels
                    || !rung.contains(rung.sample)
                {
                    return Err(format!(
                        "{wire:?} rung {id} needs low <= sample <= high < {}",
                        ladder.levels
                    ));
                }
                if ladder
                    .released
                    .is_some_and(|released| rung.contains(released))
                {
                    return Err(format!("{wire:?} released value lies in the band of {id}"));
                }
                if ladder.rungs[..position].iter().any(|other| {
                    rung.band_low <= other.band_high && other.band_low <= rung.band_high
                }) {
                    return Err(format!("{wire:?} band of {id} overlaps another rung"));
                }
                if !momentary(self.control(id)?) {
                    return Err(format!(
                        "{wire:?} rung {id} is not a momentary press or pad direction"
                    ));
                }
                if !held.insert(id) {
                    return Err(format!("{id} is declared on two ladder rungs"));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::{LadderWire, PHYSICAL_CONTROLS_SCHEMA_URI, PhysicalControls};
    use serde_json::{Value, json};

    fn document(ladders: Value) -> Value {
        let control = |behavior: Value, pushable: bool| {
            json!({"name":"Documented control","location":"rear","face":"KEY",
                "connection_ids":[],"assignable":false,"pushable":pushable,"positions":[
                    {"id":"S","label":"S","description":null}],
                "mechanical":false,"behavior":behavior})
        };
        json!({
            "$schema":PHYSICAL_CONTROLS_SCHEMA_URI,"schema":1,"id":"body-controls","body_id":"body",
            "manual":{"url":"https://example.com/manual","title":"Camera manual","pages":[3]},
            "analog_levels":null,
            "controls":{
                "ok":control(json!({"kind":"button","press":null}), true),
                "back":control(json!({"kind":"button","press":null}), true),
                "stick":control(json!({"kind":"pad","directions":{
                    "up":null,"down":null,"left":null,"right":null},"press":null}), true),
                "dial":control(json!({"kind":"dial","motion":{"kind":"steps",
                    "decrease":null,"increase":null},"press":null,"endless":true}), false),
                "mode":control(json!({"kind":"switch","positions":[
                    {"id":"S","key":null,"asserted":null}]}), false)
            },
            "ladders":ladders
        })
    }

    fn rung(control_id: &str, low: u16, high: u16, sample: u16) -> Value {
        json!({"control_id":control_id,"band_low":low,"band_high":high,"sample":sample,
            "basis":"callback 0x1000 accepts configuration 0x10..0x12"})
    }

    fn register(register: u8, released: Value, rungs: Value) -> Value {
        json!({"wire":{"kind":"power_controller_register","register":register},
            "levels":1024,"released":released,"basis":"reader masks 10 bits","rungs":rungs})
    }

    fn check(ladders: Value) -> Result<PhysicalControls, String> {
        let controls = PhysicalControls::decode_document(&document(ladders), "test")?;
        controls.validate()?;
        Ok(controls)
    }

    #[test]
    fn declared_rungs_round_trip_and_resolve_their_ladder() {
        let ladders = json!([
            register(0x34, json!(null), json!([rung("ok", 527, 582, 554), rung("stick", 0, 200, 100)])),
            {"wire":{"kind":"adc","channel":14},"levels":4096,"released":4000,
                "basis":"reader masks 12 bits","rungs":[rung("stick.up", 1443, 1766, 1604)]}
        ]);
        let controls = check(ladders.clone()).unwrap();
        assert_eq!(serde_json::to_value(&controls).unwrap()["ladders"], ladders);
        let (ladder, rung) = controls.ladder_rung("stick.up").unwrap();
        assert_eq!(ladder.released, Some(4000));
        assert_eq!((rung.band_low, rung.sample), (1443, 1604));
        assert!(controls.ladder_rung("back").is_none());
        let absent = PhysicalControls::decode_document(
            &{
                let mut value = document(json!([]));
                value.as_object_mut().unwrap().remove("ladders");
                value
            },
            "test",
        )
        .unwrap();
        assert!(absent.ladders.is_empty());
        assert!(
            serde_json::to_value(&absent)
                .unwrap()
                .get("ladders")
                .is_none()
        );
    }

    #[test]
    fn rungs_require_basis_band_and_sample_on_the_wire() {
        let one = |rung: Value| json!([register(0x35, json!(null), json!([rung]))]);
        let mut no_basis = rung("ok", 0, 10, 5);
        no_basis["basis"] = json!("  ");
        for (ladders, message) in [
            (one(no_basis), "has no basis"),
            (one(rung("ok", 10, 20, 21)), "low <= sample <= high"),
            (one(rung("ok", 20, 10, 15)), "low <= sample <= high"),
            (one(rung("ok", 1000, 1024, 1010)), "low <= sample <= high"),
            (
                json!([register(
                    0x35,
                    json!(null),
                    json!([rung("ok", 0, 100, 5), rung("back", 100, 200, 150)])
                )]),
                "overlaps",
            ),
            (
                json!([register(
                    0x35,
                    json!(150),
                    json!([rung("ok", 100, 200, 120)])
                )]),
                "released value lies",
            ),
            (
                json!([register(
                    0x35,
                    json!(1024),
                    json!([rung("ok", 100, 200, 120)])
                )]),
                "exceeds",
            ),
        ] {
            let error = check(ladders).unwrap_err();
            assert!(error.contains(message), "{error}");
        }
    }

    #[test]
    fn ladders_and_controls_are_declared_once_with_evidence() {
        let mut no_basis = register(0x34, json!(null), json!([rung("ok", 0, 10, 5)]));
        no_basis["basis"] = json!("");
        let mut flat = register(0x34, json!(null), json!([rung("ok", 0, 0, 0)]));
        flat["levels"] = json!(1);
        for (ladders, message) in [
            (json!([no_basis]), "needs at least two levels"),
            (json!([flat]), "needs at least two levels"),
            (
                json!([register(0x34, json!(null), json!([]))]),
                "needs at least two levels",
            ),
            (
                json!([
                    register(0x34, json!(null), json!([rung("ok", 0, 10, 5)])),
                    register(0x34, json!(null), json!([rung("back", 0, 10, 5)]))
                ]),
                "declared as two ladders",
            ),
            (
                json!([
                    register(0x34, json!(null), json!([rung("ok", 0, 10, 5)])),
                    register(0x35, json!(null), json!([rung("ok", 0, 10, 5)]))
                ]),
                "two ladder rungs",
            ),
        ] {
            let error = check(ladders).unwrap_err();
            assert!(error.contains(message), "{error}");
        }
    }

    #[test]
    fn xt5_ladders_declare_only_established_rungs() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../catalog/bodies/xt5/controls.json"
        );
        let document: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let controls = PhysicalControls::decode_document(&document, path).unwrap();
        controls.validate().unwrap();
        let wires: Vec<_> = controls.ladders.iter().map(|ladder| ladder.wire).collect();
        assert_eq!(
            wires,
            [
                LadderWire::Adc { channel: 1 },
                LadderWire::Adc { channel: 2 },
                LadderWire::Adc { channel: 14 },
                LadderWire::PowerControllerRegister { register: 0x34 },
                LadderWire::PowerControllerRegister { register: 0x35 },
            ]
        );
        assert!(
            controls
                .ladders
                .iter()
                .all(|ladder| ladder.released.is_none())
        );
        // Playback shares ADC11 with the focus-mode switch and has no band of its own.
        for unestablished in ["playback_button", "focus_mode_selector", "shutter_button"] {
            assert!(
                controls.ladder_rung(unestablished).is_none(),
                "{unestablished}"
            );
        }
        let (ladder, rung) = controls.ladder_rung("menu_ok_button").unwrap();
        assert_eq!(
            ladder.wire,
            LadderWire::PowerControllerRegister { register: 0x34 }
        );
        assert_eq!((rung.band_low, rung.band_high), (527, 582));
        let (_, push) = controls.ladder_rung("focus_stick").unwrap();
        assert_eq!((push.band_low, push.band_high), (0, 200));
        for ladder in &controls.ladders {
            assert!(ladder.basis.contains("key-band table"));
            assert!(
                ladder
                    .rungs
                    .iter()
                    .all(|rung| rung.basis.contains("key-band table"))
            );
        }
    }

    #[test]
    fn rungs_hold_only_documented_momentary_identities() {
        for id in ["absent", "stick.center", "mode", "dial"] {
            let ladders = json!([register(0x34, json!(null), json!([rung(id, 0, 10, 5)]))]);
            assert!(check(ladders).is_err(), "{id}");
        }
        for id in ["ok", "stick", "stick.left"] {
            let ladders = json!([register(0x34, json!(null), json!([rung(id, 0, 10, 5)]))]);
            check(ladders).unwrap();
        }
        let unknown_wire = json!([{"wire":{"kind":"gpio","port":1},"levels":2,"released":null,
            "basis":"x","rungs":[rung("ok", 0, 0, 0)]}]);
        assert!(check(unknown_wire).is_err());
    }
}
