//! Physical XP5 key names that key scripts and the viewer use; each names a
//! catalog control through the body's panel.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Key {
    Menu,
    Back,
    Up,
    Down,
    Left,
    Right,
    ShutterHalf,
    ShutterFull,
    RearPlus,
    RearMinus,
    FrontPlus,
    FrontMinus,
    /// Delete button (X-T5 scanner debug name DRIVE).
    Delete,
    /// View-mode button.
    ViewMode,
    Fn1,
    Fn2,
    /// Quick-menu (Q) button.
    Quick,
    /// Focus-stick tilts and press.
    StickUp,
    StickDown,
    StickLeft,
    StickRight,
    StickPush,
    /// Playback button (scanner debug name K_PLAY).
    Play,
    /// AE-L (exposure lock) button.
    AeLock,
    /// AF-ON button.
    AfOn,
    /// Focus-stick diagonal tilts.
    StickUpLeft,
    StickUpRight,
    StickDownLeft,
    StickDownRight,
    /// Rear and front command-dial pushes.
    RearPush,
    FrontPush,
}
impl Key {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.to_ascii_uppercase().as_str() {
            "MENU" | "OK" => Ok(Self::Menu),
            "BACK" | "DISP" | "DISP/BACK" => Ok(Self::Back),
            "UP" => Ok(Self::Up),
            "DOWN" => Ok(Self::Down),
            "LEFT" => Ok(Self::Left),
            "RIGHT" => Ok(Self::Right),
            "S1" | "HALF" | "SHUTTER_HALF" => Ok(Self::ShutterHalf),
            "S2" | "FULL" | "SHUTTER_FULL" => Ok(Self::ShutterFull),
            "REAR+" => Ok(Self::RearPlus),
            "REAR-" => Ok(Self::RearMinus),
            "FRONT+" => Ok(Self::FrontPlus),
            "FRONT-" => Ok(Self::FrontMinus),
            "DELETE" | "DRIVE" => Ok(Self::Delete),
            "VIEW" | "VIEW_MODE" => Ok(Self::ViewMode),
            "FN1" => Ok(Self::Fn1),
            "FN2" => Ok(Self::Fn2),
            "Q" | "QUICK" => Ok(Self::Quick),
            "STICK_UP" => Ok(Self::StickUp),
            "STICK_DOWN" => Ok(Self::StickDown),
            "STICK_LEFT" => Ok(Self::StickLeft),
            "STICK_RIGHT" => Ok(Self::StickRight),
            "STICK_PUSH" => Ok(Self::StickPush),
            "PLAY" | "K_PLAY" | "PLAYBACK" => Ok(Self::Play),
            "AEL" | "AE-L" | "AE_L" | "AE_LOCK" => Ok(Self::AeLock),
            "AFON" | "AF-ON" | "AF_ON" => Ok(Self::AfOn),
            "STICK_UL" | "STICK_UP_LEFT" => Ok(Self::StickUpLeft),
            "STICK_UR" | "STICK_UP_RIGHT" => Ok(Self::StickUpRight),
            "STICK_DL" | "STICK_DOWN_LEFT" => Ok(Self::StickDownLeft),
            "STICK_DR" | "STICK_DOWN_RIGHT" => Ok(Self::StickDownRight),
            "REAR_PUSH" | "CMD_PUSH" => Ok(Self::RearPush),
            "FRONT_PUSH" | "CMD2_PUSH" => Ok(Self::FrontPush),
            _ => Err(format!("unknown physical key {s:?}")),
        }
    }
    pub fn dial(self) -> bool {
        matches!(
            self,
            Self::RearPlus | Self::RearMinus | Self::FrontPlus | Self::FrontMinus
        )
    }
}
