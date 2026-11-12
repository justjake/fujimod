//! A custom-setting slot (C1–C7) set up for shooting a test chart on an
//! X100VI 1.31 in USB RAW CONV./BACKUP RESTORE mode: save the slot, write the
//! chart's image settings into it, and restore it afterward. ISO, shutter,
//! aperture, focus mode and the self-timer are not part of a slot.
use crate::ptp::{self, DeviceInfo, Pipe, Reader, Session};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const MODEL: &str = "X100VI";
pub const FIRMWARE: &str = "1.31";
pub const UNVERIFIED: &str = "The slot property codes are unverified on the X100VI: they follow \
     other X-Trans V bodies, and every write is read back and stops on a mismatch.";

const USB_MODE: u16 = 0xd16e;
const RAW_CONVERSION_MODE: u64 = 6;
const SLOT: u16 = 0xd18c;
const NAME: u16 = 0xd18d;
const DYNAMIC_RANGE: u16 = 0xd190;
const D_RANGE_PRIORITY: u16 = 0xd191;
const FILM: u16 = 0xd192;
const MONOCHROMATIC_WC: u16 = 0xd193;
const MONOCHROMATIC_MG: u16 = 0xd194;
const WHITE_BALANCE: u16 = 0xd199;
const COLOR_TEMPERATURE: u16 = 0xd19c;
const COLOR: u16 = 0xd19f;
const KELVIN: u64 = 0x8007;

/// Every numeric setting a slot holds.
pub const PROPERTIES: [(u16, &str); 23] = [
    (0xd18e, "image size"),
    (0xd18f, "image quality"),
    (DYNAMIC_RANGE, "dynamic range"),
    (D_RANGE_PRIORITY, "D range priority"),
    (FILM, "film simulation"),
    (MONOCHROMATIC_WC, "monochromatic color WC"),
    (MONOCHROMATIC_MG, "monochromatic color MG"),
    (0xd195, "grain effect"),
    (0xd196, "color chrome effect"),
    (0xd197, "color chrome FX blue"),
    (0xd198, "smooth skin effect"),
    (WHITE_BALANCE, "white balance"),
    (0xd19a, "white balance shift red"),
    (0xd19b, "white balance shift blue"),
    (COLOR_TEMPERATURE, "color temperature"),
    (0xd19d, "highlight tone"),
    (0xd19e, "shadow tone"),
    (COLOR, "color"),
    (0xd1a0, "sharpness"),
    (0xd1a1, "noise reduction"),
    (0xd1a2, "clarity"),
    (0xd1a3, "lens modulation optimizer"),
    (0xd1a4, "color space"),
];

/// The chart's settings in write order: the film and D range priority come
/// before the settings they gate, the white balance mode before its
/// temperature. MONOCHROMATIC COLOR does not apply to PROVIA and is left alone.
pub const CHART: [(u16, u32, &str); 21] = [
    (FILM, 0x01, "PROVIA"),
    (D_RANGE_PRIORITY, 0x00, "OFF"),
    (WHITE_BALANCE, 0x8007, "color temperature"),
    (COLOR_TEMPERATURE, 6700, "6700 K"),
    (0xd18e, 0x07, "L 3:2"),
    (0xd18f, 0x04, "FINE+RAW"),
    (0xd195, 0x01, "OFF"),
    (0xd196, 0x01, "OFF"),
    (0xd197, 0x01, "OFF"),
    (0xd198, 0x01, "OFF"),
    (0xd19a, 0, "0"),
    (0xd19b, 0, "0"),
    (0xd19d, 0, "0"),
    (0xd19e, 0, "0"),
    (COLOR, 0, "0"),
    (0xd1a0, 0, "0"),
    (0xd1a1, 0x2000, "0"),
    (0xd1a2, 0, "0"),
    (0xd1a3, 0x02, "OFF"),
    (0xd1a4, 0x02, "sRGB"),
    (DYNAMIC_RANGE, 100, "DR100"),
];
pub const CHART_NAME: &str = "CHART";

/// A slot as `save` read it: each property's bytes, as the camera sent them.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct Saved {
    pub model: String,
    pub firmware: String,
    pub slot: u8,
    pub name: String,
    pub values: BTreeMap<String, String>,
}

