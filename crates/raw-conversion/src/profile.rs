//! The RAW conversion profile, device property 0xD185, and the settings a job
//! may change in it. Layout and encodings follow petabyt/libfuji `fp/src/d185.c`
//! and `fp/src/fp.h` (README.md lists what is and is not confirmed).
//!
//! The camera fills the profile from the uploaded RAF. A job patches only the
//! fields it names and leaves every other word, including the header, as the
//! camera reported it.
use serde::{Deserialize, Serialize};

/// Byte offset of the first property word.
const PROPERTIES: usize = 0x201;

/// Property index in the D185 word list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    FileType = 1,
    ImageSize = 2,
    ImageQuality = 3,
    ExposureBias = 4,
    DynamicRange = 5,
    WideDynamicRange = 6,
    FilmSimulation = 7,
    Grain = 8,
    ColorChrome = 9,
    WhiteBalanceAsShot = 10,
    WhiteBalanceShiftRed = 12,
    WhiteBalanceShiftBlue = 13,
    Highlight = 15,
    Shadow = 16,
    Color = 17,
    Sharpness = 18,
    NoiseReduction = 19,
    LensModulation = 20,
    ColorSpace = 21,
    /// MONOCHROMATIC COLOR warm/cool on monochrome films; a white-balance
    /// word on color films.
    MonochromeWarmCool = 22,
    ColorChromeBlue = 24,
    /// MONOCHROMATIC COLOR magenta/green on monochrome films; a white-balance
    /// word on color films.
    MonochromeMagentaGreen = 25,
    Clarity = 26,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Film {
    Provia,
    Velvia,
    Astia,
    ProNegHi,
    ProNegStd,
    Monochrome,
    MonochromeYe,
    MonochromeR,
    MonochromeG,
    Sepia,
    ClassicChrome,
    Acros,
    AcrosYe,
    AcrosR,
    AcrosG,
    Eterna,
    ClassicNeg,
    EternaBleachBypass,
    NostalgicNeg,
    RealaAce,
}
impl Film {
    fn code(self) -> u32 {
        self as u32 + 1
    }
    /// Films whose words 22 and 25 hold MONOCHROMATIC COLOR.
    fn monochromatic(self) -> bool {
        use Film::*;
        matches!(
            self,
            Monochrome
                | MonochromeYe
                | MonochromeR
                | MonochromeG
                | Acros
                | AcrosYe
                | AcrosR
                | AcrosG
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Strength {
    Off,
    Weak,
    Strong,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum GrainSize {
    Small,
    Large,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum OnOff {
    On,
    Off,
}

/// Every setting a job file may name. Steps are the camera's menu steps.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub film: Option<Film>,
    /// `L_3_2` is the only size the jobs need.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_size: Option<ImageSize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_quality: Option<ImageQuality>,
    /// Thousandths of an EV.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exposure_bias_mev: Option<i32>,
    /// 100, 200 or 400.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dynamic_range: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dynamic_range_priority: Option<Strength>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grain: Option<Strength>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grain_size: Option<GrainSize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_chrome: Option<Strength>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_chrome_blue: Option<Strength>,
    /// `ON` keeps the RAF's own white balance.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub white_balance_as_shot: Option<OnOff>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub white_balance_shift_red: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub white_balance_shift_blue: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub highlight: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shadow: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sharpness: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub noise_reduction: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clarity: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lens_modulation: Option<OnOff>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_space: Option<ColorSpace>,
    /// MONOCHROMATIC COLOR WC; on monochrome films only, 0 when unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub monochrome_warm_cool: Option<i32>,
    /// MONOCHROMATIC COLOR MG; on monochrome films only, 0 when unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub monochrome_magenta_green: Option<i32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum ImageSize {
    #[serde(rename = "L_3_2")]
    L3x2,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ImageQuality {
    Fine,
    Normal,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ColorSpace {
    Srgb,
}

impl Settings {
    /// `other`'s fields replace this one's.
    pub fn overlay(&self, other: &Settings) -> Settings {
        Settings {
            film: other.film.or(self.film),
            image_size: other.image_size.or(self.image_size),
            image_quality: other.image_quality.or(self.image_quality),
            exposure_bias_mev: other.exposure_bias_mev.or(self.exposure_bias_mev),
            dynamic_range: other.dynamic_range.or(self.dynamic_range),
            dynamic_range_priority: other.dynamic_range_priority.or(self.dynamic_range_priority),
            grain: other.grain.or(self.grain),
            grain_size: other.grain_size.or(self.grain_size),
            color_chrome: other.color_chrome.or(self.color_chrome),
            color_chrome_blue: other.color_chrome_blue.or(self.color_chrome_blue),
            white_balance_as_shot: other.white_balance_as_shot.or(self.white_balance_as_shot),
            white_balance_shift_red: other
                .white_balance_shift_red
                .or(self.white_balance_shift_red),
            white_balance_shift_blue: other
                .white_balance_shift_blue
                .or(self.white_balance_shift_blue),
            highlight: other.highlight.or(self.highlight),
            shadow: other.shadow.or(self.shadow),
            color: other.color.or(self.color),
            sharpness: other.sharpness.or(self.sharpness),
            noise_reduction: other.noise_reduction.or(self.noise_reduction),
            clarity: other.clarity.or(self.clarity),
            lens_modulation: other.lens_modulation.or(self.lens_modulation),
            color_space: other.color_space.or(self.color_space),
            monochrome_warm_cool: other.monochrome_warm_cool.or(self.monochrome_warm_cool),
            monochrome_magenta_green: other
                .monochrome_magenta_green
                .or(self.monochrome_magenta_green),
        }
    }

    /// The words this setting list writes, in field order. JPEG output is
    /// always selected.
    pub fn words(&self) -> Result<Vec<(Field, u32)>, String> {
        let mut out = vec![(Field::FileType, 7)];
        let tenths = |v: i32, lo: i32, hi: i32, name: &str| {
            if (lo..=hi).contains(&v) {
                Ok((v * 10) as u32)
            } else {
                Err(format!("{name} {v} is outside {lo}..={hi}"))
            }
        };
        if let Some(v) = self.image_size {
            out.push((
                Field::ImageSize,
                match v {
                    ImageSize::L3x2 => 7,
                },
            ));
        }
        if let Some(v) = self.image_quality {
            out.push((
                Field::ImageQuality,
                match v {
                    ImageQuality::Fine => 2,
                    ImageQuality::Normal => 3,
                },
            ));
        }
        if let Some(v) = self.exposure_bias_mev {
            out.push((Field::ExposureBias, v as u32));
        }
        if let Some(v) = self.dynamic_range {
            if ![100, 200, 400].contains(&v) {
                return Err(format!("dynamic range {v}"));
            }
            out.push((Field::DynamicRange, v));
        }
        if let Some(v) = self.dynamic_range_priority {
            out.push((Field::WideDynamicRange, strength(v) - 1));
        }
        if let Some(film) = self.film {
            out.push((Field::FilmSimulation, film.code()));
        }
        if self.grain.is_some() || self.grain_size.is_some() {
            let size = match self.grain_size.unwrap_or(GrainSize::Small) {
                GrainSize::Small => 0,
                GrainSize::Large => 2,
            };
            // Size is encoded only with grain on; OFF plus LARGE would read as STRONG.
            let value = match self.grain.unwrap_or(Strength::Off) {
                Strength::Off => 1,
                g => strength(g) + size,
            };
            out.push((Field::Grain, value));
        }
        if let Some(v) = self.color_chrome {
            out.push((Field::ColorChrome, strength(v)));
        }
        if let Some(v) = self.white_balance_as_shot {
            out.push((Field::WhiteBalanceAsShot, on_off(v)));
        }
        if let Some(v) = self.white_balance_shift_red {
            out.push((
                Field::WhiteBalanceShiftRed,
                tenths(v, -9, 9, "white balance shift red")?,
            ));
        }
        if let Some(v) = self.white_balance_shift_blue {
            out.push((
                Field::WhiteBalanceShiftBlue,
                tenths(v, -9, 9, "white balance shift blue")?,
            ));
        }
        if let Some(v) = self.highlight {
            out.push((Field::Highlight, tenths(v, -2, 4, "highlight")?));
        }
        if let Some(v) = self.shadow {
            out.push((Field::Shadow, tenths(v, -2, 4, "shadow")?));
        }
        if let Some(v) = self.color {
            out.push((Field::Color, tenths(v, -4, 4, "color")?));
        }
        if let Some(v) = self.sharpness {
            out.push((Field::Sharpness, tenths(v, -4, 4, "sharpness")?));
        }
        if let Some(v) = self.noise_reduction {
            // libfuji fp.h FujiNoiseReduction: the camera's own nonmonotonic codes.
            let code = match v {
                4 => 0x5000,
                3 => 0x6000,
                2 => 0x0000,
                1 => 0x1000,
                0 => 0x2000,
                -1 => 0x3000,
                -2 => 0x4000,
                -3 => 0x7000,
                -4 => 0x8000,
                _ => return Err(format!("noise reduction {v} is outside -4..=4")),
            };
            out.push((Field::NoiseReduction, code));
        }
        if let Some(v) = self.lens_modulation {
            out.push((Field::LensModulation, on_off(v)));
        }
        if let Some(v) = self.color_space {
            out.push((
                Field::ColorSpace,
                match v {
                    ColorSpace::Srgb => 1,
                },
            ));
        }
        let mono = self.film.is_some_and(Film::monochromatic);
        if !mono && (self.monochrome_warm_cool.is_some() || self.monochrome_magenta_green.is_some())
        {
            return Err("MONOCHROMATIC COLOR needs a monochrome film".into());
        }
        if mono {
            let wc = self.monochrome_warm_cool.unwrap_or(0);
            let mg = self.monochrome_magenta_green.unwrap_or(0);
            out.push((
                Field::MonochromeWarmCool,
                tenths(wc, -18, 18, "monochrome warm/cool")?,
            ));
            out.push((
                Field::MonochromeMagentaGreen,
                tenths(mg, -18, 18, "monochrome magenta/green")?,
            ));
        }
        if let Some(v) = self.color_chrome_blue {
            out.push((Field::ColorChromeBlue, strength(v) - 1));
        }
        if let Some(v) = self.clarity {
            out.push((Field::Clarity, tenths(v, -5, 5, "clarity")?));
        }
        Ok(out)
    }
}

fn strength(v: Strength) -> u32 {
    match v {
        Strength::Off => 1,
        Strength::Weak => 2,
        Strength::Strong => 3,
    }
}
fn on_off(v: OnOff) -> u32 {
    match v {
        OnOff::On => 1,
        OnOff::Off => 2,
    }
}

/// A D185 value as the camera reported it.
#[derive(Clone, Debug, PartialEq)]
pub struct Profile(pub Vec<u8>);

impl Profile {
    /// How many property words the camera's header declares.
    pub fn count(&self) -> usize {
        self.0
            .get(0..2)
            .map_or(0, |b| u16::from_le_bytes([b[0], b[1]]) as usize)
    }
    pub fn get(&self, field: Field) -> Option<u32> {
        if field as usize >= self.count() {
            return None;
        }
        let at = PROPERTIES + 4 * field as usize;
        self.0
            .get(at..at + 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    }
    /// Writes the words this profile has; returns the fields it lacks.
    pub fn patch(&self, words: &[(Field, u32)]) -> (Profile, Vec<Field>) {
        let mut out = self.clone();
        let mut missing = Vec::new();
        for &(field, value) in words {
            if self.get(field).is_none() {
                missing.push(field);
                continue;
            }
            let at = PROPERTIES + 4 * field as usize;
            out.0[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        (out, missing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn profile(count: u16) -> Profile {
        let mut bytes = vec![0u8; PROPERTIES + 4 * count as usize];
        bytes[0..2].copy_from_slice(&count.to_le_bytes());
        Profile(bytes)
    }

    #[test]
    fn libfuji_encodings() {
        let s: Settings = serde_json::from_str(
            r#"{"film":"ACROS_R","grain":"STRONG","grain_size":"LARGE","noise_reduction":-4,
                "monochrome_warm_cool":9,"monochrome_magenta_green":-9,"clarity":-5,
                "color_chrome_blue":"WEAK","white_balance_as_shot":"ON","color_space":"SRGB"}"#,
        )
        .unwrap();
        let w = s.words().unwrap();
        let get = |f| w.iter().find(|(x, _)| *x == f).unwrap().1;
        assert_eq!(get(Field::FilmSimulation), 14);
        assert_eq!(get(Field::Grain), 5);
        assert_eq!(get(Field::NoiseReduction), 0x8000);
        assert_eq!(get(Field::MonochromeWarmCool), 90);
        assert_eq!(get(Field::MonochromeMagentaGreen), (-90i32) as u32);
        assert_eq!(get(Field::Clarity), (-50i32) as u32);
        assert_eq!(get(Field::ColorChromeBlue), 1);
        assert_eq!(get(Field::WhiteBalanceAsShot), 1);
        assert_eq!(get(Field::ColorSpace), 1);
        assert_eq!(get(Field::FileType), 7);
    }

    #[test]
    fn color_films_keep_white_balance_words() {
        let s = Settings {
            film: Some(Film::Provia),
            ..Default::default()
        };
        assert!(
            !s.words()
                .unwrap()
                .iter()
                .any(|(f, _)| *f == Field::MonochromeWarmCool)
        );
        let bad = Settings {
            film: Some(Film::Provia),
            monochrome_warm_cool: Some(1),
            ..Default::default()
        };
        assert!(bad.words().is_err());
        assert!(
            Settings {
                clarity: Some(6),
                ..Default::default()
            }
            .words()
            .is_err()
        );
    }

    #[test]
    fn patch_keeps_other_words_and_reports_missing_fields() {
        let mut base = profile(25);
        base.0[PROPERTIES + 4 * 22..PROPERTIES + 4 * 23]
            .copy_from_slice(&0x0902_6000u32.to_le_bytes());
        let (patched, missing) = base.patch(&[(Field::FilmSimulation, 1), (Field::Clarity, 50)]);
        assert_eq!(missing, [Field::Clarity]);
        assert_eq!(patched.get(Field::FilmSimulation), Some(1));
        assert_eq!(patched.get(Field::MonochromeWarmCool), Some(0x0902_6000));
        assert_eq!(patched.0.len(), base.0.len());
    }

    #[test]
    fn overlay_replaces_named_fields_only() {
        let base = Settings {
            clarity: Some(0),
            dynamic_range: Some(100),
            ..Default::default()
        };
        let job = Settings {
            clarity: Some(3),
            film: Some(Film::Velvia),
            ..Default::default()
        };
        let merged = base.overlay(&job);
        assert_eq!(merged.clarity, Some(3));
        assert_eq!(merged.dynamic_range, Some(100));
        assert_eq!(merged.film, Some(Film::Velvia));
    }
}
