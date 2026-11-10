//! Complete pin setup is constructed here; catalogs cannot supply partial programs.
use super::Binding;
use crate::{
    Generation, Recipe,
    image::{self, Result, require, word},
};
use serde::Deserialize;
use std::collections::BTreeSet;

#[derive(Deserialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
pub(crate) enum Initialization {
    StockGpio { ports: Vec<Port> },
    Xp5,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Port {
    pub port: u32,
    pub proof: usize,
}
#[derive(Debug, PartialEq)]
pub(crate) struct Write {
    pub address: u32,
    pub clear: u32,
    pub value: u32,
}
fn write(address: u32, clear: u32, value: u32) -> Write {
    Write {
        address,
        clear,
        value,
    }
}

impl Binding {
    pub(super) fn writes(&self, recipe: &Recipe, proofs: &[Vec<u8>]) -> Result<Vec<Write>> {
        require(!proofs.is_empty(), "Missing green LED source proofs")?;
        match &self.initialization {
            Initialization::StockGpio { ports } => {
                let (stride, id_at, base) = match recipe.generation {
                    Generation::Exr | Generation::ExrPro | Generation::Exr2 => (28, 8, 0xfff89000),
                    Generation::Xpp => (20, 4, 0xfff0c000),
                    Generation::Xp4 => (20, 4, 0xfff41000),
                    Generation::Xp5 => {
                        return Err(image::Error::Invalid(
                            "Wrong LED initialization family".into(),
                        ));
                    }
                };
                let pair = &proofs[0];
                require(pair.len() == stride * 2, "Invalid LED descriptor pair")?;
                let mut pads = Vec::new();
                for offset in [0, stride] {
                    let row = &pair[offset..offset + stride];
                    let kind = match recipe.generation {
                        Generation::Xpp => 0x100,
                        Generation::Xp4 => 0,
                        _ => 1,
                    };
                    require(
                        word(row, id_at - 4)? == kind
                            && (id_at == 4 || word(row, 0)? == 0)
                            && row[id_at + 8..].iter().all(|v| *v == 0),
                        "Invalid direct LED descriptor",
                    )?;
                    let id = word(row, id_at)?;
                    let polarity = word(row, id_at + 4)?;
                    require(
                        id >> 16 < 16 && id & 0xffff < 16 && polarity <= 1,
                        "Invalid LED pin or polarity",
                    )?;
                    pads.push((id >> 16, id & 0xffff, polarity));
                }
                require(
                    pads[0].0 != pads[1].0 || pads[0].1 != pads[1].1,
                    "LED outputs share a pin",
                )?;
                let needed: BTreeSet<_> = pads.iter().map(|pad| pad.0).collect();
                require(
                    ports.len() == needed.len()
                        && ports.iter().map(|p| p.port).collect::<BTreeSet<_>>() == needed,
                    "Missing or duplicate LED port initialization",
                )?;
                let mut writes = Vec::new();
                for index in [1, 0] {
                    let (port, pin, polarity) = pads[index];
                    let record = ports.iter().find(|p| p.port == port).unwrap();
                    require(record.proof > 0, "GPIO table proof cannot be a descriptor")?;
                    let fields = proofs
                        .get(record.proof)
                        .ok_or_else(|| image::Error::Invalid("Missing GPIO table proof".into()))?;
                    require(
                        fields.len() == 28
                            || (matches!(recipe.generation, Generation::Exr) && fields.len() == 24),
                        "Invalid GPIO table record",
                    )?;
                    let bit = 1 << pin;
                    let mux_mask = 3 << (2 * (pin % 8));
                    let extended = word(fields, 0)?;
                    let mux = word(fields, 4 * (1 + pin as usize / 8))?;
                    let control = word(fields, 12)?;
                    let direction = word(fields, 16)?;
                    let data = word(fields, 20)?;
                    require(
                        mux >> 16 & mux_mask == mux_mask
                            && [control, direction, data]
                                .iter()
                                .all(|v| v >> 16 & bit == bit),
                        "Stock GPIO table leaves an LED field unspecified",
                    )?;
                    require(
                        extended & (3 << (2 * pin)) == 0
                            && mux & mux_mask == 0
                            && direction & bit == bit,
                        "Stock LED pin is not a direct GPIO output",
                    )?;
                    let gpio = base + port * 64;
                    let high = if index == 0 { polarity } else { 1 - polarity };
                    writes.extend([
                        write(gpio + 24, 3 << (2 * pin), 0),
                        write(gpio + 12 + 4 * (pin / 8), 0, mux_mask << 16),
                        write(gpio + 8, 0, (bit << 16) | (control & bit)),
                        write(gpio, 0, (bit << 16) | (high << pin)),
                        write(gpio + 4, 0, (bit << 16) | bit),
                    ]);
                }
                Ok(writes)
            }
            Initialization::Xp5 => {
                require(
                    matches!(recipe.generation, Generation::Xp5),
                    "Wrong LED initialization family",
                )?;
                let lamp = &recipe
                    .xp5
                    .as_ref()
                    .ok_or_else(|| image::Error::Invalid("Missing XP5 lamp setup".into()))?
                    .lamp;
                let mmio = |address: u64| {
                    (0xf0000000..0xfffff000).contains(&address) && address.is_multiple_of(4)
                };
                require(
                    [lamp.pio_base, lamp.clock_address, lamp.reset_address]
                        .into_iter()
                        .all(mmio)
                        && lamp.clock_mask != 0
                        && lamp.reset_mask != 0
                        && lamp.green_port < 16
                        && lamp.red_port < 16
                        && lamp.green_port != 3
                        && lamp.red_port != 3
                        && lamp.green_port != lamp.red_port
                        && lamp.pin < 8,
                    "Invalid XP5 LED setup",
                )?;
                let pio = lamp.pio_base as u32;
                let pin = lamp.pin;
                let mut writes = vec![
                    write(lamp.clock_address as u32, lamp.clock_mask, lamp.clock_mask),
                    write(lamp.reset_address as u32, lamp.reset_mask, lamp.reset_mask),
                ];
                for port in [lamp.green_port, lamp.red_port] {
                    let gpio = pio + port * 64;
                    writes.extend([
                        write(pio + 0x500, 0, (1 << (port + 16)) | (1 << port)),
                        write(gpio, 0, 1 << (pin + 16)),
                        write(gpio + 12, 0, 3 << (pin * 2 + 16)),
                        write(gpio + 8, 0, 1 << (pin + 16)),
                        write(gpio + 4, 0, (1 << (pin + 16)) | (1 << pin)),
                    ]);
                }
                writes.push(write(
                    pio + lamp.green_port * 64,
                    0,
                    (1 << (pin + 16)) | (1 << pin),
                ));
                Ok(writes)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::put;

    fn fixture(id: &str, green: u32, red: u32) -> (Recipe, Binding, Vec<Vec<u8>>) {
        let recipe = Recipe::bundled(id).unwrap().unwrap();
        let binding = super::super::binding(id).unwrap().unwrap();
        let (stride, id_at, kind) = match recipe.generation {
            Generation::Xpp => (20, 4, 0x100),
            Generation::Xp4 => (20, 4, 0),
            _ => (28, 8, 1),
        };
        let mut pair = vec![0; stride * 2];
        for (offset, pin, polarity) in [(0, green, 0), (stride, red, 1)] {
            put(&mut pair, offset + id_at - 4, kind).unwrap();
            put(&mut pair, offset + id_at, 15 << 16 | pin).unwrap();
            put(&mut pair, offset + id_at + 4, polarity).unwrap();
        }
        let mut row = vec![0; 28];
        for (at, value) in [
            (4, 0xffff0000),
            (8, 0xffff0000),
            (12, 0xffffffff),
            (16, 0xffffffff),
            (20, 0xffff0000),
        ] {
            put(&mut row, at, value).unwrap();
        }
        let mut binding = binding;
        binding.initialization = Initialization::StockGpio {
            ports: vec![Port { port: 15, proof: 1 }],
        };
        (recipe, binding, vec![pair, row])
    }

    #[test]
    fn all_gpio_families_require_every_field_for_both_pins_in_both_mux_halves() {
        for id in [
            "x10-201",
            "xf1-101",
            "xe1-271",
            "xt1-552",
            "gfx50r-311",
            "x100v-301",
        ] {
            for (green, red) in [(2, 0), (12, 9), (0, 15)] {
                let (recipe, binding, proofs) = fixture(id, green, red);
                let writes = binding.writes(&recipe, &proofs).unwrap();
                assert_eq!(writes.len(), 10, "{id}");
                for pin in [green, red] {
                    for (offset, mask) in [
                        (4 * (1 + pin as usize / 8), 3 << (2 * (pin % 8))),
                        (12, 1 << pin),
                        (16, 1 << pin),
                        (20, 1 << pin),
                    ] {
                        let mut incomplete = proofs.clone();
                        let value = word(&incomplete[1], offset).unwrap();
                        put(&mut incomplete[1], offset, value & !(mask << 16)).unwrap();
                        assert!(
                            binding.writes(&recipe, &incomplete).is_err(),
                            "{id} pin {pin} field {offset}"
                        );
                    }
                }
                for offset in [0, 4 * (1 + green as usize / 8), 16] {
                    let mut wrong = proofs.clone();
                    let value = word(&wrong[1], offset).unwrap();
                    let mask = if offset == 16 {
                        1 << green
                    } else {
                        3 << (2 * (if offset == 0 { green } else { green % 8 }))
                    };
                    put(
                        &mut wrong[1],
                        offset,
                        if offset == 16 {
                            value & !mask
                        } else {
                            value | mask
                        },
                    )
                    .unwrap();
                    assert!(
                        binding.writes(&recipe, &wrong).is_err(),
                        "{id} field {offset}"
                    );
                }
                for size in [0, 4, 20, 25] {
                    let mut truncated = proofs.clone();
                    truncated[1].truncate(size);
                    assert!(binding.writes(&recipe, &truncated).is_err());
                }
                let mut missing = binding;
                missing.initialization = Initialization::StockGpio { ports: vec![] };
                assert!(missing.writes(&recipe, &proofs).is_err());
            }
        }
    }

    #[test]
    fn catalog_cannot_supply_or_omit_any_program_writes() {
        let rows: Vec<serde_json::Value> = serde_json::from_str(include_str!(
            "../../../../catalog/recovery/green-led.generated.json"
        ))
        .unwrap();
        for row in rows {
            assert!(serde_json::from_value::<Binding>(row.clone()).is_ok());
            let mut missing = row.clone();
            missing.as_object_mut().unwrap().remove("initialization");
            assert!(serde_json::from_value::<Binding>(missing).is_err());
            let mut partial = row;
            partial["writes"] = serde_json::json!([]);
            assert!(serde_json::from_value::<Binding>(partial).is_err());
        }
    }
}

#[cfg(test)]
mod xp5_tests {
    use super::*;

    #[test]
    fn xp5_clock_reset_gates_and_pad_setup_are_mandatory() {
        let original = Recipe::bundled("x100vi-131").unwrap().unwrap();
        let binding = super::super::binding("x100vi-131").unwrap().unwrap();
        let proofs = vec![vec![]];
        let writes = binding.writes(&original, &proofs).unwrap();
        assert_eq!(writes.len(), 13);
        let lamp = &original.xp5.as_ref().unwrap().lamp;
        assert_eq!(
            writes[0],
            write(lamp.clock_address as u32, lamp.clock_mask, lamp.clock_mask)
        );
        assert_eq!(
            writes[1],
            write(lamp.reset_address as u32, lamp.reset_mask, lamp.reset_mask)
        );
        for damage in 0..7 {
            let mut recipe = original.clone();
            let lamp = &mut recipe.xp5.as_mut().unwrap().lamp;
            match damage {
                0 => lamp.clock_mask = 0,
                1 => lamp.reset_mask = 0,
                2 => lamp.pin = 8,
                3 => lamp.green_port = lamp.red_port,
                4 => lamp.green_port = 3,
                5 => lamp.pio_base = 0,
                _ => recipe.xp5 = None,
            }
            assert!(binding.writes(&recipe, &proofs).is_err());
        }
        assert!(binding.writes(&original, &[]).is_err());
    }
}
