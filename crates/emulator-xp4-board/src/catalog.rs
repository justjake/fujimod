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
        Ok(Self { soc, rows, ips, board })
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
        if self.soc["id"] == "xp4" {
            self.xp4_fields(&mut out)?;
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
        // A host with a declared SD card owns its page, clock halfword included.
        let owned: Vec<u64> = self.board["sd_cards"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|card| card["host_base"].as_u64())
            .collect();
        let clocks: Vec<Value> = clocks
            .into_iter()
            .filter(|(address, _)| !owned.contains(&(address & !0xfff)))
            .map(|(a, t)| json!([a, t]))
            .collect();
        out.insert("sd_clocks".into(), json!(clocks));
        Ok(())
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

    #[test]
    fn xt3_processor_fields_come_from_its_blocks() {
        let fields = xt3().processor_fields().unwrap();
        if let Ok(path) = std::env::var("FUJIMOD_XP4_CATALOG_DUMP") {
            std::fs::write(path, serde_json::to_string_pretty(&fields).unwrap()).unwrap();
        }
        assert_eq!(fields["cores"], 4);
        assert_eq!(fields["cbar"], 0xffff0000u64);
        assert_eq!(fields["timer_base"], 0xfff3c300u64);
        assert_eq!(fields["timer_routes"][0], json!([0, 57]));
        assert_eq!(fields["storage_irq"], 37);
        assert_eq!(fields["decompressor"], json!([0xfff79000u64, 128]));
    }
}
