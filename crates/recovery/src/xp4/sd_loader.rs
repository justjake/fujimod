//! Compiled XP4 SD loader and its symbol and relocation layout.
use serde::Deserialize;
use std::{collections::BTreeMap, sync::LazyLock};

pub(super) const IMAGE: &[u8] = include_bytes!("../../assets/xp4-sd-loader.generated.bin");
pub(super) static LAYOUT: LazyLock<Layout> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../assets/xp4-sd-loader.generated.json"))
        .expect("checked XP4 SD loader layout")
});

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Layout {
    pub ffinit: Ffinit,
    pub card: Card,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Ffinit {
    pub symbols: BTreeMap<String, usize>,
    pub text: Segment,
    pub suffix: Segment,
    pub fixups: Vec<(usize, u32, i64)>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Card {
    pub address: usize,
    pub offset: usize,
    pub size: usize,
    pub symbols: BTreeMap<String, usize>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Segment {
    pub offset: usize,
    pub size: usize,
}
impl Segment {
    pub fn bytes(&self) -> &'static [u8] {
        &IMAGE[self.offset..self.offset + self.size]
    }
}
