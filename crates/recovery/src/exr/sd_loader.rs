//! Compiled EXR SD loader and firmware-reader layout.
use serde::Deserialize;
use std::{collections::BTreeMap, sync::LazyLock};

const IMAGE: &[u8] = include_bytes!("../../assets/exr-sd-loader.generated.bin");
pub(crate) static LAYOUT: LazyLock<Layout> = LazyLock::new(|| {
    let layout: Layout =
        serde_json::from_str(include_str!("../../assets/exr-sd-loader.generated.json"))
            .expect("checked EXR SD loader layout");
    assert_eq!(layout.schema, 1, "EXR SD loader layout schema");
    assert_eq!(layout.image_bytes, IMAGE.len(), "EXR SD loader image size");
    assert_eq!(layout.stage.offset, 0);
    assert_eq!(layout.stage.size, layout.firmware.offset);
    assert_eq!(layout.firmware.offset + layout.firmware.size, IMAGE.len());
    assert!(layout.stage.config + 20 <= layout.stage.size);
    assert!(layout.stage.sram >= 0xfff00448);
    assert!(layout.stage.sram + layout.stage.sram_size <= layout.stage.stack_bottom);
    assert_eq!(layout.stage.stack_bottom, 0xfff01000);
    assert_eq!(layout.stage.stack_top, 0xfff01ff8);
    layout
});

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Layout {
    schema: usize,
    image_bytes: usize,
    pub stage: Stage,
    pub firmware: Firmware,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Stage {
    offset: usize,
    size: usize,
    pub config: usize,
    sram: usize,
    sram_size: usize,
    stack_bottom: usize,
    stack_top: usize,
}
impl Stage {
    pub fn bytes(&self) -> &'static [u8] {
        &IMAGE[self.offset..self.offset + self.size]
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Firmware {
    pub address: usize,
    offset: usize,
    size: usize,
    pub symbols: BTreeMap<String, usize>,
}
impl Firmware {
    pub fn bytes(&self) -> &'static [u8] {
        &IMAGE[self.offset..self.offset + self.size]
    }
}