impl Saved {
    fn bytes(&self, code: u16) -> Result<Vec<u8>, String> {
        let hex = self
            .values
            .get(&format!("{code:#06x}"))
            .ok_or(format!("the saved slot has no {code:#06x}"))?;
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|e| e.to_string()))
            .collect()
    }
}

fn name(code: u16) -> &'static str {
    PROPERTIES
        .iter()
        .find(|(c, _)| *c == code)
        .map_or("slot", |(_, n)| n)
}

fn unsigned(bytes: &[u8]) -> u64 {
    bytes.iter().rev().fold(0, |v, b| (v << 8) | u64::from(*b))
}

fn monochrome(film: u64) -> bool {
    matches!(film, 0x06..=0x09 | 0x0c..=0x0f)
}

async fn get<P: Pipe>(s: &mut Session<P>, code: u16) -> Result<Vec<u8>, String> {
    let reply = s
        .call(ptp::GET_DEVICE_PROP_VALUE, &[code.into()], None)
        .await
        .map_err(|e| format!("reading {} ({code:#06x}): {e}", name(code)))?;
    Ok(reply.data)
}

/// Sends `bytes` and reads them back.
async fn put<P: Pipe>(s: &mut Session<P>, code: u16, bytes: &[u8]) -> Result<(), String> {
    s.call(ptp::SET_DEVICE_PROP_VALUE, &[code.into()], Some(bytes))
        .await
        .map_err(|e| format!("writing {} ({code:#06x}): {e}", name(code)))?;
    let back = get(s, code).await?;
    if back != bytes {
        return Err(format!(
            "{} ({code:#06x}) reads back {back:02x?} after writing {bytes:02x?}",
            name(code)
        ));
    }
    Ok(())
}

/// Writes `value` at the width the camera uses for the property.
async fn set<P: Pipe>(s: &mut Session<P>, code: u16, value: u32) -> Result<(), String> {
    let width = get(s, code).await?.len();
    if !matches!(width, 1 | 2 | 4) || (width < 4 && value >> (8 * width) != 0) {
        return Err(format!(
            "{} ({code:#06x}) is {width} bytes; {value:#x} does not fit",
            name(code)
        ));
    }
    put(s, code, &value.to_le_bytes()[..width]).await
}

/// Refuses anything but an X100VI 1.31 in USB RAW CONV./BACKUP RESTORE mode.
pub async fn check<P: Pipe>(s: &mut Session<P>) -> Result<(String, String), String> {
    let reply = s
        .call(ptp::GET_DEVICE_INFO, &[], None)
        .await
        .map_err(|e| e.to_string())?;
    let info = DeviceInfo::parse(&reply.data).map_err(|e| e.to_string())?;
    if info.manufacturer != "FUJIFILM" || info.model != MODEL || info.device_version != FIRMWARE {
        return Err(format!(
            "this is a {} {} on firmware {}; chart-slot runs only on a FUJIFILM {MODEL} {FIRMWARE}",
            info.manufacturer, info.model, info.device_version
        ));
    }
    let mode = get(s, USB_MODE).await.map(|b| unsigned(&b));
    if mode != Ok(RAW_CONVERSION_MODE) {
        return Err(
            "set CONNECTION SETTING > CONNECTION MODE to USB RAW CONV./BACKUP RESTORE".into(),
        );
    }
    Ok((info.model, info.device_version))
}

async fn select<P: Pipe>(s: &mut Session<P>, slot: u8) -> Result<(), String> {
    if !(1..=7).contains(&slot) {
        return Err(format!("slot C{slot}: the X100VI has C1 to C7"));
    }
    set(s, SLOT, slot.into()).await
}

async fn slot_name<P: Pipe>(s: &mut Session<P>) -> Result<String, String> {
    Reader::new(&get(s, NAME).await?)
        .string()
        .map_err(|e| e.to_string())
}

async fn put_name<P: Pipe>(s: &mut Session<P>, text: &str) -> Result<(), String> {
    let mut bytes = Vec::new();
    ptp::write_string(&mut bytes, text);
    put(s, NAME, &bytes).await
}

