//! Musical time. Positions are in ticks at a fixed resolution per quarter note.

use serde::{Deserialize, Serialize};

pub const TICKS_PER_BEAT: u32 = 960;

pub type Ticks = u64;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TimeSignature {
    pub numerator: u32,
    pub denominator: u32,
}

impl TimeSignature {
    pub fn ticks_per_bar(self) -> Ticks {
        u64::from(TICKS_PER_BEAT) * 4 * u64::from(self.numerator) / u64::from(self.denominator)
    }
}

impl Default for TimeSignature {
    fn default() -> Self {
        Self { numerator: 4, denominator: 4 }
    }
}

/// Grid divisions used for time snapping across the piano roll, playlist, and
/// automation editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Grid {
    Off,
    Bar,
    Beat,
    /// 1/n of a whole note, e.g. `Division(16)` is a sixteenth.
    Division(u32),
    Triplet(u32),
    Dotted(u32),
}

impl Grid {
    pub const CHOICES: [Grid; 14] = [
        Grid::Off,
        Grid::Bar,
        Grid::Beat,
        Grid::Division(8),
        Grid::Division(16),
        Grid::Division(32),
        Grid::Division(64),
        Grid::Triplet(8),
        Grid::Triplet(16),
        Grid::Triplet(32),
        Grid::Dotted(4),
        Grid::Dotted(8),
        Grid::Dotted(16),
        Grid::Division(2),
    ];

    /// Step length in ticks, or `None` when snapping is off.
    pub fn step(self, signature: TimeSignature) -> Option<Ticks> {
        let whole = u64::from(TICKS_PER_BEAT) * 4;
        match self {
            Grid::Off => None,
            Grid::Bar => Some(signature.ticks_per_bar()),
            Grid::Beat => Some(whole / u64::from(signature.denominator)),
            Grid::Division(n) => Some(whole / u64::from(n)),
            Grid::Triplet(n) => Some(whole * 2 / (3 * u64::from(n))),
            Grid::Dotted(n) => Some(whole * 3 / (2 * u64::from(n))),
        }
    }

    pub fn snap(self, ticks: Ticks, signature: TimeSignature) -> Ticks {
        match self.step(signature) {
            None => ticks,
            Some(step) => (ticks + step / 2) / step * step,
        }
    }

    pub fn snap_floor(self, ticks: Ticks, signature: TimeSignature) -> Ticks {
        match self.step(signature) {
            None => ticks,
            Some(step) => ticks / step * step,
        }
    }

    pub fn label(self) -> String {
        match self {
            Grid::Off => "off".into(),
            Grid::Bar => "bar".into(),
            Grid::Beat => "beat".into(),
            Grid::Division(n) => format!("1/{n}"),
            Grid::Triplet(n) => format!("1/{n}T"),
            Grid::Dotted(n) => format!("1/{n}."),
        }
    }
}

impl std::fmt::Display for Grid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label())
    }
}

pub fn ticks_to_seconds(ticks: f64, bpm: f64) -> f64 {
    ticks / f64::from(TICKS_PER_BEAT) * 60.0 / bpm
}

pub fn seconds_to_ticks(seconds: f64, bpm: f64) -> f64 {
    seconds * bpm / 60.0 * f64::from(TICKS_PER_BEAT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_steps_and_snapping() {
        let sig = TimeSignature::default();
        assert_eq!(Grid::Bar.step(sig), Some(3840));
        assert_eq!(Grid::Beat.step(sig), Some(960));
        assert_eq!(Grid::Division(16).step(sig), Some(240));
        assert_eq!(Grid::Triplet(8).step(sig), Some(320));
        assert_eq!(Grid::Dotted(8).step(sig), Some(720));
        assert_eq!(Grid::Off.step(sig), None);
        assert_eq!(Grid::Beat.snap(470, sig), 0);
        assert_eq!(Grid::Beat.snap(490, sig), 960);
        assert_eq!(Grid::Beat.snap_floor(1900, sig), 960);
        assert_eq!(Grid::Off.snap(123, sig), 123);
    }
}
