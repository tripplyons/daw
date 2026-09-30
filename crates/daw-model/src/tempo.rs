//! Song time under tempo automation.

use crate::automation::{Segment, tempo_from_normalized};
use crate::time::{Ticks, seconds_to_ticks, ticks_to_seconds};

/// Ticks between tempo changes inside a tempo automation clip. Each step
/// plays at the clip's tempo at the step's middle.
const STEP: Ticks = 30;

#[derive(Debug, Clone, Copy)]
struct Step {
    tick: f64,
    /// Song time at `tick`.
    seconds: f64,
    bpm: f64,
}

/// The song's tempo at every tick when playing from the start: the project
/// tempo before the first tempo clip, each clip's curve inside it, and the
/// clip's last tempo held after it.
#[derive(Debug, Clone)]
pub struct TempoMap {
    /// Sorted by tick. The first starts at tick 0 and also covers negative
    /// ticks.
    steps: Vec<Step>,
}

impl TempoMap {
    /// A map for tempo automation clips `segments`, sorted by start. Where
    /// clips overlap, the one that starts later wins, as in playback.
    pub fn new(bpm: f64, segments: &[Segment]) -> TempoMap {
        let mut map = TempoMap { steps: vec![Step { tick: 0.0, seconds: 0.0, bpm }] };
        let mut bounds: Vec<Ticks> = segments.iter().flat_map(|s| [s.start, s.end]).collect();
        bounds.sort_unstable();
        bounds.dedup();
        for pair in bounds.windows(2) {
            let (from, to) = (pair[0], pair[1]);
            let Some(segment) = segments.iter().rev().find(|s| s.start <= from && from < s.end) else { continue };
            let mut tick = from;
            while tick < to {
                let next = (tick + STEP).min(to);
                if let Some(value) = segment.value_at((tick + next) as f64 / 2.0) {
                    map.change(tick as f64, tempo_from_normalized(value));
                }
                tick = next;
            }
        }
        map
    }

    /// Change to `bpm` at `tick`, which is not before the last step.
    fn change(&mut self, tick: f64, bpm: f64) {
        let last = *self.steps.last().expect("a first step");
        if bpm == last.bpm {
            return;
        }
        if tick == last.tick {
            self.steps.pop();
        }
        let seconds = last.seconds + ticks_to_seconds(tick - last.tick, last.bpm);
        self.steps.push(Step { tick, seconds, bpm });
    }

    fn step_at(&self, tick: f64) -> Step {
        self.steps[self.steps.partition_point(|s| s.tick <= tick).saturating_sub(1)]
    }

    pub fn bpm_at(&self, tick: f64) -> f64 {
        self.step_at(tick).bpm
    }

    /// Song time at `tick`, in seconds from tick 0.
    pub fn seconds_at(&self, tick: f64) -> f64 {
        let step = self.step_at(tick);
        step.seconds + ticks_to_seconds(tick - step.tick, step.bpm)
    }

    /// The tick heard `seconds` into the song.
    pub fn tick_at(&self, seconds: f64) -> f64 {
        let step = self.steps[self.steps.partition_point(|s| s.seconds <= seconds).saturating_sub(1)];
        step.tick + seconds_to_ticks(seconds - step.seconds, step.bpm)
    }

    pub fn seconds_between(&self, from: f64, to: f64) -> f64 {
        self.seconds_at(to) - self.seconds_at(from)
    }

    /// Ticks that `seconds` of audio starting at tick `start` spans.
    pub fn ticks_spanned(&self, start: f64, seconds: f64) -> f64 {
        self.tick_at(self.seconds_at(start) + seconds) - start
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::{Envelope, Point, tempo_to_normalized};
    use crate::time::TICKS_PER_BEAT;

    const BEAT: Ticks = TICKS_PER_BEAT as Ticks;

    fn flat(start: Ticks, end: Ticks, bpm: f64) -> Segment {
        Segment { start, end, offset: 0, length: 0, envelope: Envelope { points: vec![Point::new(0, tempo_to_normalized(bpm))] } }
    }

    #[test]
    fn without_automation_the_project_tempo_holds() {
        let map = TempoMap::new(120.0, &[]);
        assert_eq!(map.seconds_at(BEAT as f64 * 4.0), 2.0);
        assert_eq!(map.seconds_at(-(BEAT as f64)), -0.5);
        assert_eq!(map.tick_at(1.0), BEAT as f64 * 2.0);
    }

    #[test]
    fn clips_set_the_tempo_and_their_last_tempo_holds_after_them() {
        let bpm = |value: f64| tempo_from_normalized(tempo_to_normalized(value));
        let map = TempoMap::new(120.0, &[flat(BEAT * 2, BEAT * 4, 60.0)]);
        assert_eq!(map.bpm_at(BEAT as f64), 120.0);
        assert_eq!(map.bpm_at(BEAT as f64 * 3.0), bpm(60.0));
        assert_eq!(map.bpm_at(BEAT as f64 * 9.0), bpm(60.0));
        // Two beats at 120, then two at about 60.
        let seconds = 1.0 + ticks_to_seconds(BEAT as f64 * 2.0, bpm(60.0));
        assert!((map.seconds_at(BEAT as f64 * 4.0) - seconds).abs() < 1e-9);
        assert!((map.tick_at(seconds) - BEAT as f64 * 4.0).abs() < 1e-6);
        assert!((map.ticks_spanned(0.0, seconds) - BEAT as f64 * 4.0).abs() < 1e-6);
    }

    #[test]
    fn later_clips_win_where_clips_overlap() {
        let map = TempoMap::new(120.0, &[flat(0, BEAT * 4, 100.0), flat(BEAT, BEAT * 2, 200.0)]);
        let bpm = |value: f64| tempo_from_normalized(tempo_to_normalized(value));
        assert_eq!(map.bpm_at(0.0), bpm(100.0));
        assert_eq!(map.bpm_at(BEAT as f64 * 1.5), bpm(200.0));
        assert_eq!(map.bpm_at(BEAT as f64 * 3.0), bpm(100.0));
    }
}
