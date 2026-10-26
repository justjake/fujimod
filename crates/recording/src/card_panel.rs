//! Optional video presentation of a caller-supplied SD directory snapshot.
//! Only the card's top and a short caller-pruned directory tree cover the video.
use crate::{
    Result,
    canvas::{Canvas, rounded_rect},
    overlays::{BODY, BORDER, label as line},
    text,
};
use serde::Deserialize;
use tiny_skia::PathBuilder;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CardRow {
    pub name: String,
    pub path: String,
    pub depth: u32,
    pub directory: bool,
    pub bytes: u64,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CardPanel {
    pub label: String,
    pub rows: Vec<CardRow>,
    pub highlight: Option<String>,
}

impl CardPanel {
    pub fn parse(json: &str) -> Result<Self> {
        let panel: Self = serde_json::from_str(json).map_err(|e| e.to_string())?;
        if panel.rows.len() > 6
            || panel.label.chars().count() > 64
            || panel.rows.iter().any(|r| {
                r.depth > 32
                    || r.name.is_empty()
                    || r.name.chars().count() > 255
                    || r.name.chars().any(char::is_control)
            })
        {
            return Err("Invalid SD card presentation".into());
        }
        if panel
            .highlight
            .as_ref()
            .is_some_and(|path| !panel.rows.iter().any(|r| &r.path == path))
        {
            return Err("Highlighted file is absent from the SD snapshot".into());
        }
        Ok(panel)
    }
}

fn bytes(value: u64) -> String {
    if value < 1024 {
        format!("{value} B")
    } else if value < 1024 * 1024 {
        format!("{:.1} KB", value as f64 / 1024.)
    } else {
        format!("{:.1} MB", value as f64 / (1024. * 1024.))
    }
}

pub(crate) fn draw(canvas: &mut Canvas, panel: &CardPanel, visibility: f32, shake: (f32, f32)) {
    if visibility <= 0. {
        return;
    }
    let scale = canvas.height() / 480.;
    let (x, y, w, h) = (
        canvas.width() - 314. * scale + shake.0 * scale,
        canvas.height() - 250. * scale + (1. - visibility) * 265. * scale + shake.1 * scale,
        300. * scale,
        400. * scale,
    );
    let mut body = PathBuilder::new();
    body.move_to(x + 10. * scale, y);
    body.line_to(x + w - 42. * scale, y);
    body.line_to(x + w, y + 42. * scale);
    body.line_to(x + w, y + h - 10. * scale);
    body.quad_to(x + w, y + h, x + w - 10. * scale, y + h);
    body.line_to(x + 10. * scale, y + h);
    body.quad_to(x, y + h, x, y + h - 10. * scale);
    body.line_to(x, y + 10. * scale);
    body.quad_to(x, y, x + 10. * scale, y);
    body.close();
    let body = body.finish().expect("SD outline");
    canvas.fill(&body, BODY);
    canvas.stroke(&body, BORDER, scale);
    for index in 0..7 {
        if let Some(pad) = rounded_rect(
            x + (20. + index as f32 * 29.) * scale,
            y + 12. * scale,
            17. * scale,
            20. * scale,
            2. * scale,
        ) {
            canvas.fill(&pad, [145, 122, 67, 255]);
        }
    }
    line(
        canvas,
        "SD CARD",
        x + 18. * scale,
        y + 44. * scale,
        11. * scale,
        w - 36. * scale,
        [142, 142, 147, 255],
    );
    line(
        canvas,
        if panel.label.is_empty() {
            "(untitled)"
        } else {
            &panel.label
        },
        x + 18. * scale,
        y + 63. * scale,
        17. * scale,
        w - 36. * scale,
        [238, 238, 242, 255],
    );
    for (index, row) in panel.rows.iter().enumerate() {
        let top = y + (96. + index as f32 * 22.) * scale;
        let indent = row.depth.min(8) as f32 * 14. * scale;
        if Some(&row.path) == panel.highlight.as_ref()
            && let Some(band) = rounded_rect(
                x + 10. * scale,
                top,
                w - 20. * scale,
                21. * scale,
                4. * scale,
            )
        {
            canvas.fill(&band, [35, 55, 40, 255]);
        }
        let left = x + 18. * scale + indent;
        if row.directory {
            let mut folder = PathBuilder::new();
            folder.move_to(left, top + 7. * scale);
            folder.line_to(left + 5. * scale, top + 7. * scale);
            folder.line_to(left + 7. * scale, top + 10. * scale);
            folder.line_to(left + 12. * scale, top + 10. * scale);
            folder.line_to(left + 12. * scale, top + 18. * scale);
            folder.line_to(left, top + 18. * scale);
            folder.close();
            canvas.stroke(
                &folder.finish().expect("folder"),
                [174, 174, 181, 255],
                scale,
            );
        } else if let Some(icon) = rounded_rect(
            left + 2. * scale,
            top + 7. * scale,
            9. * scale,
            12. * scale,
            scale,
        ) {
            canvas.stroke(&icon, [142, 142, 147, 255], scale);
        }
        let amount = if row.directory {
            String::new()
        } else {
            bytes(row.bytes)
        };
        let amount_width = text::layout(&amount, 0., 0., 10. * scale, 15. * scale).width;
        let right = x + w - 17. * scale;
        line(
            canvas,
            &row.name,
            left + 19. * scale,
            top + 3. * scale,
            12. * scale,
            right - left - 27. * scale - amount_width,
            [222, 222, 228, 255],
        );
        line(
            canvas,
            &amount,
            right - amount_width,
            top + 5. * scale,
            10. * scale,
            amount_width + scale,
            [142, 142, 147, 255],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Frame;
    #[test]
    fn panel_overlays_only_the_card_corner_and_requires_a_real_highlight() {
        let frame = Frame::rgb(720, 480, vec![60; 720 * 480 * 3]).unwrap();
        let json = r#"{"label":"2026-10-25","rows":[{"name":"DCIM","path":"/DCIM","depth":0,"directory":true,"bytes":0}],"highlight":"/DCIM"}"#;
        let panel = CardPanel::parse(json).unwrap();
        let mut canvas = Canvas::from_rgb(frame.pixels(), frame.width(), frame.height());
        draw(&mut canvas, &panel, 1., (0., 0.));
        let shown = canvas.rgb();
        for row in shown.chunks_exact(frame.width() as usize * 3) {
            assert_eq!(&row[..400 * 3], &vec![60; 400 * 3]);
        }
        assert_eq!(shown.len(), frame.pixels().len());
        assert_eq!(&shown[..200 * 720 * 3], &frame.pixels()[..200 * 720 * 3]);
        assert_ne!(shown, frame.pixels());
        assert!(CardPanel::parse(&json.replace("/DCIM\"}", "/MISSING\"}")).is_err());
    }
}
