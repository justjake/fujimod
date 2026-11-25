//! The XP4 board definition built from the catalog: the processor's blocks
//! with the selected body profile's overrides, and the body's inputs.
//!
//! Every value comes from a record. A block keeps its window, interrupt
//! outputs and device model; the body profile replaces a block's device only
//! through `block_overrides`.
use fujimod_catalog_resolution::ResolvedConfiguration;
use serde_json::{Map, Value, json};

/// One processor block with the device the selected body leaves in place.
#[derive(Clone, Copy)]
pub struct Block<'a> {
    pub raw: &'a Value,
    pub device: &'a Value,
}
impl<'a> Block<'a> {
    pub fn id(&self) -> &'a str {
        self.raw["id"].as_str().unwrap_or_default()
    }
    pub fn base(&self) -> u64 {
        self.raw["base"].as_u64().unwrap_or_default()
    }
    pub fn size(&self) -> u64 {
        self.raw["size"].as_u64().unwrap_or_default()
    }
    pub fn interrupts(&self) -> &'a [Value] {
        self.raw["interrupts"].as_array().map_or(&[], Vec::as_slice)
    }
    /// The line of the named output, or of the first output.
    pub fn interrupt(&self, output: Option<&str>) -> Option<u64> {
        self.interrupts()
            .iter()
            .find(|row| output.is_none_or(|name| row["output"] == name))
            .and_then(|row| row["line"].as_u64())
    }
    pub fn lines(&self) -> Vec<u64> {
        self.interrupts().iter().filter_map(|row| row["line"].as_u64()).collect()
    }
    pub fn field(&self, name: &str) -> Result<&'a Value, String> {
        self.device
            .get(name)
            .ok_or_else(|| format!("block {} declares no {name}", self.id()))
    }
    pub fn number(&self, name: &str) -> Result<u64, String> {
        self.field(name)?
            .as_u64()
            .ok_or_else(|| format!("block {} {name} is not a number", self.id()))
    }
}

