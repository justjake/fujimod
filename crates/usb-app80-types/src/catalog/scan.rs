//! Read one row from the current wireless scan results.
use super::{ArgumentKind, CommandError};

/// An index checked against a scan count observed from a prior station-scan reply.
/// The firmware may replace its results before this selection is executed, so
/// this check does not guarantee that the live list still contains the row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScanSelection(u8);

impl ScanSelection {
    /// Check `index` against the count reported by the most recent scan reply.
    /// This only validates the supplied observation; firmware may replace
    /// its scan-result list before the request executes.
    pub fn new(index: u8, observed_count: u8) -> Result<Self, CommandError> {
        if index >= observed_count {
            return Err(CommandError::Argument(ArgumentKind::ScanSelection));
        }
        Ok(Self(index))
    }

    /// The selected row index sent to the firmware.
    pub fn value(self) -> u8 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_is_bounded_by_observed_count() {
        assert!(matches!(
            ScanSelection::new(0, 0),
            Err(CommandError::Argument(ArgumentKind::ScanSelection))
        ));
        assert!(matches!(
            ScanSelection::new(3, 3),
            Err(CommandError::Argument(ArgumentKind::ScanSelection))
        ));

        assert_eq!(ScanSelection::new(0, 1).unwrap().value(), 0);
        assert_eq!(ScanSelection::new(63, 64).unwrap().value(), 63);
        assert_eq!(ScanSelection::new(254, 255).unwrap().value(), 254);
    }
}
