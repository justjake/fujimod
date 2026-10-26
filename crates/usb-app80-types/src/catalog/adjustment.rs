//! Direct adjustment requests. Reported bytes do not instruct host polling.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdjustmentOptionByte(u8);
impl AdjustmentOptionByte {
    pub const fn new(value: u8) -> Self {
        Self(value)
    }
    pub fn value(self) -> u8 {
        self.0
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdjustmentOptionStatus {
    ReportedSuccess,
    ReportedFailure,
    Unknown(u8),
}
impl AdjustmentOptionStatus {
    pub(super) fn decode(raw: u8) -> Self {
        match raw {
            0 => Self::ReportedSuccess,
            1 => Self::ReportedFailure,
            n => Self::Unknown(n),
        }
    }
}
