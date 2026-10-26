//! Execution-owner permission for bounded, instruction-proven loop acceleration.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LoopMode {
    Disabled,
    Measure,
    #[default]
    Accelerate,
}
impl LoopMode {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "disabled" => Ok(Self::Disabled),
            "measure" => Ok(Self::Measure),
            "accelerate" => Ok(Self::Accelerate),
            _ => Err(format!("unknown loop execution mode: {value}")),
        }
    }
}
