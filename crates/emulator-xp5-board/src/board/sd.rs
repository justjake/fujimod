//! External card media and explicit SD host wiring.
//!
//! A slot's card belongs to one UHS-I host (`sdhci`). A UHS-II port of the same
//! slot (`sdhci-uhs2`) shares only its contacts.
use fujimod_emulator_devices::{
    bus::InterruptLine,
    graph::{DeviceGraph, DeviceWindow},
    parts::{card_contacts::Contacts, sd_card::Card},
};
use fujimod_emulator_ip::sdhci::{
    host::{Options, SdHost, Wiring},
    uhs2,
};
use fujimod_emulator_storage::fat;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub struct Media {
    /// Taken by the slot's UHS-I host when it is registered.
    pub card: Option<Card>,
    pub contacts: Contacts,
}
/// Fails when a declared card has no UHS-I host.
pub fn require_hosts(media: &BTreeMap<u64, Media>) -> Result<(), String> {
    match media.iter().find(|(_, media)| media.card.is_some()) {
        Some((base, _)) => Err(format!("SD card media at {base:#x} has no SD host")),
        None => Ok(()),
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
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
        if media.insert(input.base, Media { card: Some(card), contacts }).is_some() {
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
    let slot = media
        .get_mut(&window.base)
        .ok_or("SD host requires explicit card media")?;
    let card = slot.card.take().ok_or("SD card media already has an SD host")?;
    let contacts = slot.contacts.clone();
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
/// A UHS-II port of the slot whose UHS-I host is at `slot`: its PHY control
/// block at the window base and its SDHCI register file `0x200` above it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Uhs2Port {
    slot: u64,
    capabilities: u64,
    clock_ticks: u64,
}
pub(super) fn register_uhs2(
    graph: &mut DeviceGraph,
    window: &DeviceWindow,
    media: &BTreeMap<u64, Media>,
) -> Result<(), String> {
    let port: Uhs2Port = toml::Value::Table(window.options.clone())
        .try_into()
        .map_err(|e| format!("UHS-II port options: {e}"))?;
    if window.size != 0x300 {
        return Err("UHS-II port window must hold its PHY block and register file".into());
    }
    let contacts = media
        .get(&port.slot)
        .ok_or("UHS-II port requires the slot's card media")?
        .contacts
        .clone();
    let device = uhs2::Port::new(
        uhs2::Options {
            phy: window.base,
            host: window.base + 0x200,
            capabilities: port.capabilities,
            clock_ticks: port.clock_ticks,
        },
        contacts,
    )?;
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
            fat::Reader::new(media[&0xffd00000].card.as_ref().unwrap())
                .unwrap()
                .read("marble_init.bin", 2)
                .unwrap(),
            [0x12, 0x34]
        );
        let mut graph = DeviceGraph::new();
        let window=DeviceWindow {base:0xffd00000,size:0x1000,kind:"sdhci".into(),options:toml::from_str("[controller]\nbase=4291821568\nprotocol='sdhc'\ntransfer_ticks=4\nclock_ticks=2\nrca=1\nram_bytes=4294967296").unwrap()};
        register(&mut graph, &window, &mut media).unwrap();
        assert!(require_hosts(&media).is_ok());
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
    #[test]
    fn uhs2_port_shares_the_slot_contacts_and_needs_its_card_on_a_uhs1_host() {
        let assets = BTreeMap::from([("setup".into(), vec![0x12, 0x34])]);
        let mut empty = input();
        empty.present = false;
        let mut media = resolve(vec![empty], &assets).unwrap();
        let mut graph = DeviceGraph::new();
        let port = DeviceWindow {
            base: 0xffd02000,
            size: 0x300,
            kind: "sdhci-uhs2".into(),
            options: toml::from_str("slot=4291821568\ncapabilities=34359738368\nclock_ticks=10").unwrap(),
        };
        register_uhs2(&mut graph, &port, &media).unwrap();
        assert!(graph.at(0xffd02224).is_ok());
        assert!(require_hosts(&media).unwrap_err().contains("no SD host"));
        let host = DeviceWindow {base:0xffd00000,size:0x1e0,kind:"sdhci".into(),options:toml::from_str("[controller]\nbase=4291821568\nprotocol='sdhc'\ntransfer_ticks=4\nclock_ticks=2\nrca=1\nram_bytes=4294967296").unwrap()};
        register(&mut graph, &host, &mut media).unwrap();
        require_hosts(&media).unwrap();
        let mut wrong = port.clone();
        wrong.size = 0x200;
        assert!(register_uhs2(&mut DeviceGraph::new(), &wrong, &media).is_err());
        let mut unknown = port.clone();
        unknown.options.insert("slot".into(), toml::Value::Integer(0xffd04000));
        assert!(
            register_uhs2(&mut DeviceGraph::new(), &unknown, &media)
                .unwrap_err()
                .contains("card media")
        );
    }
}
