//! External card media and explicit SD host wiring.
use fujimod_emulator_devices::{
    bus::InterruptLine,
    graph::{DeviceGraph, DeviceWindow},
    parts::{card_contacts::Contacts, sd_card::Card},
};
use fujimod_emulator_ip::sdhci::host::{Options, SdHost, Wiring};
use fujimod_emulator_storage::fat;
use serde::Deserialize;
use std::collections::BTreeMap;

pub struct Media {
    pub card: Card,
    pub contacts: Contacts,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaInputs {
    pub base: u64,
    pub format: fat::Options,
    pub files: Vec<(String, String)>,
    pub present: bool,
    pub protected: bool,
}
pub fn resolve(
    inputs: Vec<MediaInputs>,
    assets: &BTreeMap<String, Vec<u8>>,
) -> Result<BTreeMap<u64, Media>, String> {
    let mut media = BTreeMap::new();
    for input in inputs {
        let files = input
            .files
            .into_iter()
            .map(|(path, id)| {
                assets
                    .get(&id)
                    .cloned()
                    .map(|bytes| (path, bytes))
                    .ok_or_else(|| format!("Missing SD file asset {id}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let card = fat::format(&input.format, &files)?;
        let contacts = Contacts::new(input.present, input.protected);
        if media.insert(input.base, Media { card, contacts }).is_some() {
            return Err("Repeated SD media base".into());
        }
    }
    Ok(media)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Host {
    controller: Options,
    irq: Option<u32>,
}
pub(super) fn register(
    graph: &mut DeviceGraph,
    window: &DeviceWindow,
    media: &mut BTreeMap<u64, Media>,
) -> Result<(), String> {
    let host: Host = toml::Value::Table(window.options.clone())
        .try_into()
        .map_err(|e| format!("SD host options: {e}"))?;
    if !matches!(window.size, 0x1000 | 0x1e0) || window.base != host.controller.base {
        return Err("SD host window and controller base differ".into());
    }
    let Media { card, contacts } = media
        .remove(&window.base)
        .ok_or("SD host requires explicit card media")?;
    let device = if let Some(irq) = host.irq {
        SdHost::new(
            host.controller,
            Wiring {
                card,
                contacts,
                irq: InterruptLine::Shared(irq),
            },
        )?
    } else {
        SdHost::new_polled(host.controller, card, contacts)?
    };
    graph.register(window.base, window.size, Box::new(device))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn input() -> MediaInputs {
        MediaInputs {
            base: 0xffd00000,
            format: fat::Options {
                sectors: 0x800000,
                max_dirty_sectors: 65536,
                partition_start: 8192,
                sectors_per_cluster: 64,
                label: "BOOT".into(),
                read_only_files: vec![],
            },
            files: vec![("marble_init.bin".into(), "setup".into())],
            present: true,
            protected: false,
        }
    }
    #[test]
    fn declared_card_assets_keep_long_names_contacts_and_explicit_host_wiring() {
        let assets = BTreeMap::from([("setup".into(), vec![0x12, 0x34])]);
        let mut media = resolve(vec![input()], &assets).unwrap();
        let contacts = media[&0xffd00000].contacts.clone();
        assert!(contacts.present());
        assert_eq!(
            fat::Reader::new(&media[&0xffd00000].card)
                .unwrap()
                .read("marble_init.bin", 2)
                .unwrap(),
            [0x12, 0x34]
        );
        let mut graph = DeviceGraph::new();
        let window=DeviceWindow {base:0xffd00000,size:0x1000,kind:"sdhci".into(),options:toml::from_str("[controller]\nbase=4291821568\nprotocol='sdhc'\ntransfer_ticks=4\nclock_ticks=2\nrca=1\nram_bytes=4294967296").unwrap()};
        register(&mut graph, &window, &mut media).unwrap();
        assert!(media.is_empty());
        assert!(graph.at(0xffd00000).is_ok());
        assert!(
            register(&mut graph, &window, &mut media)
                .unwrap_err()
                .contains("card media")
        );
        contacts.set(false, true);
        assert!(!contacts.present());
        assert!(contacts.protected());
        assert!(
            resolve(vec![input()], &BTreeMap::new())
                .err()
                .unwrap()
                .contains("Missing SD file")
        );
        assert!(
            resolve(vec![input(), input()], &assets)
                .err()
                .unwrap()
                .contains("Repeated")
        );
    }
}
