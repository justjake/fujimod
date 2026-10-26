//! Language negotiation over the rows actually offered by a firmware.
use fujimod_catalog::first_start::FirstStartMenu;
use std::collections::BTreeMap;

fn chinese_script(tag: &str) -> Option<&str> {
    let parts = tag.split('-').collect::<Vec<_>>();
    if parts.first().copied() != Some("zh") {
        return None;
    }
    if tag == "zh" {
        return Some("hans");
    }
    if parts.contains(&"hant") {
        return Some("hant");
    }
    if parts.contains(&"hans") {
        return Some("hans");
    }
    if parts.iter().any(|p| matches!(*p, "tw" | "hk" | "mo")) {
        return Some("hant");
    }
    if parts.iter().any(|p| matches!(*p, "cn" | "sg")) {
        return Some("hans");
    }
    None
}

pub(super) fn month_days(year: u16, month: u16) -> u16 {
    match month {
        2 => {
            if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) {
                29
            } else {
                28
            }
        }
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

pub(super) fn calendar(seconds: u32) -> [u16; 5] {
    let mut days = seconds / 86_400;
    let mut year = 2000;
    loop {
        let count = if month_days(year, 2) == 29 { 366 } else { 365 };
        if days < count {
            break;
        }
        days -= count;
        year += 1;
    }
    let mut month = 1;
    while days >= u32::from(month_days(year, month)) {
        days -= u32::from(month_days(year, month));
        month += 1;
    }
    [
        year,
        month,
        days as u16 + 1,
        ((seconds / 3600) % 24) as u16,
        ((seconds / 60) % 60) as u16,
    ]
}

pub(super) fn select(
    menu: &FirstStartMenu,
    preferred: &[String],
    explicit: Option<&str>,
) -> Result<usize, String> {
    if let Some(name) = explicit {
        return menu
            .languages
            .iter()
            .position(|language| language.name == name)
            .ok_or_else(|| format!("START MENU does not offer {name}"));
    }
    let tags: BTreeMap<String, String> =
        serde_json::from_str(include_str!("language-tags.json")).map_err(|e| e.to_string())?;
    let offered = menu
        .languages
        .iter()
        .map(|language| {
            tags.get(&language.name)
                .map(|tag| tag.to_ascii_lowercase())
                .ok_or_else(|| format!("no BCP 47 tag for {}", language.name))
        })
        .collect::<Result<Vec<_>, _>>()?;
    for preference in preferred {
        let tag = preference.to_ascii_lowercase();
        if let Some(index) = offered.iter().position(|offered| *offered == tag) {
            return Ok(index);
        }
        let primary = match tag.split('-').next().unwrap_or_default() {
            "nb" | "nn" => "no",
            primary => primary,
        };
        if let Some(index) = offered.iter().position(|offered| {
            offered.split('-').next() == Some(primary)
                && (primary != "zh"
                    || chinese_script(&tag)
                        .is_some_and(|script| chinese_script(offered) == Some(script)))
        }) {
            return Ok(index);
        }
    }
    offered
        .iter()
        .position(|tag| tag == "en")
        .ok_or("START MENU has no English fallback".into())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_seconds_convert_across_leap_days() {
        assert_eq!(calendar(0), [2000, 1, 1, 0, 0]);
        assert_eq!(calendar(762_525_240), [2024, 2, 29, 12, 34]);
        assert_eq!(month_days(2100, 2), 28);
    }
    fn menu() -> FirstStartMenu {
        fujimod_catalog::ReferenceCatalog::bundled()
            .unwrap()
            .resolve_firmware("x70-110")
            .unwrap()
            .first_start
            .unwrap()
    }
    fn selected(preferred: &[&str]) -> String {
        let menu = menu();
        let index = select(
            &menu,
            &preferred.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            None,
        )
        .unwrap();
        menu.languages[index].name.clone()
    }
    #[test]
    fn ordered_preferences_exact_and_primary_then_english() {
        assert_eq!(selected(&["xx", "DE-at", "en"]), "BKUP_LANGUAGE_GERMAN");
        assert_eq!(selected(&["en-US", "de"]), "BKUP_LANGUAGE_ENGLISH");
        assert_eq!(selected(&["fr-CA"]), "BKUP_LANGUAGE_FRENCH");
        assert_eq!(selected(&[]), "BKUP_LANGUAGE_ENGLISH");
        assert_eq!(selected(&["xx"]), "BKUP_LANGUAGE_ENGLISH");
    }
    #[test]
    fn norwegian_browser_tags_select_norway() {
        for tag in ["no", "nb", "nb-NO", "nn", "nn-NO"] {
            assert_eq!(selected(&[tag]), "BKUP_LANGUAGE_NORWAY");
        }
    }
    #[test]
    fn chinese_scripts_never_cross() {
        for tag in ["zh-Hant", "zh-TW", "zh-HK", "zh-Hant-CN"] {
            assert_eq!(selected(&[tag]), "BKUP_LANGUAGE_CHINA2");
        }
        for tag in ["zh", "zh-Hans", "zh-CN", "zh-SG", "zh-Hans-TW"] {
            assert_eq!(selected(&[tag]), "BKUP_LANGUAGE_CHINA");
        }
        let mut menu = menu();
        menu.languages
            .retain(|language| language.name != "BKUP_LANGUAGE_CHINA2");
        let index = select(&menu, &["zh-Hant".into(), "de".into()], None).unwrap();
        assert_eq!(menu.languages[index].name, "BKUP_LANGUAGE_GERMAN");
    }
    #[test]
    fn explicit_language_must_be_offered() {
        let menu = menu();
        assert!(select(&menu, &[], Some("missing")).is_err());
        let index = select(&menu, &[], Some("BKUP_LANGUAGE_GERMAN")).unwrap();
        assert_eq!(menu.languages[index].value, 2);
    }
}
