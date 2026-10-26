//! Plan options of the EXR II face results model in `fujimod-emulator-exr2-isp`.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    Empty,
    Box64x20,
    Box172x120,
}

impl Profile {
    pub fn output(self, offset: u16) -> Option<u32> {
        Some(match (self, offset) {
            (Self::Empty, 0x600 | 0x610 | 0x614 | 0x618) => 0,
            (_, 0x600) => 1,
            (_, 0x610) => (255 << 16) | 3,
            (Self::Box64x20, 0x614) => 64 * 128,
            (Self::Box64x20, 0x618) => 20 * 128,
            (Self::Box172x120, 0x614) => 172 * 128,
            (Self::Box172x120, 0x618) => 120 * 128,
            _ => return None,
        })
    }
}
