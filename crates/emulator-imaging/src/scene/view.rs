//! A bounded sensor readout window in its declared full-field coordinates.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Window")]
pub struct View {
    pub field: [usize; 2],
    pub rectangle: [usize; 4],
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Window {
    field: [usize; 2],
    rectangle: [usize; 4],
}
impl TryFrom<Window> for View {
    type Error = String;
    fn try_from(value: Window) -> Result<Self, Self::Error> {
        Self::new(value.field, value.rectangle)
    }
}
impl View {
    pub fn new(field: [usize; 2], rectangle: [usize; 4]) -> Result<Self, String> {
        let [x, y, width, height] = rectangle;
        if field.contains(&0)
            || field.iter().any(|&n| n > 8192)
            || width == 0
            || height == 0
            || x.checked_add(width).is_none_or(|n| n > field[0])
            || y.checked_add(height).is_none_or(|n| n > field[1])
        {
            return Err("sensor readout window exceeds its declared field".into());
        }
        Ok(Self { field, rectangle })
    }
    /// Center sample mapped into an independently sized source raster.
    pub fn coordinate(self, axis: usize, value: usize, grid: usize, source: usize) -> usize {
        let start = self.rectangle[axis];
        let span = self.rectangle[axis + 2];
        ((2 * start * grid + (2 * value + 1) * span) * source / (2 * self.field[axis] * grid))
            .min(source - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn centers_preserve_crop_coordinates_at_every_consumer_resolution() {
        let view = View::new([2496, 3344], [735, 1010, 1002, 1364]).unwrap();
        assert_eq!(view.coordinate(0, 0, 720, 720), 212);
        assert_eq!(view.coordinate(1, 479, 480, 480), 340);
        assert!(View::new([2496, 3344], [2490, 0, 8, 1]).is_err());
        assert!(serde_json::from_str::<View>(r#"{"field":[1,1],"rectangle":[0,0,2,1]}"#).is_err());
    }
}