/// The processor's blocks, the body profile and the processor record.
pub struct Catalog {
    soc: Value,
    rows: Vec<(Value, Value)>,
    ips: Map<String, Value>,
    body: Value,
    firmware: Value,
    configuration: Value,
    pub board: Value,
}
impl Catalog {
    pub fn new(resolved: &ResolvedConfiguration) -> Result<Self, String> {
        let soc = serde_json::to_value(&resolved.reference.soc).map_err(|e| e.to_string())?;
        let profile = resolved
            .configuration
            .preset
            .as_ref()
            .and_then(|preset| preset.hardware_selection.as_ref())
            .and_then(|machine| {
                resolved.reference.body.hardware.as_ref()?.profiles.get(&machine.body_profile_id)
            });
        let mut board = match profile {
            Some(profile) => serde_json::to_value(profile).map_err(|e| e.to_string())?,
            None => json!({}),
        };
        // The run's reset inputs join the body profile, as the Python builder merges them.
        if let Some(inputs) = resolved.hardware_inputs()? {
            let inputs = serde_json::to_value(inputs).map_err(|e| e.to_string())?;
            if let (Some(board), Some(reset)) = (board.as_object_mut(), inputs["reset"].as_object()) {
                board.extend(reset.clone());
            }
        }
        let mut replacements = Map::new();
        for row in board["block_overrides"].as_array().into_iter().flatten() {
            let id = row["block_id"].as_str().ok_or("a block override names no block")?;
            replacements.insert(id.into(), row["device"].clone());
        }
        let blocks = soc["blocks"].as_array().ok_or("the processor record has no blocks")?;
        for id in replacements.keys() {
            if !blocks.iter().any(|row| row["id"] == id.as_str()) {
                return Err(format!("body override names an absent processor block: {id}"));
            }
        }
        let rows: Vec<(Value, Value)> = blocks
            .iter()
            .map(|row| {
                let id = row["id"].as_str().unwrap_or_default();
                let device = replacements.get(id).cloned().unwrap_or_else(|| row["device"].clone());
                (row.clone(), device)
            })
            .collect();
        let reference = fujimod_catalog::reference::ReferenceCatalog::bundled()?;
        let mut ips = Map::new();
        for (row, _) in &rows {
            if let Some(id) = row["ip"].as_str() {
                if let Some(ip) = reference.ip(id) {
                    ips.insert(id.into(), serde_json::to_value(ip).map_err(|e| e.to_string())?);
                }
            }
        }
        let body = serde_json::to_value(&resolved.reference.body).map_err(|e| e.to_string())?;
        let firmware = serde_json::to_value(&resolved.reference.firmware).map_err(|e| e.to_string())?;
        let configuration = serde_json::to_value(&resolved.configuration).map_err(|e| e.to_string())?;
        Ok(Self { soc, rows, ips, body, firmware, configuration, board })
    }
    pub fn blocks(&self) -> impl Iterator<Item = Block<'_>> {
        self.rows.iter().map(|(raw, device)| Block { raw, device })
    }
    /// Blocks whose selected device has `model`, in record order.
    pub fn with_device(&self, model: &str) -> Vec<Block<'_>> {
        self.blocks().filter(|b| b.device["model"] == model).collect()
    }
    /// The first block carrying `model`.
    pub fn device(&self, model: &str) -> Option<Block<'_>> {
        self.with_device(model).into_iter().next()
    }
    pub fn block(&self, id: &str) -> Result<Block<'_>, String> {
        let mut matches = self.blocks().filter(|b| b.id() == id);
        match (matches.next(), matches.next()) {
            (Some(block), None) => Ok(block),
            _ => Err(format!("the processor must declare block {id} exactly once")),
        }
    }
    pub fn soc(&self) -> &Value {
        &self.soc
    }

    /// The definition fields the processor record decides.
    pub fn processor_fields(&self) -> Result<Map<String, Value>, String> {
        let mut out = Map::new();
        let processors = self.soc["processors"].as_array().ok_or("no processors")?;
        out.insert("cores".into(), json!(processors.len()));
        let aprofile = &processors.first().ok_or("no processors")?["aprofile"];
        out.insert("cbar".into(), aprofile["configuration_base"].clone());
        let caches = &aprofile["caches"];
        if caches.is_object() {
            out.insert(
                "cache_sizes".into(),
                json!([caches["l1_data_bytes"], caches["l1_instruction_bytes"], caches["l2_bytes"]]),
            );
        }
        if let Some(timer) = self.device("interval_timer") {
            out.insert("timer_base".into(), json!(timer.base()));
            out.insert("timer_layout".into(), timer.field("layout")?.clone());
            let routes = timer
                .interrupts()
                .iter()
                .map(|row| {
                    let channel = row["output"]
                        .as_str()
                        .and_then(|name| name.strip_prefix("channel"))
                        .and_then(|n| n.parse::<u64>().ok())
                        .ok_or("an interval-timer output is not a channel")?;
                    Ok(json!([channel, row["line"]]))
                })
                .collect::<Result<Vec<_>, String>>()?;
            out.insert("timer_routes".into(), json!(routes));
        }
        if let Some(flash) = self.device("serial_flash") {
            out.insert("storage_irq".into(), json!(flash.interrupt(None)));
            if let Some(dma) = self.device("dma") {
                out.insert("storage_dma_routes".into(), json!(dma.lines()));
            }
        }
        if let Some(block) = self.device("decompressor") {
            out.insert("decompressor".into(), json!([block.base(), block.interrupt(None)]));
        }
        if let Some(block) = self.device("dmp") {
            out.insert("dmp".into(), json!([block.base(), block.interrupt(None)]));
        }
        if let Some(block) = self.device("io_groups") {
            out.insert("io_group_base".into(), json!(block.base()));
        }
        let requests = self
            .blocks()
            .filter(|b| matches!(b.device["model"].as_str(), Some("control_request" | "control_group")))
            .map(|b| Ok(json!([b.base() + b.number("request_offset")?, b.number("ack_ticks")?])))
            .collect::<Result<Vec<_>, String>>()?;
        out.insert("control_requests".into(), json!(requests));
        self.output_fields(&mut out)?;
        out.insert("register_banks".into(), json!(self.register_storage()?));
        if let Some(sram) = self.soc["memory_regions"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|row| row["id"] == "boot-sram")
        {
            out.insert("boot_sram".into(), json!([sram["base"], sram["size"]["bytes"]]));
        }
        if self.soc["id"] == "xp4" {
            self.xp4_fields(&mut out)?;
        }
        Ok(out)
    }

    /// The definition fields the body's inputs decide.
    pub fn body_fields(&self) -> Result<Map<String, Value>, String> {
        let board = &self.board;
        let mut out = Map::new();
        let ram = self.configuration["dram_address_space"]["limit_bytes"]
            .as_u64()
            .or_else(|| self.body["dram_bytes"].as_u64())
            .ok_or("the run declares no DRAM size")?;
        out.insert("ram_size".into(), json!(ram));
        let mut labels = self.firmware["installation"]["entry_labels"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(address, _)| {
                u64::from_str_radix(address.trim_start_matches("0x"), 16)
                    .map_err(|_| format!("entry label address {address} is not hexadecimal"))
            })
            .collect::<Result<Vec<u64>, String>>()?;
        labels.sort_unstable();
        out.insert("labels".into(), json!(labels));
        let declared = &self.soc["instructions_per_tick"];
        let rate = match &board["execution_clock"]["instructions_per_tick"] {
            Value::Array(rate) => rate.clone(),
            _ if declared.is_object() => vec![declared["numerator"].clone(), declared["denominator"].clone()],
            _ => vec![json!(1), json!(1)],
        };
        let (n, d) = (rate.first().and_then(Value::as_u64), rate.get(1).and_then(Value::as_u64));
        if rate.len() != 2 || !matches!((n, d), (Some(n), Some(d)) if d >= 1 && n >= d && n <= 1 << 32) {
            return Err("A7 instruction throughput requires a positive ratio of at least one".into());
        }
        out.insert("instructions_per_tick".into(), json!(rate));
        let chip = &board["serial_flash"];
        if chip.is_object() {
            let g = &chip["geometry"];
            out.insert("storage_geometry".into(),
                json!([g["erase_bytes"], g["page_bytes"], g["erase_ticks"], g["page_ticks"]]));
            out.insert("storage_jedec".into(), chip["jedec_id"].clone());
        }
        if let Some(adc) = self.device("adc").filter(|b| b.id() != "temperature") {
            let input = &board["adc_input"];
            if !input.is_object() {
                return Err("XP4 ADC controller requires its explicit external inputs".into());
            }
            out.insert("adc".into(), json!([input["samples"], adc.number("sample_ticks")?]));
        }
        if let Some(temperature) = self.with_device("adc").into_iter().find(|b| b.id() == "temperature") {
            out.insert("temperature".into(), json!([board["temperature_input"]["millicelsius"],
                temperature.number("sample_ticks")?]));
        }
        if self.device("gpio").is_some() {
            out.insert("gpio".into(), board["gpio_input"]["levels"].clone());
        }
        let routes = board["gpio_interrupts"]["routes"].as_array().cloned().unwrap_or_default();
        out.insert("gpio_interrupts".into(), json!(routes.iter().map(|r| json!({
            "port": r["port"], "pin": r["pin"], "line": r["line"], "active_high": r["active_high"]})).collect::<Vec<_>>()));
        let spi: Vec<(String, Block<'_>)> = self
            .with_device("spi")
            .into_iter()
            .map(|b| (b.id().trim_start_matches("spi").to_string(), b))
            .collect();
        let inputs = board["spi_inputs"].as_object().cloned().unwrap_or_default();
        let mut ports: Vec<&String> = spi.iter().map(|(p, _)| p).collect();
        ports.sort();
        let mut declared: Vec<&String> = inputs.keys().collect();
        declared.sort();
        if ports != declared {
            return Err("XP4 SPI controller and explicit MISO input must be selected together".into());
        }
        let mut plain = Vec::new();
        for (port, block) in &spi {
            let pin = &inputs[port.as_str()];
            let high = pin["level"] == "high";
            if let Some(power) = pin.get("power_registers") {
                let cs = &power["chip_select"];
                out.insert("power_spi".into(), json!([block.base(), high, cs["port"], cs["pin"], cs["active_high"],
                    power["inputs"], power["rtc_ticks_per_second"]]));
            } else if let Some(power) = pin.get("power_checksum_registers") {
                let cs = &power["chip_select"];
                let writes: Vec<Value> = power["disable_writes"].as_array().into_iter().flatten()
                    .map(|r| json!([r["register"], r["value"]])).collect();
                out.insert("checksum_power_spi".into(), json!([block.base(), high, cs["port"], cs["pin"],
                    cs["active_high"], power["inputs"], writes, power.get("rtc").cloned().unwrap_or(Value::Null),
                    power["retained_registers"]]));
            } else {
                plain.push(json!([block.base(), high]));
            }
        }
        out.insert("spi_inputs".into(), json!(plain));
        let mut hosts = Vec::new();
        for card in board["sd_cards"].as_array().into_iter().flatten() {
            let base = card["host_base"].as_u64().ok_or("an SD card names no host")?;
            let host = self
                .with_device("sd_host")
                .into_iter()
                .find(|h| h.base() == base)
                .ok_or_else(|| format!("SD card host {base:#x} is not a declared SD host"))?;
            let c = &card["card"];
            let d = host.device;
            hosts.push(json!({
                "options": {"base": base, "transfer_ticks": d["transfer_ticks"], "clock_ticks": d["clock_ticks"],
                    "rca": c["rca"], "ram_bytes": ram, "high_speed": c["high_speed"],
                    "capabilities": d["capabilities"], "capabilities_2": d["capabilities_2"],
                    "slot_and_version": d["slot_and_version"],
                    "vendor_offsets": d.get("vendor_offsets").cloned().unwrap_or(json!([]))},
                "card": {"sectors": c["sectors"], "max_dirty_sectors": 1u64 << 21,
                    "partition_start": c["format"]["partition_start"],
                    "sectors_per_cluster": c["format"]["sectors_per_cluster"], "label": c["format"]["label"]},
                "irq": host.interrupt(None)}));
        }
        out.insert("sd_hosts".into(), json!(hosts));
        if let Some(command) = self.device("sensor_command") {
            let receiver = self.device("imaging_receiver").ok_or("the sensor command block needs a receiver")?;
            let sensor = &board["sensor_input"];
            let lines = |block: Block<'_>, prefix: &str| -> Vec<Value> {
                block.interrupts().iter()
                    .filter(|row| row["output"].as_str().is_some_and(|o| o.starts_with(prefix)))
                    .map(|row| row["line"].clone()).collect()
            };
            let buffer = match self.device("imaging_input_buffer") {
                None => Value::Null,
                Some(buffer) => {
                    let region = buffer.field("sram_region_id")?;
                    let sram = self.soc["memory_regions"].as_array().into_iter().flatten()
                        .find(|row| &row["id"] == region)
                        .ok_or("the input buffer names an absent memory region")?;
                    json!([buffer.base(), buffer.interrupt(None), buffer.number("reset_ticks")?, sram["base"],
                        buffer.number("bank_bytes")?, sensor["statistics_scene"]])
                }
            };
            out.insert("imaging".into(), json!([
                [command.base(), receiver.base(), lines(receiver, "normal"), lines(receiver, "error"),
                 lines(command, "timing"), lines(receiver, "raw-dma")],
                [sensor["frame_period_ticks"], sensor["frame_active_ticks"],
                 command.number("command_ticks")?, command.number("reset_ticks")?],
                [sensor["read_gate"]["register"], sensor["read_gate"]["mask"]],
                buffer]));
        }
        let usb_input = &board["usb_input"];
        if let Some(usb) = self.device("usb").filter(|_| usb_input.is_object()) {
            let speed = match usb_input["speed"].as_str() {
                Some("high") => 0,
                Some("super") => 4,
                other => return Err(format!("unknown USB speed {other:?}")),
            };
            out.insert("usb".into(), json!([usb.base(), usb.interrupt(None), usb.number("command_ticks")?,
                usb.number("transaction_ticks")?, speed]));
        }
        if let Some(dials) = board["countdown_dials"]["dials"].as_array() {
            out.insert("countdown_dials".into(), json!(dials.iter().map(|d| json!({"name": d["control"],
                "channel": d["channel"], "phase_ticks": d["phase_ticks"], "rest_phase": d["rest_phase"]}))
                .collect::<Vec<_>>()));
        }
        Ok(out)
    }

    /// Whether the board builds a device for `block`. A block with no device,
    /// a device this board does not model, or an SD host with no declared SD
    /// card leaves its window to register storage.
    pub fn builds(&self, block: Block<'_>) -> bool {
        match block.device["model"].as_str() {
            None | Some("register_storage" | "byte_i2c") => false,
            Some("sd_host") => self.sd_card_hosts().contains(&block.base()),
            Some(_) => true,
        }
    }

    /// The SD host windows the body declares an SD card in.
    fn sd_card_hosts(&self) -> Vec<u64> {
        self.board["sd_cards"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|card| card["host_base"].as_u64())
            .collect()
    }

    /// Register-storage windows less every address another selected block owns,
    /// as `[low, high)` ranges in record order.
    pub fn register_storage(&self) -> Result<Vec<(u64, u64)>, String> {
        let mut owned: Vec<(u64, u64)> = Vec::new();
        for block in self.blocks().filter(|&b| self.builds(b)) {
            owned.push((block.base(), block.base() + block.size()));
            // Windows the device names as its own (`*_block_id`, `*_block_ids`).
            for (key, value) in block.device.as_object().into_iter().flatten() {
                let ids: Vec<&Value> = match value {
                    Value::String(_) if key.ends_with("_block_id") => vec![value],
                    Value::Array(rows) if key.ends_with("_block_ids") => rows.iter().collect(),
                    _ => continue,
                };
                for id in ids {
                    let named = self.block(id.as_str().unwrap_or_default())?;
                    owned.push((named.base(), named.base() + named.size()));
                }
            }
        }
        // Each SD clock model owns its halfword.
        for clock in self.sd_clock_words()? {
            owned.push((clock.0, clock.0 + 2));
        }
        let mut out = Vec::new();
        for block in self.with_device("register_storage") {
            let mut segments = vec![(block.base(), block.base() + block.size())];
            for &(lo, hi) in &owned {
                segments = segments
                    .into_iter()
                    .flat_map(|(a, b)| {
                        if hi <= a || b <= lo {
                            vec![(a, b)]
                        } else {
                            [(a, lo.max(a)), (hi.min(b), b)].into_iter().filter(|(x, y)| x < y).collect()
                        }
                    })
                    .collect();
            }
            out.extend(segments);
        }
        Ok(out)
    }

    /// The offset of `register` in the design `block` instantiates.
    fn register_offset(&self, block: Block<'_>, register: &str) -> Result<u64, String> {
        let ip = block.raw["ip"].as_str().ok_or_else(|| format!("block {} names no design", block.id()))?;
        self.ips
            .get(ip)
            .and_then(|design| design["registers"].as_array())
            .and_then(|rows| rows.iter().find(|row| row["name"] == register))
            .and_then(|row| row["offset"].as_u64())
            .ok_or_else(|| format!("design {ip} declares no {register} register"))
    }

    /// Displays, video outputs, USB, input pins and SD card clocks.
    fn output_fields(&self, out: &mut Map<String, Value>) -> Result<(), String> {
        let lcdc = self
            .with_device("display")
            .into_iter()
            .filter(|b| b.device.get("pixel_clock_ticks").is_some())
            .map(|b| {
                let mut row = vec![json!(b.base()), b.device["pixel_clock_ticks"].clone()];
                row.extend(b.lines().into_iter().map(Value::from));
                row.push(b.device.get("picture_layout").cloned().unwrap_or(Value::Null));
                row.push(b.device.get("video_layout").cloned().unwrap_or(Value::Null));
                Ok(Value::Array(row))
            })
            .collect::<Result<Vec<_>, String>>()?;
        out.insert("lcdc".into(), json!(lcdc));
        let movdc = self.with_device("movdc");
        let frames = &self.board["processed_frame_input"];
        if !movdc.is_empty() && !frames.is_object() {
            return Err("video outputs require the body's processed frame input".into());
        }
        let movdc = movdc
            .into_iter()
            .map(|b| Ok(json!([b.base(), b.interrupt(None), b.number("reset_ticks")?,
                frames["frame_period_ticks"], frames["luma"]])))
            .collect::<Result<Vec<_>, String>>()?;
        out.insert("movdc".into(), json!(movdc));
        if let Some(phy) = self.device("usb_phy") {
            let input = &self.board["usb_phy_input"];
            out.insert("usb_phy".into(), json!([phy.base(), input["registers"], input["parallel"]]));
        }
        let cable = &self.board["usb_cable"];
        if let Some(usb) = self.device("xpp_usb").filter(|_| cable.is_object()) {
            let speed = match cable["speed"].as_str() {
                Some("high") => 0,
                Some("super") => 4,
                other => return Err(format!("unknown USB cable speed {other:?}")),
            };
            out.insert("xpp_usb".into(), json!([usb.base(), usb.interrupt(None),
                usb.number("command_ticks")?, usb.number("transaction_ticks")?, speed]));
        }
        if let Some(gpio) = self.device("gpio") {
            out.insert("gpio_base".into(), json!(gpio.base()));
        }
        let owned = self.sd_card_hosts();
        let clocks: Vec<Value> = self
            .sd_clock_words()?
            .into_iter()
            .filter(|(address, _)| !owned.contains(&(address & !0xfff)))
            .map(|(a, t)| json!([a, t]))
            .collect();
        out.insert("sd_clocks".into(), json!(clocks));
        Ok(())
    }

    /// Every SD interface clock halfword with its settle time: each host's
    /// clock-control word and the free-standing clock blocks, sorted.
    fn sd_clock_words(&self) -> Result<Vec<(u64, u64)>, String> {
        let mut clocks = self
            .with_device("sd_host_clock")
            .into_iter()
            .map(|b| Ok((b.base(), b.number("settle_ticks")?)))
            .collect::<Result<Vec<_>, String>>()?;
        let hosts = self.with_device("sd_host");
        if let Some(&first) = hosts.first() {
            let mut settles = hosts.iter().map(|h| h.number("clock_ticks")).collect::<Result<Vec<_>, _>>()?;
            settles.dedup();
            if settles.len() != 1 {
                return Err("XP4 SD hosts declare different clock settle times".into());
            }
            let offset = self.register_offset(first, "clock_control")?;
            clocks.extend(hosts.iter().map(|h| (h.base() + offset, settles[0])));
        }
        clocks.sort_unstable();
        Ok(clocks)
    }

    /// The XP4 imaging and still devices, each from the block that owns it.
    fn xp4_fields(&self, out: &mut Map<String, Value>) -> Result<(), String> {
        let required = |model: &str| {
            self.device(model)
                .ok_or_else(|| format!("XP4 requires a declared {model} block"))
        };
        let jpeg = self
            .with_device("jpeg_codec")
            .into_iter()
            .find(|b| !b.id().contains("decoder"))
            .ok_or("XP4 requires a declared JPEG encoder block")?;
        let memory_type = self.block(jpeg.field("memory_type_block_id")?.as_str().unwrap_or_default())?;
        out.insert("jpeg_encoder".into(), json!({"base": jpeg.base(), "irq": jpeg.interrupt(None),
            "run_ticks": jpeg.number("run_ticks")?, "memory_type": memory_type.base()}));
        let engines = self
            .with_device("still_engine")
            .into_iter()
            .map(|b| Ok(json!({"engine": b.field("engine")?, "base": b.base(), "irq": b.interrupt(None),
                "run_ticks": b.number("run_ticks")?})))
            .collect::<Result<Vec<_>, String>>()?;
        out.insert("still_engines".into(), json!(engines));
        let dsp = required("dsp")?;
        out.insert("dsp".into(), json!({"irq": dsp.interrupt(None), "run_ticks": dsp.number("run_ticks")?}));
        let face = required("face_detection")?;
        out.insert("face_detector".into(), json!({"base": face.base(), "irq": face.interrupt(None),
            "run_ticks": face.number("run_ticks")?}));
        let filter = required("filter_engine")?;
        let types = self.block(filter.field("memory_types_block_id")?.as_str().unwrap_or_default())?;
        out.insert("filter_engine".into(), json!({"base": filter.base(), "memory_types": types.base(),
            "irq": filter.interrupt(None), "run_ticks": filter.number("run_ticks")?}));
        let countdown = required("countdown_timer")?;
        let routes = countdown
            .interrupts()
            .iter()
            .map(|row| {
                let channel = row["output"].as_str().and_then(|n| n.strip_prefix("channel"))
                    .and_then(|n| n.parse::<u64>().ok()).ok_or("a countdown output is not a channel")?;
                Ok((channel, row["line"].clone()))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let mut channels: Vec<u64> = routes.iter().map(|r| r.0).collect();
        channels.sort_unstable();
        if channels != (0..countdown.number("channels")?).collect::<Vec<_>>() {
            return Err("the countdown timer block must route every channel once".into());
        }
        out.insert("countdown_timers".into(), json!([countdown.base(),
            routes.iter().map(|(c, l)| json!([c, l])).collect::<Vec<_>>()]));
        let usb = required("usb_condition")?;
        out.insert("usb_condition".into(), json!([usb.base(), false]));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xt3() -> Catalog {
        Catalog::new(&ResolvedConfiguration::bundled("xt3-511").unwrap()).unwrap()
    }

    /// Writes each configuration's fields to `FUJIMOD_XP4_CATALOG_DIR/<id>.json`
    /// when set, for comparison with another builder.
    #[test]
    fn every_xp4_configuration_builds() {
        for id in ["xt3-511", "xt30-201-boot", "xe4-201-handoff"] {
            let catalog = Catalog::new(&ResolvedConfiguration::bundled(id).unwrap()).unwrap();
            let mut fields = catalog.processor_fields().unwrap();
            fields.extend(catalog.body_fields().unwrap_or_else(|e| panic!("{id}: {e}")));
            if let Ok(dir) = std::env::var("FUJIMOD_XP4_CATALOG_DIR") {
                std::fs::write(format!("{dir}/{id}.json"), serde_json::to_string_pretty(&fields).unwrap()).unwrap();
            }
        }
    }

    #[test]
    fn xt3_processor_fields_come_from_its_blocks() {
        let mut fields = xt3().processor_fields().unwrap();
        fields.extend(xt3().body_fields().unwrap());
        if let Ok(path) = std::env::var("FUJIMOD_XP4_CATALOG_DUMP") {
            std::fs::write(path, serde_json::to_string_pretty(&fields).unwrap()).unwrap();
            if let Ok(board) = std::env::var("FUJIMOD_XP4_BOARD_DUMP") {
                std::fs::write(board, serde_json::to_string_pretty(&xt3().board).unwrap()).unwrap();
            }
        }
        assert_eq!(fields["cores"], 4);
        assert_eq!(fields["cbar"], 0xffff0000u64);
        assert_eq!(fields["timer_base"], 0xfff3c300u64);
        assert_eq!(fields["timer_routes"][0], json!([0, 57]));
        assert_eq!(fields["storage_irq"], 37);
        assert_eq!(fields["decompressor"], json!([0xfff79000u64, 128]));
    }
}
