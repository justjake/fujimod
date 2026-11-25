use fujimod_emulator_devices::graph::DeviceGraph;
use fujimod_emulator_xp5_io::clocks::{BANKS, Bank, Reset};

/// Options of a `clock-banks` window: declared start state of chosen banks.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Options {
    #[serde(default)]
    banks: Vec<Reset>,
}

pub fn register(
    bus: &mut DeviceGraph,
    base: u64,
    size: u64,
    options: &toml::Table,
) -> Result<(), String> {
    register_except(bus, base, size, &[], options)
}
/// Explicit specialized banks (e.g. display pixel clocks) retain their owner.
pub fn register_except(
    bus: &mut DeviceGraph,
    base: u64,
    size: u64,
    specialized: &[u64],
    options: &toml::Table,
) -> Result<(), String> {
    if base != 0xffa00000 || size != 0x20000 {
        return Err("clock-banks requires ffa00000..ffa20000".into());
    }
    if specialized
        .iter()
        .any(|base| !BANKS.iter().any(|(b, _)| b == base))
    {
        return Err("specialized clock bank outside stock table".into());
    }
    let options: Options = toml::Value::Table(options.clone())
        .try_into()
        .map_err(|e| format!("clock-banks options: {e}"))?;
    for (i, reset) in options.banks.iter().enumerate() {
        if !BANKS.iter().any(|(b, _)| *b == reset.base) {
            return Err(format!(
                "clock-banks options: {:#x} is not a clock bank",
                reset.base
            ));
        }
        if options.banks[..i].iter().any(|r| r.base == reset.base) {
            return Err(format!(
                "clock-banks options: {:#x} declared twice",
                reset.base
            ));
        }
        if specialized.contains(&reset.base) {
            return Err(format!(
                "clock-banks options: {:#x} belongs to a specialized window",
                reset.base
            ));
        }
    }
    for (base, _) in BANKS {
        if specialized.contains(&base) {
            continue;
        }
        let reset = options.banks.iter().find(|r| r.base == base);
        bus.register(base, 0x1000, Box::new(Bank::with_reset(base, reset)?))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::{Cx, test_host::recording::RecordingHost};

    fn options(text: &str) -> toml::Table {
        text.parse().unwrap()
    }
    fn bank(extra: &str) -> String {
        format!(
            "[[banks]]\nbase = 0xffa01000\nn = 7\nfraction = 9\ndividers = [1, 2, 3, 4]\nidle_status = 0x20\n{extra}"
        )
    }
    fn build(specialized: &[u64], text: &str) -> Result<DeviceGraph, String> {
        let mut bus = DeviceGraph::new();
        register_except(&mut bus, 0xffa00000, 0x20000, specialized, &options(text))?;
        Ok(bus)
    }

    #[test]
    fn declared_bank_is_programmed_and_others_start_zero() {
        let mut bus = build(&[], &bank("")).unwrap();
        let mut host = RecordingHost::default();
        for (address, expected) in [
            (0xffa01000, 7),
            (0xffa01008, 9),
            (0xffa01040 + 12, 4),
            (0xffa010f0, 0x20),
            (0xffa02000, 0),
            (0xffa020f0, 0),
        ] {
            let node = bus.at(address).unwrap();
            let mut cx = Cx::new(&mut host, 0, 0, 0);
            assert_eq!(bus.nodes[node].device.read(&mut cx, address, 4), expected);
        }
        assert!(register(&mut DeviceGraph::new(), 0xffa00000, 0x20000, &options("")).is_ok());
    }

    #[test]
    fn invalid_options_refuse() {
        let cases = [
            ("dividers = [1, 2, 3, 4]\n", "dividers = [1, 2, 3]\n"),
            ("dividers = [1, 2, 3, 4]\n", "dividers = [1, 2, 3, 512]\n"),
            ("idle_status = 0x20\n", "idle_status = 0x21\n"),
            ("base = 0xffa01000\n", "base = 0xffa06000\n"),
        ];
        for (from, to) in cases {
            let text = bank("").replace(from, to);
            assert!(build(&[], &text).is_err(), "{to}");
        }
        let err = build(&[], &format!("{}{}", bank(""), bank("")))
            .err()
            .unwrap();
        assert!(err.contains("declared twice"), "{err}");
        let err = build(&[0xffa01000], &bank("")).err().unwrap();
        assert!(err.contains("specialized"), "{err}");
        let err = build(&[], &format!("unknown = 1\n{}", bank("")))
            .err()
            .unwrap();
        assert!(err.starts_with("clock-banks options: "), "{err}");
        let err = build(&[], &bank("extra = 1\n")).err().unwrap();
        assert!(err.starts_with("clock-banks options: "), "{err}");
    }
}