/// Reads slot C`slot`.
pub async fn save<P: Pipe>(s: &mut Session<P>, slot: u8) -> Result<Saved, String> {
    let (model, firmware) = check(s).await?;
    select(s, slot).await?;
    let mut values = BTreeMap::new();
    for (code, _) in PROPERTIES {
        let hex = get(s, code)
            .await?
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        values.insert(format!("{code:#06x}"), hex);
    }
    let name = slot_name(s).await?;
    Ok(Saved {
        model,
        firmware,
        slot,
        name,
        values,
    })
}

/// Writes the chart's settings into the slot `saved` was read from.
pub async fn write<P: Pipe>(
    s: &mut Session<P>,
    saved: &Saved,
    mut log: impl FnMut(&str),
) -> Result<(), String> {
    check(s).await?;
    select(s, saved.slot).await?;
    for (code, value, label) in CHART {
        set(s, code, value).await?;
        log(&format!("C{} {}: {label}", saved.slot, name(code)));
    }
    put_name(s, CHART_NAME).await?;
    log(&format!("C{} name: {CHART_NAME}", saved.slot));
    Ok(())
}

/// Puts back what `save` read. The camera refuses a setting the current film,
/// white balance or D range priority makes inapplicable, so COLOR and the
/// color temperature go back while the slot still allows them, before the
/// film and white balance; D range priority goes back last.
pub async fn restore<P: Pipe>(
    s: &mut Session<P>,
    saved: &Saved,
    mut log: impl FnMut(&str),
) -> Result<(), String> {
    check(s).await?;
    select(s, saved.slot).await?;
    let priority = saved.bytes(D_RANGE_PRIORITY)?;
    put(s, D_RANGE_PRIORITY, &vec![0; priority.len()]).await?;
    let mut order = Vec::new();
    if !monochrome(unsigned(&get(s, FILM).await?)) {
        order.push(COLOR);
    }
    if unsigned(&get(s, WHITE_BALANCE).await?) == KELVIN {
        order.push(COLOR_TEMPERATURE);
    }
    order.extend([FILM, WHITE_BALANCE]);
    let mono = monochrome(unsigned(&saved.bytes(FILM)?));
    for (code, _) in PROPERTIES {
        let gated = match code {
            MONOCHROMATIC_WC | MONOCHROMATIC_MG => !mono,
            COLOR => mono,
            COLOR_TEMPERATURE => unsigned(&saved.bytes(WHITE_BALANCE)?) != KELVIN,
            _ => false,
        };
        if !order.contains(&code) && code != D_RANGE_PRIORITY && !gated {
            order.push(code);
        }
    }
    for code in order {
        put(s, code, &saved.bytes(code)?).await?;
        log(&format!("C{} {}: restored", saved.slot, name(code)));
    }
    put(s, D_RANGE_PRIORITY, &priority).await?;
    put_name(s, &saved.name).await?;
    log(&format!("C{} restored as {:?}", saved.slot, saved.name));
    Ok(())
}

