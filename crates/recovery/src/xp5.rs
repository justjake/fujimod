//! Raw XP5 entry, serial NOR preparation, and optional post-DDR FAT32 loading.
use crate::image::{self, File, Result, Source, extent, put, require, word};
use serde::{Deserialize, Serialize};
mod banks;
pub use banks::Banks;
mod card_boot;
pub use card_boot::CardBoot;
mod template {
    include!(concat!(env!("OUT_DIR"), "/xp5_sd_loader.rs"));
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Operation {
    pub address: u64,
    pub mask: u32,
    pub value: u32,
    pub kind: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Board {
    pub host: u64,
    pub timeout_setting: u16,
    pub operations: Vec<Operation>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodeProof {
    pub address: usize,
    pub size: usize,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceProof {
    pub source: Source,
    pub offset: usize,
    pub size: usize,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterWrite {
    pub address: u64,
    pub value: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Flash {
    pub controller: u64,
    pub aperture: u64,
    pub window_mask: u32,
    pub clock: u64,
    pub clock_commit: u64,
    pub pins: Vec<RegisterWrite>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lamp {
    pub pio_base: u64,
    pub clock_address: u64,
    pub clock_mask: u32,
    pub reset_address: u64,
    pub reset_mask: u32,
    pub green_port: u32,
    pub red_port: u32,
    pub pin: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Setup {
    pub prefix: Source,
    pub low_length: usize,
    pub length_flags: u32,
    pub base: u64,
    pub stack: u64,
    pub bootstrap_offset: usize,
    pub initialize_offset: usize,
    pub gate_offset: usize,
    pub continuation: u64,
    pub initializer: CodeProof,
    pub delay: CodeProof,
    pub reset_dat: usize,
    pub reset_copy_size: usize,
    pub selector: usize,
    pub flash: Flash,
    pub lamp: Lamp,
    pub board: Board,
    pub proofs: Vec<SourceProof>,
    pub banks: Banks,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_boot: Option<CardBoot>,
}
fn put64(data: &mut [u8], at: usize, value: u64) -> Result<()> {
    extent(data, at, 8)?;
    data[at..at + 8].copy_from_slice(&value.to_le_bytes());
    Ok(())
}
fn branch(at: u64, target: u64, link: bool) -> Result<u32> {
    let delta = i128::from(target) - i128::from(at);
    require(
        delta % 4 == 0 && (-(1 << 27)..(1 << 27)).contains(&delta),
        "AArch64 branch outside immediate range",
    )?;
    Ok((if link { 0x94000000 } else { 0x14000000 }) | (((delta >> 2) as u32) & 0x03ffffff))
}
fn mmio(address: u64) -> bool {
    (0xf0000000..0xfffff000).contains(&address) && address.is_multiple_of(4)
}
impl Setup {
    fn verify(&self, inverted: &[u8]) -> Result<Vec<u8>> {
        let prefix = self.prefix.read(inverted)?;
        require(
            prefix.len() == 0x40000
                && self.low_length >= 4
                && self.low_length <= prefix.len() - 0x800,
            "XP5 prefix extent differs",
        )?;
        require(
            self.base.is_multiple_of(0x4000) && (0xff000000..0xffff0000).contains(&self.base),
            "Invalid XP5 SRAM base",
        )?;
        require(
            self.length_flags & !0xc0000000 == 0
                && word(&prefix, 0x10)? == self.length_flags | self.low_length as u32,
            "XP5 setup length field differs",
        )?;
        require(
            self.bootstrap_offset >= 0x800
                && self
                    .bootstrap_offset
                    .checked_add(48)
                    .is_some_and(|end| end <= self.low_length + 0x800)
                && self.gate_offset == self.bootstrap_offset + 40
                && self.gate_offset >= 0x800
                && self
                    .gate_offset
                    .checked_add(12)
                    .is_some_and(|end| end <= self.low_length + 0x800),
            "XP5 bootstrap escapes setup",
        )?;
        let mut sources = std::collections::BTreeMap::new();
        for proof in &self.proofs {
            let key = serde_json::to_string(&proof.source)
                .map_err(|e| image::Error::Invalid(e.to_string()))?;
            if !sources.contains_key(&key) {
                sources.insert(key.clone(), proof.source.read(inverted)?);
            }
            image::hash(
                extent(&sources[&key], proof.offset, proof.size)?,
                &proof.sha256,
            )?;
        }
        require(
            !self.proofs.is_empty(),
            "XP5 device bindings lack source proofs",
        )?;
        for proof in [&self.initializer, &self.delay] {
            let offset = (proof.address as u64)
                .checked_sub(self.base)
                .ok_or_else(|| image::Error::Invalid("XP5 code proof precedes setup".into()))?
                as usize;
            require(
                proof.size > 0
                    && offset
                        .checked_add(proof.size)
                        .is_some_and(|end| end <= self.low_length),
                "XP5 code proof escapes setup",
            )?;
            image::hash(extent(&prefix, offset + 0x800, proof.size)?, &proof.sha256)?;
        }
        require(
            self.initialize_offset >= 0x800
                && self.initializer.address as u64
                    == self.base + self.initialize_offset as u64 - 0x800,
            "XP5 initializer binding differs",
        )?;
        Ok(prefix)
    }
}
/// Card entry replays its checked header without probing installed flash.
/// Stage one probes installed flash before retaining its stock continuation.
pub fn package(
    inverted: &[u8],
    setup: &Setup,
    ram: Option<Vec<u8>>,
    success_hold_seconds: u32,
) -> Result<Vec<File>> {
    package_with_storage(inverted, setup, ram, success_hold_seconds, None)
}

pub(crate) fn package_with_storage(
    inverted: &[u8], setup: &Setup, ram: Option<Vec<u8>>, success_hold_seconds: u32,
    storage: Option<&[u8]>,
) -> Result<Vec<File>> {
    let prefix = setup.verify(inverted)?;
    let copied_end = setup.base + setup.low_length as u64;
    // XP5 raw-entry SRAM reserves its upper workspace for the boot service.
    let workspace = setup.base + 0x38800;
    let address = (copied_end.max(setup.stack + 0x100) + 0x3fff) & !0x3fff;
    let capacity = workspace.saturating_sub(address);
    require(
        copied_end < setup.stack && setup.stack < workspace && capacity >= 0x4000,
        "No XP5 extension above the stock stack and below boot workspace",
    )?;
    let count = word(&prefix, 0x14)? as usize;
    require(count <= 253, "XP5 header operations exceed header")?;
    let header_size = 0x800;
    let mut program = template::IMAGE.to_vec();
    let header_at = program.len();
    program.extend_from_slice(extent(&prefix, 0, header_size)?);
    let mut resident_end = setup
        .banks
        .end(setup.low_length, (workspace - setup.base) as usize)?;
    if let Some(card) = &setup.card_boot {
        resident_end = resident_end.max(card.resident_end()?);
    }
    let end = (address + program.len() as u64).max(
        setup.base
            + if ram.is_some() {
                resident_end as u64
            } else {
                0
            },
    );
    let end = (end + 0x1ff) & !0x1ff;
    require(
        end <= address + capacity && end <= workspace,
        "XP5 SD loader exceeds SRAM workspace",
    )?;
    require(
        template::PRELUDE_STACK_MAX + 512
            <= template::XP5_SETUP_STACK_TOP - template::XP5_SETUP_STACK,
        "XP5 prelude exceeds private stack",
    )?;
    let cfg = template::XP5_SETUP_CONFIGURATION;
    let mut empty = vec![0; 208];
    empty[0] = 2;
    require(
        extent(&program, cfg, 208)? == empty,
        "XP5 setup configuration template changed",
    )?;
    let flash = &setup.flash;
    let lamp = &setup.lamp;
    require(
        [
            flash.controller,
            flash.aperture,
            flash.clock,
            flash.clock_commit,
            lamp.pio_base,
            lamp.clock_address,
            lamp.reset_address,
        ]
        .into_iter()
        .all(mmio)
            && flash.pins.len() == 6
            && flash.pins.iter().all(|p| mmio(p.address))
            && flash.window_mask != 0
            && lamp.clock_mask != 0
            && lamp.reset_mask != 0
            && lamp.green_port < 16
            && lamp.red_port < 16
            && lamp.green_port != 3
            && lamp.red_port != 3
            && lamp.green_port != lamp.red_port
            && lamp.pin < 8,
        "Invalid XP5 flash/lamp binding",
    )?;
    let stock = extent(&prefix, 0x800, setup.low_length)?;
    let first = word(stock, 0)?;
    let jump = branch(
        setup.base,
        address + template::XP5_SETUP_ENTRY as u64,
        false,
    )?;
    for (n, value) in [
        2,
        flash.controller,
        flash.aperture,
        u64::from(flash.window_mask),
        flash.clock,
        flash.clock_commit,
        1000000,
        setup.base,
        u64::from(first),
        u64::from(jump),
        lamp.pio_base,
        lamp.clock_address,
        u64::from(lamp.clock_mask),
        lamp.reset_address,
        u64::from(lamp.reset_mask),
    ]
    .into_iter()
    .enumerate()
    {
        put64(&mut program, cfg + n * 8, value)?;
    }
    for (n, value) in [lamp.green_port, lamp.red_port, lamp.pin, count as u32]
        .into_iter()
        .enumerate()
    {
        put(&mut program, cfg + 120 + n * 4, value)?;
    }
    for (n, pin) in flash.pins.iter().enumerate() {
        put(&mut program, cfg + 136 + n * 8, pin.address as u32)?;
        put(&mut program, cfg + 140 + n * 8, pin.value)?;
    }
    put64(&mut program, cfg + 184, address + header_at as u64)?;
    put64(
        &mut program,
        cfg + 192,
        // Each unit executes 1,000,000 four-NOP iterations. Even allowing
        // two billion iterations per second, 2,048 units take >= 1.024 s.
        u64::from(success_hold_seconds) * 2_048,
    )?;
    put64(&mut program, cfg + 200, u64::from(ram.is_none()))?;
    let restore = template::XP5_SETUP_RETURN_BRANCH;
    require(
        word(&program, restore)? == 0x14000000,
        "XP5 restore branch template changed",
    )?;
    put(
        &mut program,
        restore,
        branch(address + restore as u64, setup.base, false)?,
    )?;
    let call = setup.base + setup.bootstrap_offset as u64 - 0x800 + 36;
    let gate = setup.base + setup.gate_offset as u64 - 0x800;
    for &(at, kind, name) in template::EXTERNALS {
        let target = match name {
            "boot_setup_initialize" => setup.initializer.address as u64,
            "boot_setup_delay" => setup.delay.address as u64,
            _ => {
                return Err(image::Error::Invalid(
                    "Unexpected XP5 SD loader external".into(),
                ));
            }
        };
        put(
            &mut program,
            at,
            branch(address + at as u64, target, kind == 283)?,
        )?;
    }
    let mut raw = vec![0xff; (end - setup.base) as usize];
    raw[..stock.len()].copy_from_slice(stock);
    put(&mut raw, 0, jump)?;
    let mut files = Vec::new();
    if let Some(mut ram) = ram {
        require(
            !ram.is_empty()
                && ram.len() < 1 << 30
                && setup
                    .selector
                    .checked_add(8)
                    .is_some_and(|end| end <= setup.reset_copy_size)
                && setup.reset_copy_size <= ram.len(),
            "XP5 payload/reset extent differs",
        )?;
        let mut reset = extent(inverted, setup.reset_dat, setup.reset_copy_size)?.to_vec();
        require(
            word(&reset, setup.selector)? == 1 && word(&reset, setup.selector + 4)? == 0,
            "XP5 stock reset selector differs",
        )?;
        put64(&mut reset, setup.selector, 0)?;
        require(
            reset == ram[..setup.reset_copy_size],
            "XP5 reset copy differs beyond preloaded selector",
        )?;
        require(
            word(&prefix, setup.bootstrap_offset + 36)?
                == branch(call, setup.initializer.address as u64, true)?,
            "XP5 initializer call differs",
        )?;
        let load = word(&prefix, setup.gate_offset)?;
        require(
            load & 0xff00001f == 0x5800001e && word(&prefix, setup.gate_offset + 4)? == 0xd65f03c0,
            "XP5 bootstrap continuation differs",
        )?;
        let displacement = (((load >> 5) & 0x7ffff) as i64 ^ 0x40000) - 0x40000;
        let literal = (setup.gate_offset as i64 + displacement * 4)
            .try_into()
            .map_err(|_| image::Error::Invalid("XP5 zero literal escapes prefix".into()))?;
        require(
            extent(&prefix, literal, 8)? == setup.continuation.to_le_bytes(),
            "XP5 continuation literal differs",
        )?;
        let board = &setup.board;
        let cfg = template::SETUP_SD_BOARD;
        require(
            board.operations.len() <= 32 && board.timeout_setting <= 14 && mmio(board.host),
            "Invalid XP5 SD binding",
        )?;
        require(
            extent(&program, cfg, 16 + 32 * 24)?.iter().all(|v| *v == 0),
            "XP5 board template changed",
        )?;
        put64(&mut program, cfg, board.host)?;
        put(&mut program, cfg + 8, u32::from(board.timeout_setting))?;
        put(&mut program, cfg + 12, board.operations.len() as u32)?;
        for (n, op) in board.operations.iter().enumerate() {
            require(
                op.kind <= 2 && (op.kind == 2 || mmio(op.address)),
                "Invalid XP5 SD operation",
            )?;
            let at = cfg + 16 + n * 24;
            put64(&mut program, at, op.address)?;
            put(&mut program, at + 8, op.mask)?;
            put(&mut program, at + 12, op.value)?;
            put(&mut program, at + 16, op.kind)?;
        }
        let request = template::SETUP_SD_REQUEST;
        require(
            extent(&program, request, 40)? == [1u64, 0, 0, 0, 0].map(u64::to_le_bytes).concat(),
            "XP5 request template changed",
        )?;
        for (n, value) in [
            1,
            ram.len() as u64,
            u64::from(crate::xp4::crc32(&ram)),
            100,
            100000,
        ]
        .into_iter()
        .enumerate()
        {
            put64(&mut program, request + n * 8, value)?;
        }
        put(
            &mut raw,
            (call - setup.base) as usize,
            branch(call, address + template::SETUP_SD_INITIALIZE as u64, true)?,
        )?;
        put(
            &mut raw,
            (gate - setup.base) as usize,
            branch(gate, address + template::A64_BOOT_ZERO_GATE as u64, false)?,
        )?;
        if let Some(card) = &setup.card_boot {
            card.copy_unit_cache(storage, &mut raw, setup.low_length,
                (setup.stack-setup.base) as usize,
                (address-setup.base) as usize..(address-setup.base) as usize+program.len(),
                &setup.banks)?;
            card.install(inverted, storage, card_boot::Installation {
                base: setup.base, stock, program_address: address,
                raw: &mut raw, program: &mut program, ram: &mut ram,
            })?;
        } else {
            require(storage.is_none(), "Setup has no card storage binding")?;
        }
        files.push(File::new("ffboot.bin", ram)?);
    }
    require(
        extent(&program, template::SETUP_SD_PROGRESS, 64)?
            .iter()
            .all(|v| *v == 0)
            && word(&program, template::A64_BOOT_READY)? == 0,
        "XP5 handoff is already published",
    )?;
    let offset = (address - setup.base) as usize;
    raw[offset..offset + program.len()].copy_from_slice(&program);
    if !files.is_empty() {
        setup.banks.apply(
            &prefix,
            &mut raw,
            setup.low_length,
            (setup.stack - setup.base) as usize,
            offset,
            offset + program.len(),
        )?;
    }
    files.insert(0, File::new("marble_init.bin", raw)?);
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source(bytes: &[u8]) -> Source {
        Source {
            dat_offset: 0,
            size: bytes.len(),
            compression: image::Compression::Raw,
            decoded_size: bytes.len(),
            sha256: fujimod_firmware::sha256(bytes),
            initialized_prefix: None,
        }
    }
    fn fixture() -> (Vec<u8>, Setup, Vec<u8>) {
        let base = 0xff800000;
        let mut data = vec![0xff; 0x41000];
        for (at, value) in [
            (0x10, 0x4000),
            (0x14, 1),
            (0x18, 0xff900000),
            (0x1c, 7),
            (0x800, 0xd503201f),
            (0xa00, 0xd503201f),
            (0xb00, 0xd503201f),
            (0x924, branch(base + 0x124, base + 0x200, true).unwrap()),
            (0x928, 0x5800005e),
            (0x92c, 0xd65f03c0),
        ] {
            put(&mut data, at, value).unwrap();
        }
        put64(&mut data, 0x930, base + 0x3000).unwrap();
        put64(&mut data, 0x40040, 1).unwrap();
        let prefix = source(&data[..0x40000]);
        let setup = Setup {
            prefix: prefix.clone(),
            low_length: 0x4000,
            length_flags: 0,
            base,
            stack: base + 0x8000,
            bootstrap_offset: 0x900,
            initialize_offset: 0xa00,
            gate_offset: 0x928,
            continuation: base + 0x3000,
            initializer: CodeProof {
                address: (base + 0x200) as usize,
                size: 4,
                sha256: fujimod_firmware::sha256(&data[0xa00..0xa04]),
            },
            delay: CodeProof {
                address: (base + 0x300) as usize,
                size: 4,
                sha256: fujimod_firmware::sha256(&data[0xb00..0xb04]),
            },
            reset_dat: 0x40000,
            reset_copy_size: 0x1000,
            selector: 0x40,
            flash: Flash {
                controller: 0xff900000,
                aperture: 0xf0000000,
                window_mask: 0xf8000000,
                clock: 0xffa10000,
                clock_commit: 0xffa10040,
                pins: (0..6)
                    .map(|n| RegisterWrite {
                        address: 0xff901000 + n * 4,
                        value: 0,
                    })
                    .collect(),
            },
            lamp: Lamp {
                pio_base: 0xff902000,
                clock_address: 0xff903000,
                clock_mask: 1,
                reset_address: 0xff904000,
                reset_mask: 2,
                green_port: 4,
                red_port: 5,
                pin: 1,
            },
            board: Board {
                host: 0xff905000,
                timeout_setting: 14,
                operations: vec![Operation {
                    address: 0xff906000,
                    mask: 0,
                    value: 1,
                    kind: 0,
                }],
            },
            proofs: vec![SourceProof {
                source: prefix,
                offset: 0,
                size: 4,
                sha256: fujimod_firmware::sha256(&data[..4]),
            }],
            banks: Banks {
                copies: vec![],
                edits: vec![],
            },
            card_boot: None,
        };
        let mut ram = data[0x40000..].to_vec();
        put64(&mut ram, 0x40, 0).unwrap();
        (data, setup, ram)
    }
    #[test]
    fn startup_skips_installed_flash_probe_while_stage_one_preserves_it() {
        let (data, s, ram) = fixture();
        for payload in [None, Some(ram.clone())] {
            let combined = payload.is_some();
            let files = package(&data, &s, payload, 0).unwrap();
            let raw = &files[0].bytes;
            assert_eq!(files.len(), if combined { 2 } else { 1 });
            assert_eq!(files[0].name, "marble_init.bin");
            let source = 0xc000 + template::XP5_SETUP_CONFIGURATION + 200;
            assert_eq!(
                u64::from_le_bytes(raw[source..source + 8].try_into().unwrap()),
                u64::from(!combined)
            );
            let address = s.base + 0xc000;
            assert_eq!(
                word(raw, 0).unwrap(),
                branch(s.base, address + template::XP5_SETUP_ENTRY as u64, false).unwrap()
            );
            let mut restored = raw[..s.low_length].to_vec();
            put(&mut restored, 0, word(&data, 0x800).unwrap()).unwrap();
            if combined {
                assert_eq!(files[1].name, "ffboot.bin");
                assert_eq!(files[1].bytes, ram);
                for at in [0x124, 0x128] {
                    put(&mut restored, at, word(&data, at + 0x800).unwrap()).unwrap();
                }
                let request = 0xc000 + template::SETUP_SD_REQUEST;
                assert_eq!(
                    u64::from_le_bytes(raw[request + 8..request + 16].try_into().unwrap()),
                    ram.len() as u64
                );
                assert_eq!(word(raw, request + 16).unwrap(), crate::xp4::crc32(&ram));
            }
            assert_eq!(restored, data[0x800..0x800 + s.low_length]);
            let header = 0xc000 + template::IMAGE.len();
            assert_eq!(&raw[header..header + 0x800], &data[..0x800]);
            assert!(raw.len() <= 0x38800);
            assert_eq!(
                word(raw, 0xc000 + template::XP5_SETUP_RETURN_BRANCH).unwrap(),
                branch(
                    address + template::XP5_SETUP_RETURN_BRANCH as u64,
                    s.base,
                    false
                )
                .unwrap()
            );
        }
    }
    #[test]
    fn success_hold_is_optional_and_changes_only_its_configuration() {
        let (data, s, ram) = fixture();
        assert_eq!(crate::StartupOptions::default().success_hold_seconds, 0);
        for payload in [None, Some(ram)] {
            let normal = package(&data, &s, payload.clone(), 0).unwrap();
            let at = 0xc000 + template::XP5_SETUP_CONFIGURATION + 192;
            assert_eq!(&normal[0].bytes[at..at + 8], &[0; 8]);
            for seconds in [1, 3, u32::MAX] {
                let held = package(&data, &s, payload.clone(), seconds).unwrap();
                let mut expected = normal[0].bytes.clone();
                put64(&mut expected, at, u64::from(seconds) * 2_048).unwrap();
                assert_eq!(expected, held[0].bytes);
                if normal.len() == 2 {
                    assert_eq!(normal[1].bytes, held[1].bytes);
                }
            }
        }
    }

    #[test]
    fn changed_source_and_incomplete_bindings_refuse() {
        let (data, s, ram) = fixture();
        let mut changed = data.clone();
        changed[0x1000] ^= 1;
        assert!(package(&changed, &s, None, 0).is_err());
        let mut changed = s.clone();
        changed.flash.pins.clear();
        assert!(package(&data, &changed, None, 0).is_err());
        let mut changed = s.clone();
        changed.initializer.sha256 = "0".repeat(64);
        assert!(package(&data, &changed, None, 0).is_err());
        let mut changed = s.clone();
        changed.board.operations[0].kind = 3;
        assert!(package(&data, &changed, Some(ram.clone()), 0).is_err());
        let mut changed = s.clone();
        changed.gate_offset = 0x940;
        assert!(package(&data, &changed, Some(ram), 0).is_err());
        let mut changed = s.clone();
        changed.stack = s.base + 0x38000;
        assert!(package(&data, &changed, None, 0).is_err());
    }
    #[test]
    fn oversized_header_and_changed_payload_refuse() {
        let (mut data, mut s, mut ram) = fixture();
        ram[0x20] ^= 1;
        assert!(package(&data, &s, Some(ram), 0).is_err());
        put(&mut data, 0x14, 254).unwrap();
        s.prefix = source(&data[..0x40000]);
        s.proofs[0].source = s.prefix.clone();
        assert!(package(&data, &s, None, 0).is_err());
    }
}
