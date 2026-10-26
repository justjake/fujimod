//! Offline preparation and staging of compiled stock-reader payloads.
use super::{CodeCheck, Payload, Range, Transport, WordCheck};
use fujimod_catalog::{ReferenceCatalog, ResolvedSelection};
use fujimod_firmware::{extent, invert, sha256};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub firmware_id: String,
    pub update: PathBuf,
    pub load_map: PathBuf,
    pub callback: PathBuf,
    pub bulk: PathBuf,
    pub manifest: PathBuf,
    /// Explicit compiler and argv; no shell is used.
    pub command: Vec<String>,
    /// Browser catalog display and update-download metadata.
    pub display: Value,
    #[serde(default)]
    pub index: Option<PathBuf>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stage {
    pub catalog: PathBuf,
    pub runtime: PathBuf,
    pub selections: Vec<Selection>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub payload: Payload,
    pub image: String,
}
fn number(v: &Value) -> Result<u64> {
    if let Some(n) = v.as_u64() {
        return Ok(n);
    }
    if let Some(s) = v.as_str() {
        return Ok(if let Some(s) = s.strip_prefix("0x") {
            u64::from_str_radix(s, 16)?
        } else {
            s.parse()?
        });
    }
    Err("expected an unsigned address or size".into())
}
fn get<'a>(v: &'a Value, k: &str) -> Result<&'a Value> {
    v.get(k).ok_or_else(|| format!("missing {k}").into())
}
fn num(v: &Value, k: &str) -> Result<u64> {
    number(get(v, k)?)
}
fn text<'a>(v: &'a Value, k: &str) -> Result<&'a str> {
    get(v, k)?
        .as_str()
        .ok_or_else(|| format!("expected text {k}").into())
}
fn list<'a>(v: &'a Value, k: &str) -> Result<&'a Vec<Value>> {
    get(v, k)?
        .as_array()
        .ok_or_else(|| format!("expected array {k}").into())
}
fn document(path: &Path) -> Result<Value> {
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}
fn check(ok: bool, message: &str) -> Result<()> {
    if ok { Ok(()) } else { Err(message.into()) }
}
fn one<'a>(items: &'a [Value], key: &str, name: &str) -> Result<&'a Value> {
    let matches: Vec<_> = items.iter().filter(|s| s[key] == name).collect();
    check(matches.len() == 1, "missing or ambiguous build symbol")?;
    Ok(matches[0])
}

struct Source {
    address: u64,
    bytes: Vec<u8>,
    bss: u64,
    bss_size: u64,
}
fn sources(map: &Value, inverted: &[u8]) -> Result<Vec<Source>> {
    let units = list(&map["facts"]["load_map"]["value"], "units")?;
    let mut result = Vec::new();
    for unit in units
        .iter()
        .filter(|u| u["role"] == "resident" || u["role"] == "kernel")
    {
        let source = get(unit, "source")?;
        let packed = extent(inverted, num(source, "dat_offset")?, num(source, "size")?)?;
        let mut bytes = match text(source, "compression")? {
            "raw" => packed.to_vec(),
            "method4" => {
                let (data, header) = fujimod_firmware::decompress(packed)?;
                check(
                    u64::from(header.packed_size) <= packed.len() as u64,
                    "compressed source exceeds allocated extent",
                )?;
                data
            }
            _ => return Err("unsupported initialized-source compression".into()),
        };
        check(
            bytes.len() as u64 == num(source, "decoded_size")?
                && sha256(&bytes) == text(source, "sha256")?,
            "decoded source identity differs",
        )?;
        if let Some(prefix) = source.get("initialized_prefix") {
            let size = num(prefix, "size")? as usize;
            check(
                size <= bytes.len()
                    && sha256(&bytes[..size]) == text(prefix, "sha256")?
                    && bytes[size..].iter().all(|b| *b == 0),
                "initialized source prefix differs",
            )?;
            bytes.truncate(size);
        }
        check(
            bytes.len() as u64 <= num(unit, "declared_size")?,
            "initialized source exceeds its load extent",
        )?;
        result.push(Source {
            address: num(unit, "address")?,
            bytes,
            bss: num(unit, "bss")?,
            bss_size: num(unit, "bss_size")?,
        });
    }
    Ok(result)
}
fn source(units: &[Source], address: u64, size: u64) -> Result<&[u8]> {
    let end = address.checked_add(size).ok_or("source extent overflows")?;
    for unit in units {
        if address >= unit.address && end <= unit.address + unit.bytes.len() as u64 {
            return Ok(extent(&unit.bytes, address - unit.address, size)?);
        }
    }
    Err(format!("no initialized source for {address:#x}+{size:#x}").into())
}
fn code_checks(units: &[Source], ranges: &[Value]) -> Result<Vec<CodeCheck>> {
    ranges
        .iter()
        .map(|r| {
            let address = num(r, "address")?;
            let size = num(r, "size")?;
            Ok(CodeCheck {
                address,
                size,
                sha256: sha256(source(units, address, size)?),
            })
        })
        .collect()
}
fn symbol(manifest: &Value, name: &str, size: Option<u64>, base: u64, count: u64) -> Result<u64> {
    let s = one(list(manifest, "symbols")?, "name", name)?;
    let address = num(s, "address")?;
    let n = num(s, "size")?;
    check(
        n > 0
            && size.is_none_or(|expected| n == expected)
            && address >= base
            && address
                .checked_add(n)
                .is_some_and(|end| end <= base + count),
        "payload symbol extent differs",
    )?;
    Ok(address)
}