/// What `write` would send, without a camera.
pub fn plan(slot: u8) -> Vec<String> {
    let mut lines: Vec<String> = CHART
        .iter()
        .map(|(code, value, label)| {
            format!("C{slot} {code:#06x} {} = {value:#x} ({label})", name(*code))
        })
        .collect();
    lines.push(format!("C{slot} {NAME:#06x} name = {CHART_NAME:?}"));
    lines.push(UNVERIFIED.into());
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_camera::FakeCamera;
    use futures_lite::future::block_on;

    fn x100vi() -> FakeCamera {
        let mut c = FakeCamera::new(512);
        c.model = MODEL.into();
        c.firmware = FIRMWARE.into();
        c
    }

    fn session(camera: FakeCamera) -> Session<FakeCamera> {
        block_on(Session::open(camera)).unwrap()
    }

    #[test]
    fn write_sets_every_chart_setting_and_restore_puts_the_slot_back() {
        let mut camera = x100vi();
        // The owner's C4: ACROS with a tint, D range priority WEAK, Kelvin 5000.
        camera.slots[4].insert(FILM, vec![0x0c, 0]);
        camera.slots[4].insert(MONOCHROMATIC_WC, (-30i16).to_le_bytes().to_vec());
        camera.slots[4].insert(D_RANGE_PRIORITY, vec![1, 0]);
        camera.slots[4].insert(WHITE_BALANCE, vec![0x07, 0x80]);
        camera.slots[4].insert(COLOR_TEMPERATURE, 5000u32.to_le_bytes().to_vec());
        let before = camera.slots[4].clone();
        let mut s = session(camera);
        let saved = block_on(save(&mut s, 4)).unwrap();
        assert_eq!(saved.name, "MY ACROS");
        let mut log = Vec::new();
        block_on(write(&mut s, &saved, |l| log.push(l.to_owned()))).unwrap();
        assert_eq!(log.len(), CHART.len() + 1);
        let camera = block_on(s.close()).unwrap();
        for (code, value, _) in CHART {
            assert_eq!(
                unsigned(&camera.slots[4][&code]),
                u64::from(value),
                "{code:#06x}"
            );
        }
        assert_eq!(camera.slots[4][&0xd18f], vec![0x04, 0]);
        assert_eq!(camera.slots[4][&COLOR_TEMPERATURE].len(), 4);
        let mut s = session(camera);
        block_on(restore(&mut s, &saved, |_| ())).unwrap();
        let camera = block_on(s.close()).unwrap();
        assert_eq!(camera.slots[4], before);
        assert!(camera.slots[3] == FakeCamera::new(512).slots[3]);
    }

    #[test]
    fn saved_slots_round_trip_through_json() {
        let mut s = session(x100vi());
        let saved = block_on(save(&mut s, 7)).unwrap();
        let text = serde_json::to_string(&saved).unwrap();
        assert_eq!(serde_json::from_str::<Saved>(&text).unwrap(), saved);
        assert_eq!(saved.values.len(), PROPERTIES.len());
    }

    #[test]
    fn refuses_another_body_firmware_or_usb_mode() {
        let mut camera = x100vi();
        camera.firmware = "1.30".into();
        let e = block_on(save(&mut session(camera), 7)).unwrap_err();
        assert!(e.contains("firmware 1.30"), "{e}");
        let e = block_on(save(&mut session(FakeCamera::new(512)), 7)).unwrap_err();
        assert!(e.contains("X-T5"), "{e}");
        let mut camera = x100vi();
        camera.usb_mode = 5;
        let e = block_on(save(&mut session(camera), 7)).unwrap_err();
        assert!(e.contains("USB RAW CONV"), "{e}");
        let e = block_on(save(&mut session(x100vi()), 8)).unwrap_err();
        assert!(e.contains("C1 to C7"), "{e}");
    }

    #[test]
    fn stops_when_the_camera_refuses_or_ignores_a_setting() {
        let mut camera = x100vi();
        camera.refuse_property = Some(0xd18f);
        let mut s = session(camera);
        let saved = block_on(save(&mut s, 7)).unwrap();
        let e = block_on(write(&mut s, &saved, |_| ())).unwrap_err();
        assert!(e.contains("image quality (0xd18f)"), "{e}");
        let mut camera = x100vi();
        camera.ignore_property = Some(COLOR_TEMPERATURE);
        let mut s = session(camera);
        let saved = block_on(save(&mut s, 7)).unwrap();
        let e = block_on(write(&mut s, &saved, |_| ())).unwrap_err();
        assert!(e.contains("color temperature (0xd19c) reads back"), "{e}");
    }

    #[test]
    fn a_value_wider_than_the_property_is_refused() {
        let mut camera = x100vi();
        camera.slots[7].insert(COLOR_TEMPERATURE, vec![0]);
        let mut s = session(camera);
        let saved = block_on(save(&mut s, 7)).unwrap();
        let e = block_on(write(&mut s, &saved, |_| ())).unwrap_err();
        assert!(e.contains("is 1 bytes"), "{e}");
    }

    #[test]
    fn the_plan_names_every_write_and_says_it_is_unverified() {
        let plan = plan(7);
        assert_eq!(plan.len(), CHART.len() + 2);
        assert!(plan[0].starts_with("C7 0xd192 film simulation = 0x1 (PROVIA)"));
        assert!(plan.last().unwrap().contains("unverified on the X100VI"));
    }
}
