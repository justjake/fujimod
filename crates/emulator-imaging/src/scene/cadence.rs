use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameEdge {
    pub index: u64,
    pub tick: u64,
}

/// Two physical exposure phases shared by their source and pixel consumers.
pub type FramePhases = std::sync::Arc<std::sync::Mutex<[Option<FrameEdge>; 2]>>;

/// An external frame clock. Missed edges are skipped in constant time.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FrameCadence {
    period: u64,
    next: Option<FrameEdge>,
}
impl FrameCadence {
    pub fn new(period: u64, first_tick: u64) -> Result<Self, String> {
        if period == 0 {
            return Err("frame period must be positive".into());
        }
        Ok(Self {
            period,
            next: Some(FrameEdge {
                index: 1,
                tick: first_tick,
            }),
        })
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.period == 0 || self.next.is_some_and(|e| e.index == 0) {
            Err("invalid frame clock snapshot".into())
        } else {
            Ok(())
        }
    }
    pub fn next_deadline(&self) -> Option<u64> {
        self.next.map(|e| e.tick)
    }
    pub fn period(&self) -> u64 {
        self.period
    }
    /// A shorter declared period can move the next edge earlier. A longer
    /// period preserves the already scheduled edge; it applies afterward.
    pub fn set_period(&mut self, period: u64, now: u64) -> Result<(), String> {
        if period == 0 {
            return Err("frame period must be positive".into());
        }
        if period < self.period {
            let next = now
                .checked_add(period)
                .ok_or("frame clock deadline overflows")?;
            if let Some(edge) = &mut self.next {
                edge.tick = edge.tick.min(next);
            }
        }
        self.period = period;
        Ok(())
    }
    pub fn take_due(&mut self, now: u64) -> Option<FrameEdge> {
        let edge = self.next?;
        if now < edge.tick {
            return None;
        }
        let skipped = (now - edge.tick) / self.period;
        let Some(index) = edge.index.checked_add(skipped) else {
            self.next = None;
            return None;
        };
        let result = FrameEdge {
            index,
            tick: edge.tick + skipped * self.period,
        };
        self.next = result
            .tick
            .checked_add(self.period)
            .zip(result.index.checked_add(1))
            .map(|(tick, index)| FrameEdge { tick, index });
        Some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clock_and_activity() {
        let mut c = FrameCadence::new(10, 5).unwrap();
        assert_eq!(c.take_due(4), None);
        assert_eq!(c.take_due(28), Some(FrameEdge { index: 3, tick: 25 }));
        assert_eq!(c.next_deadline(), Some(35));
        c.set_period(4, 29).unwrap();
        assert_eq!(c.next_deadline(), Some(33));
        c.set_period(20, 30).unwrap();
        assert_eq!(c.take_due(33), Some(FrameEdge { index: 4, tick: 33 }));
        assert_eq!(c.next_deadline(), Some(53));
        let mut c = FrameCadence::new(10, u64::MAX).unwrap();
        assert!(c.take_due(u64::MAX).is_some());
        assert_eq!(c.next_deadline(), None);
    }
}
