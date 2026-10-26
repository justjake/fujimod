//! Browser TCG tier selection and statistics. Rust owns defaults and bounds.
use fujimod_emulator_cpu_unicorn::jit::{JitOptions, Statistics};
use serde::{Deserialize, Serialize};
use tsify::Tsify;

/// `false` disables the tier, `true` selects Rust defaults.
#[derive(Tsify)]
#[serde(untagged)]
#[tsify(hashmap_as_object, missing_as_null)]
pub enum JitSelection {
    Enabled(bool),
    Options(JitOptions),
}
impl<'de> Deserialize<'de> for JitSelection {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        match serde_json::Value::deserialize(deserializer)? {
            serde_json::Value::Bool(enabled) => Ok(Self::Enabled(enabled)),
            options @ serde_json::Value::Object(_) => serde_json::from_value(options)
                .map(Self::Options)
                .map_err(|e| D::Error::custom(format!("tcgJit: {e}"))),
            _ => Err(D::Error::custom(
                "tcgJit must be false, true or an options object",
            )),
        }
    }
}
pub(super) fn selection(value: Option<JitSelection>) -> Result<Option<JitOptions>, String> {
    match value {
        None | Some(JitSelection::Enabled(true)) => Ok(Some(JitOptions::default())),
        Some(JitSelection::Enabled(false)) => Ok(None),
        Some(JitSelection::Options(options)) => Ok(Some(options.validate()?)),
    }
}
pub(super) fn setup(options: Option<JitOptions>) -> Result<(), String> {
    #[cfg(target_os = "emscripten")]
    {
        fujimod_emulator_cpu_unicorn::jit::emscripten::setup(options)
    }
    #[cfg(not(target_os = "emscripten"))]
    {
        match options {
            None => Ok(()),
            Some(_) => Err("the generated-code tier requires Emscripten".into()),
        }
    }
}
#[derive(Serialize, Tsify)]
#[tsify(hashmap_as_object, missing_as_null)]
pub struct DisabledStatistics {
    #[tsify(type = "false")]
    enabled: bool,
}
#[derive(Serialize, Tsify)]
#[serde(untagged)]
#[tsify(hashmap_as_object, missing_as_null)]
// A short-lived report converted to JavaScript at once; boxing buys nothing.
#[allow(clippy::large_enum_variant)]
pub enum JitStatistics {
    Disabled(DisabledStatistics),
    Enabled(Statistics),
}
pub(super) fn statistics() -> JitStatistics {
    #[cfg(target_os = "emscripten")]
    if let Some(stats) = fujimod_emulator_cpu_unicorn::jit::emscripten::statistics() {
        return JitStatistics::Enabled(stats);
    }
    JitStatistics::Disabled(DisabledStatistics { enabled: false })
}
#[cfg(test)]
mod tests {
    use super::*;
    fn parse(text: &str) -> Result<Option<JitOptions>, String> {
        let value: JitSelection = serde_json::from_str(text).map_err(|e| e.to_string())?;
        selection(Some(value))
    }
    #[test]
    fn rust_owns_default_selection_types_and_option_bounds() {
        assert_eq!(selection(None).unwrap().unwrap().max_functions, 32768);
        assert!(parse("false").unwrap().is_none());
        assert_eq!(parse("true").unwrap().unwrap().threshold, 32);
        assert_eq!(
            parse(r#"{"threshold":2,"batchSize":1}"#)
                .unwrap()
                .unwrap()
                .threshold,
            2
        );
        for text in [
            "null",
            "0",
            r#""true""#,
            "[]",
            r#"{"threshold":0}"#,
            r#"{"batchSize":1.5}"#,
            r#"{"unknown":1}"#,
        ] {
            assert!(parse(text).is_err(), "{text}");
        }
    }
}