/// Verify source identities, executable mappings, symbols and the initial
/// update-backed control without constructing a CPU or opening a device.
pub fn prepare(input: &Selection, selected: &ResolvedSelection) -> Result<(Artifact, Vec<u8>)> {
    if selected
        .flash
        .as_ref()
        .is_some_and(|f| f.transport.as_deref() == Some("exr2-query-heap"))
    {
        return prepare_exr2(input, selected);
    }
    let flash = selected.flash.as_ref().ok_or("no catalog flash fact")?;
    let bulk = selected.bulk.as_ref().ok_or("no catalog bulk fact")?;
    check(
        flash.transport.as_deref() == Some("xp5-cmd-callback"),
        "unsupported reader transport",
    )?;
    let update_identity = selected
        .firmware
        .update
        .as_ref()
        .ok_or("no update identity")?;
    let update = fs::read(&input.update)?;
    check(
        update.len() as u64 == update_identity.size && sha256(&update) == update_identity.sha256,
        "update identity differs",
    )?;
    let inverted = invert(&update);
    let manifest = document(&input.manifest)?;
    check(
        manifest["target"] == selected.firmware.id
            && manifest["firmware_sha256"] == update_identity.sha256
            && manifest["arch"] == "aarch64"
            && manifest["abi"] == "aapcs64-lp64-le"
            && manifest["mode"] == "emulator"
            && manifest["live_safe"] == false
            && list(&manifest, "inits")?.is_empty()
            && list(&manifest, "words")?.is_empty(),
        "unsupported payload build or identity",
    )?;
    let regions = list(&manifest, "regions")?;
    check(regions.len() == 1, "payload needs one region")?;
    let region = &regions[0];
    let file = text(region, "file")?;
    check(
        Path::new(file).components().count() == 1
            && !matches!(file, "." | "..")
            && !Path::new(file).is_absolute(),
        "payload image must be beside the manifest",
    )?;
    let image = fs::read(
        input
            .manifest
            .parent()
            .ok_or("manifest directory")?
            .join(file),
    )?;
    check(
        image.len() as u64 == num(region, "size")? && sha256(&image) == text(region, "sha256")?,
        "compiled image identity differs",
    )?;
    let profile = document(&input.callback)?;
    let base = num(&profile["arena"], "address")?;
    let capacity = num(&profile["arena"], "size")?;
    check(
        profile["schema"] == "xp5-cmd-callback-experiment-v2"
            && profile["firmware_id"] == selected.firmware.id
            && num(&manifest["region"], "address")? == base
            && num(&manifest["region"], "size")? == capacity
            && num(region, "address")? == base,
        "callback profile and compiled arena differ",
    )?;
    let map = document(&input.load_map)?;
    check(
        map["format_version"] == 2
            && map["firmware_id"] == selected.firmware.id
            && map["firmware_sha256"] == update_identity.sha256,
        "load-map identity differs",
    )?;
    let units = sources(&map, &inverted)?;
    let callback = num(&profile, "callback")?;
    check(
        callback.is_multiple_of(8)
            && units
                .iter()
                .any(|u| callback >= u.bss && callback + 8 <= u.bss + u.bss_size),
        "callback is not in resident BSS",
    )?;
    let cache = num(&profile, "instruction_cache_sync")?;
    check(
        sha256(source(&units, cache, 16)?)
            == "8d590103f11bcc2dc4b4f4fca056353bca5a3574e411f37d21fa00fe074c1cef",
        "cache primitive differs",
    )?;
    check(
        source(&units, num(&profile, "command_token")?, 4)? == b"CMD\0",
        "callback command token differs",
    )?;
    let mut state = document(&input.bulk)?;
    if let Some(value) = state.pointer("/facts/bulk_ram/value") {
        state = value.clone();
    }
    check(
        state["firmware_id"] == selected.firmware.id
            && state["firmware_sha256"] == update_identity.sha256
            && num(&state, "guard_offset")? == 0xa3
            && num(&state, "debug_offset")? == 0xf7
            && num(&state["system_mode"], "required")? == 7,
        "bulk state identity or service contract differs",
    )?;
    if let fujimod_catalog_bulk::BulkSource::PatternZeroBase {
        registered_buffer,
        source_state,
        capacity,
    } = bulk.source
    {
        check(
            registered_buffer == num(&state, "registered_buffer")?
                && source_state == num(&state, "source_state")?
                && u64::from(capacity) == num(&state, "capacity")?,
            "bulk source contract differs",
        )?;
    } else {
        return Err("callback requires zero-base bulk source".into());
    }
    let translation = get(&profile, "translation")?;
    let mut words = vec![WordCheck {
        address: num(translation, "root_pointer")?,
        size: 8,
        value: num(translation, "root")?,
    }];
    for row in list(translation, "path")? {
        let value = num(row, "value")?;
        let bit = if row["kind"] == "table" { 59 } else { 53 };
        check(
            value & 1 != 0 && value & (1 << bit) == 0,
            "arena mapping is not executable",
        )?;
        words.push(WordCheck {
            address: num(row, "address")?,
            size: 8,
            value,
        });
    }
    words.push(WordCheck {
        address: num(&state["system_mode"], "address")?,
        size: 4,
        value: 7,
    });
    let entry = symbol(&manifest, "flash_reader", None, base, image.len() as u64)?;
    check(
        num(
            one(list(&manifest, "entrypoints")?, "name", "flash_reader")?,
            "address",
        )? == entry,
        "entrypoint differs",
    )?;
    let stock = one(
        list(&manifest, "stock_symbols")?,
        "name",
        &flash.reader.symbol,
    )?;
    check(
        flash.update_image.dat_offset <= inverted.len() as u64,
        "update origin exceeds input",
    )?;
    let update_bytes = extent(
        &inverted,
        flash.update_image.dat_offset,
        (inverted.len() as u64 - flash.update_image.dat_offset).min(flash.window.size),
    )?;
    check(flash.update_image.xor == 255, "unsupported update XOR")?;
    let control = extent(update_bytes, flash.control.offset, flash.control.size)?;
    let mutable = list(&manifest, "symbols")?
        .iter()
        .filter(|s| s["type"] == "STT_OBJECT")
        .map(|s| {
            Ok(Range {
                address: num(s, "address")?,
                size: num(s, "size")?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let checks = flash
        .code_ranges
        .iter()
        .map(|r| {
            Ok(CodeCheck {
                address: r.address,
                size: r.size,
                sha256: sha256(source(&units, r.address, r.size)?),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let payload = super::xp5::Payload {
        schema: 1,
        firmware_id: selected.firmware.id.clone(),
        firmware_sha256: update_identity.sha256.clone(),
        address: base,
        capacity,
        sha256: sha256(&image),
        entry,
        request: symbol(
            &manifest,
            "flash_reader_request",
            Some(24),
            base,
            image.len() as u64,
        )?,
        result: symbol(
            &manifest,
            "flash_reader_result",
            Some(40),
            base,
            image.len() as u64,
        )?,
        buffer: symbol(
            &manifest,
            "flash_reader_buffer",
            None,
            base,
            image.len() as u64,
        )?,
        buffer_capacity: u32::try_from(num(
            one(list(&manifest, "symbols")?, "name", "flash_reader_buffer")?,
            "size",
        )?)?,
        stock_reader: num(stock, "address")?,
        stock_reader_symbol: flash.reader.symbol.clone(),
        mutable,
        transport: Transport {
            callback,
            instruction_cache_sync: cache,
            registered_scratch: Range {
                address: num(&profile["registered_scratch"], "address")?,
                size: num(&profile["registered_scratch"], "size")?,
            },
            configuration_pointer: num(&state, "configuration_pointer")?,
            configuration_bytes: num(&state, "configuration_bytes")?,
            guard_offset: 0xa3,
            debug_offset: 0xf7,
            checks: code_checks(&units, list(&profile, "code_ranges")?)?,
            words,
        },
        checks,
        control_sha256: sha256(control),
        update_size: update_bytes.len() as u64,
    };
    payload.validate(flash, bulk, &image)?;
    Ok((
        Artifact {
            payload: Payload::Xp5Command(payload),
            image: "flash-reader.bin".into(),
        },
        image,
    ))
}

/// Build every declared selection and stage metadata and binary images. The
/// runtime catalog is replaced only after all payloads validate successfully.
fn prepare_exr2(input: &Selection, selected: &ResolvedSelection) -> Result<(Artifact, Vec<u8>)> {
    let flash = selected.flash.as_ref().ok_or("no flash fact")?;
    let bulk = selected.bulk.as_ref().ok_or("no bulk fact")?;
    check(selected.soc.id == "exr2", "NAND execution requires EXR II")?;
    let identity = selected
        .firmware
        .update
        .as_ref()
        .ok_or("no update identity")?;
    let update = fs::read(&input.update)?;
    check(
        update.len() as u64 == identity.size && sha256(&update) == identity.sha256,
        "update identity differs",
    )?;
    let inverted = invert(&update);
    let map = document(&input.load_map)?;
    check(
        map["format_version"] == 2
            && map["firmware_id"] == selected.firmware.id
            && map["firmware_sha256"] == identity.sha256,
        "load-map identity differs",
    )?;
    let units = sources(&map, &inverted)?;
    let manifest = document(&input.manifest)?;
    check(
        manifest["target"] == selected.firmware.id
            && manifest["firmware_sha256"] == identity.sha256
            && manifest["mode"] == "runtime"
            && list(&manifest, "words")?.is_empty()
            && list(&manifest, "overrides")?.is_empty()
            && list(&manifest, "inits")?.is_empty(),
        "unsupported NAND payload build",
    )?;
    let regions = list(&manifest, "regions")?;
    check(regions.len() == 1, "NAND build requires one region")?;
    let region = &regions[0];
    let filename = text(region, "file")?;
    check(
        Path::new(filename).components().count() == 1
            && !matches!(filename, "." | "..")
            && !Path::new(filename).is_absolute(),
        "payload image must be beside the manifest",
    )?;
    let compiled = fs::read(
        input
            .manifest
            .parent()
            .ok_or("manifest directory")?
            .join(filename),
    )?;
    check(
        compiled.len() as u64 == num(region, "size")?
            && sha256(&compiled) == text(region, "sha256")?,
        "compiled NAND image differs",
    )?;
    let base = num(region, "address")?;
    let mut image = Vec::new();
    let mut sizes = Vec::new();
    for name in ["flash_reader_call", "flash_reader"] {
        let item = one(list(&manifest, "symbols")?, "name", name)?;
        let at = num(item, "address")?;
        let size = num(item, "size")?;
        check(
            at >= base && size > 0 && at.is_multiple_of(4) && size.is_multiple_of(4),
            "invalid NAND entry extent",
        )?;
        let bytes = extent(&compiled, at - base, size)?;
        // Runtime builds use word relocations. No code body may carry a pointer
        // into its link image: all runtime data and calls arrive as arguments.
        check(
            !bytes.as_chunks::<4>().0.iter().any(|b| {
                let v = u32::from_le_bytes(*b) as u64;
                v >= base && v < base + compiled.len() as u64
            }),
            "NAND entry has an absolute self reference",
        )?;
        image.extend(bytes);
        sizes.push(size);
        if name == "flash_reader_call" {
            check(size <= 32, "bootstrap exceeds borrowed slot")?;
            image.resize(32, 0);
        }
    }
    let index = fujimod_corpus::Store::open(
        input
            .index
            .as_ref()
            .ok_or("NAND build requires an explicit function index")?,
    )?;
    let execution = fujimod_corpus::locators::exr2_execution(&index, &selected.firmware.id)?;
    let mut ranges = list(&execution, "code_ranges")?.clone();
    use fujimod_corpus::Lookup;
    for range in &ranges {
        let at = num(range, "address")?;
        let size = num(range, "size")?;
        check(
            index
                .read(&selected.firmware.id, "main", at, size.try_into()?)?
                .as_deref()
                == Some(source(&units, at, size)?),
            "execution locator index differs from update source",
        )?;
    }
    // The EXR II CPU0 kernel's cache and barrier entrypoints are qualified as
    // complete immutable spans, including the shared cache helpers' literals.
    let kernel = units
        .iter()
        .find(|u| u.address == 0x40000)
        .ok_or("missing CPU0 kernel")?;
    let mut primitives = Vec::new();
    for (size, hash) in [
        (
            0x214,
            "10aad5ca7b798181283de15647d4cae836a2feaf0ce7363f23b430a0855d316c",
        ),
        (
            0x28,
            "4f5e46f3fcacb11933fd0f5e8fc33ae5837f738e38902a0e475c040ac5897890",
        ),
    ] {
        let matches: Vec<_> = kernel
            .bytes
            .windows(size)
            .enumerate()
            .step_by(4)
            .filter(|(_, b)| sha256(b) == hash)
            .map(|(i, _)| kernel.address + i as u64)
            .collect();
        check(
            matches.len() == 1,
            "missing or ambiguous EXR II kernel execution primitive",
        )?;
        let address = matches[0];
        ranges.push(json!({"address":address,"size":size}));
        primitives.push(address);
    }
    for range in &flash.code_ranges {
        ranges.push(json!({"address":range.address,"size":range.size}));
    }
    let update_size = (inverted.len() as u64)
        .checked_sub(flash.update_image.dat_offset)
        .ok_or("invalid NAND update origin")?
        .min(flash.window.size);
    check(flash.update_image.xor == 255, "unsupported NAND update XOR")?;
    let control = extent(
        &inverted,
        flash.update_image.dat_offset + flash.control.offset,
        flash.control.size,
    )?;
    let payload = super::exr2::Payload {
        schema: 1,
        firmware_id: selected.firmware.id.clone(),
        firmware_sha256: identity.sha256.clone(),
        sha256: sha256(&image),
        bootstrap_size: sizes[0] as u32,
        reader_offset: 32,
        allocate: num(&execution, "allocate")?.try_into()?,
        release: num(&execution, "release")?.try_into()?,
        configuration: serde_json::from_value(execution["configuration"].clone())?,
        cache_clean: (primitives[0] + 0x18).try_into()?,
        instruction_invalidate: (primitives[0] + 0xfc).try_into()?,
        barrier: primitives[1].try_into()?,
        instruction_barrier: primitives[0].try_into()?,
        checks: code_checks(&units, &ranges)?,
        control_sha256: sha256(control),
        update_size,
    };
    payload.validate(flash, bulk, &image)?;
    Ok((
        Artifact {
            payload: Payload::Exr2Query(payload),
            image: "flash-reader.bin".into(),
        },
        image,
    ))
}

pub fn stage(recipe: &Stage) -> Result<()> {
    let catalog = ReferenceCatalog::load(&recipe.catalog)?;
    let path = recipe.runtime.join("catalog.json");
    let mut runtime = document(&path)?;
    let entries = runtime["firmwares"]
        .as_array_mut()
        .ok_or("runtime firmware list")?;
    for input in &recipe.selections {
        let selected = catalog.resolve_firmware(&input.firmware_id)?;
        check(!input.command.is_empty(), "compiler command is empty")?;
        let status = Command::new(&input.command[0])
            .args(&input.command[1..])
            .status()?;
        check(status.success(), "reader compilation failed")?;
        let (artifact, image) = prepare(input, &selected)?;
        let directory = recipe.runtime.join(&input.firmware_id);
        fs::create_dir_all(&directory)?;
        fs::write(directory.join(&artifact.image), &image)?;
        let mut metadata = serde_json::to_vec_pretty(&artifact)?;
        metadata.push(b'\n');
        fs::write(directory.join("flash-reader.json"), metadata)?;
        let entry = if let Some(at) = entries.iter().position(|e| e["id"] == input.firmware_id) {
            &mut entries[at]
        } else {
            entries.push(input.display.clone());
            entries.last_mut().unwrap()
        };
        check(
            entry["sha256"] == artifact.payload.firmware_sha256(),
            "runtime identity differs",
        )?;
        entry["body"] = json!(selected.body.id);
        entry["flash_reader"] = json!(format!("runtime/{}/flash-reader.json", input.firmware_id));
        println!(
            "{}: staged {}-byte flash reader",
            input.firmware_id,
            image.len()
        );
    }
    let temporary = path.with_extension("json.tmp");
    let mut bytes = serde_json::to_vec_pretty(&runtime)?;
    bytes.push(b'\n');
    fs::write(&temporary, bytes)?;
    fs::rename(temporary, path)?;
    Ok(())
}
