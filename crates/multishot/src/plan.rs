//! MULTI-SHOT STACK capture plan and merge pipeline.
//!
//! A capture plan is three nested loops; the innermost runs fastest:
//!
//! ```text
//! for focus step { for exposure step { for burst frame { capture } } }
//! ```
//!
//! The merge is a list of stages, each reducing one axis of the plan:
//!
//! | Stage | Reduces | Input | Output |
//! | --- | --- | --- | --- |
//! | resolution | burst | aligned CFA frames | linear RGB on a finer grid |
//! | noise | burst | aligned CFA frames | linear RGB at sensor resolution |
//! | exposure | exposure steps | linear RGB images | linear RGB, wider range |
//! | focus | focus steps | linear RGB images | linear RGB, deeper focus |
//!
//! Burst stages read CFA rows through [`crate::source::FrameSource`] and write
//! strips to a [`crate::merge::StripSink`]. Image stages will read strips of
//! their inputs and write strips the same way, so the whole pipeline runs with
//! bounded memory. This crate implements the resolution stage; the noise
//! stage is the resolution stage at scale 1. The exposure stage runs on CFA
//! frames when each exposure step is a single frame (an exposure bracket):
//! the same merge with [`ResolutionParams::bracket`] set. Exposure after a
//! burst stage (an image stage) and the focus stage are declared so plans
//! can be validated, and are refused.

use crate::merge::ResolutionParams;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CapturePlan {
    pub burst_frames: usize,
    /// Exposure offsets in EV, one per exposure step.
    pub exposure_steps_ev: Vec<f64>,
    pub focus_steps: usize,
}

/// Position of one frame in the plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanIndex {
    pub burst: usize,
    pub exposure: usize,
    pub focus: usize,
}

impl CapturePlan {
    pub fn burst(frames: usize) -> Self {
        Self {
            burst_frames: frames,
            exposure_steps_ev: vec![0.0],
            focus_steps: 1,
        }
    }

    pub fn frame_count(&self) -> usize {
        self.burst_frames * self.exposure_steps_ev.len() * self.focus_steps
    }

    pub fn index_of(&self, i: PlanIndex) -> usize {
        (i.focus * self.exposure_steps_ev.len() + i.exposure) * self.burst_frames + i.burst
    }

    pub fn position(&self, frame: usize) -> PlanIndex {
        let burst = frame % self.burst_frames;
        let rest = frame / self.burst_frames;
        PlanIndex {
            burst,
            exposure: rest % self.exposure_steps_ev.len(),
            focus: rest / self.exposure_steps_ev.len(),
        }
    }

    /// Frame indices of one burst (fixed exposure and focus step).
    pub fn burst_frames_of(&self, exposure: usize, focus: usize) -> Vec<usize> {
        (0..self.burst_frames)
            .map(|burst| {
                self.index_of(PlanIndex {
                    burst,
                    exposure,
                    focus,
                })
            })
            .collect()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Stage {
    Resolution(ResolutionParams),
    /// Robust average at sensor resolution.
    Noise(ResolutionParams),
    /// Exposure bracket merge on CFA frames (`bracket` set).
    Exposure(ResolutionParams),
    Focus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Burst,
    Exposure,
    Focus,
}

impl Stage {
    pub fn axis(&self) -> Axis {
        match self {
            Stage::Resolution(_) | Stage::Noise(_) => Axis::Burst,
            Stage::Exposure(_) => Axis::Exposure,
            Stage::Focus => Axis::Focus,
        }
    }

    pub fn noise() -> Self {
        Stage::Noise(ResolutionParams {
            scale: 1.0,
            sigma: 0.6,
            radius: 1.8,
            ..ResolutionParams::default()
        })
    }

    /// Exposure bracket merge at sensor resolution.
    pub fn exposure() -> Self {
        Stage::Exposure(ResolutionParams {
            bracket: true,
            ..match Stage::noise() {
                Stage::Noise(p) => p,
                _ => unreachable!(),
            }
        })
    }
}

/// Checks that `stages` reduce every axis the plan has, in plan order (burst,
/// then exposure, then focus), each exactly once.
pub fn validate(plan: &CapturePlan, stages: &[Stage]) -> Result<(), String> {
    if plan.burst_frames == 0 || plan.exposure_steps_ev.is_empty() || plan.focus_steps == 0 {
        return Err("every plan axis needs at least one step".into());
    }
    let axes: Vec<Axis> = stages.iter().map(Stage::axis).collect();
    let mut expected = Vec::new();
    if plan.burst_frames > 1 {
        expected.push(Axis::Burst);
    }
    if plan.exposure_steps_ev.len() > 1 {
        expected.push(Axis::Exposure);
    }
    if plan.focus_steps > 1 {
        expected.push(Axis::Focus);
    }
    if axes != expected {
        return Err(format!(
            "stages reduce {axes:?}; the plan needs {expected:?}"
        ));
    }
    for (k, s) in stages.iter().enumerate() {
        match s {
            Stage::Exposure(p) if k > 0 || !p.bracket => {
                return Err(
                    "the exposure stage runs only on CFA brackets: one frame per exposure \
                     step, first in the plan, with `bracket` set"
                        .into(),
                );
            }
            Stage::Focus => {
                return Err("the focus stage is not implemented yet".into());
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_loop_order() {
        let plan = CapturePlan {
            burst_frames: 4,
            exposure_steps_ev: vec![-2.0, 0.0, 2.0],
            focus_steps: 2,
        };
        assert_eq!(plan.frame_count(), 24);
        assert_eq!(
            plan.position(5),
            PlanIndex {
                burst: 1,
                exposure: 1,
                focus: 0
            }
        );
        assert_eq!(
            plan.index_of(PlanIndex {
                burst: 3,
                exposure: 2,
                focus: 1
            }),
            23
        );
        assert_eq!(plan.burst_frames_of(1, 1), vec![16, 17, 18, 19]);
        assert!(
            validate(
                &CapturePlan::burst(20),
                &[Stage::Resolution(ResolutionParams::default())]
            )
            .is_ok()
        );
        assert!(validate(&plan, &[Stage::noise(), Stage::exposure(), Stage::Focus]).is_err());
        assert!(validate(&plan, &[Stage::noise()]).is_err());
        let bracket = CapturePlan {
            burst_frames: 1,
            exposure_steps_ev: vec![0.0, 1.0, 2.0],
            focus_steps: 1,
        };
        assert!(validate(&bracket, &[Stage::exposure()]).is_ok());
        assert!(validate(&bracket, &[Stage::Exposure(ResolutionParams::default())]).is_err());
        let bursts = CapturePlan {
            burst_frames: 4,
            ..bracket
        };
        assert!(validate(&bursts, &[Stage::noise(), Stage::exposure()]).is_err());
    }
}
