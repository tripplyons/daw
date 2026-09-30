//! Automation clips: breakpoint envelopes over a single parameter.
//!
//! Each point owns the shape of the segment that follows it, so shape and
//! tension edits touch exactly one point.

use serde::{Deserialize, Serialize};

use crate::Project;
use crate::time::{Grid, TimeSignature, Ticks};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Shape {
    /// Stay at this point's value until the next point.
    Hold,
    Linear,
    /// Power curve. Tension in -1..1: negative bows toward the start value,
    /// positive toward the end value.
    Curve,
    /// Symmetric ease in and out, sharpened by tension.
    SCurve,
    /// Quantized ramp with `n` equal steps.
    Stairs(u8),
    /// Alternates between the two values `n` times across the segment.
    Pulse(u8),
}

impl Shape {
    pub const CYCLE: [Shape; 6] = [
        Shape::Linear,
        Shape::Curve,
        Shape::SCurve,
        Shape::Hold,
        Shape::Stairs(4),
        Shape::Pulse(4),
    ];

    pub fn next(self) -> Shape {
        let index = Shape::CYCLE
            .iter()
            .position(|s| std::mem::discriminant(s) == std::mem::discriminant(&self))
            .unwrap_or(0);
        Shape::CYCLE[(index + 1) % Shape::CYCLE.len()]
    }

    pub fn name(self) -> String {
        match self {
            Shape::Hold => "hold".into(),
            Shape::Linear => "linear".into(),
            Shape::Curve => "curve".into(),
            Shape::SCurve => "s-curve".into(),
            Shape::Stairs(n) => format!("stairs {n}"),
            Shape::Pulse(n) => format!("pulse {n}"),
        }
    }

    pub fn uses_tension(self) -> bool {
        matches!(self, Shape::Curve | Shape::SCurve)
    }
}

impl std::fmt::Display for Shape {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub time: Ticks,
    /// Normalized parameter value, 0..1.
    pub value: f32,
    pub shape: Shape,
    pub tension: f32,
}

impl Point {
    pub fn new(time: Ticks, value: f32) -> Self {
        Self { time, value: value.clamp(0.0, 1.0), shape: Shape::Linear, tension: 0.0 }
    }
}

/// Tempo automation spans the whole `Project::BPM` range.
const TEMPO_MIN: f64 = *Project::BPM.start();
const TEMPO_MAX: f64 = *Project::BPM.end();

pub fn tempo_from_normalized(value: f32) -> f64 {
    TEMPO_MIN + f64::from(value) * (TEMPO_MAX - TEMPO_MIN)
}

pub fn tempo_to_normalized(bpm: f64) -> f32 {
    ((bpm - TEMPO_MIN) / (TEMPO_MAX - TEMPO_MIN)).clamp(0.0, 1.0) as f32
}

/// Insert volume is linear gain 0..2. Channel volume is already 0..1.
pub fn insert_volume_from_normalized(value: f32) -> f32 {
    value * 2.0
}

pub fn insert_volume_to_normalized(volume: f32) -> f32 {
    volume / 2.0
}

/// Pan runs from -1 (left) to 1 (right).
pub fn pan_from_normalized(value: f32) -> f32 {
    value * 2.0 - 1.0
}

pub fn pan_to_normalized(pan: f32) -> f32 {
    (pan + 1.0) / 2.0
}

/// Shape a 0..1 position within a segment into a 0..1 blend factor.
pub fn shape_factor(shape: Shape, tension: f32, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    match shape {
        Shape::Hold => 0.0,
        Shape::Linear => t,
        Shape::Curve => {
            // Map tension -1..1 to an exponent 1/8..8 on a log scale.
            let exponent = 8f32.powf(-tension.clamp(-1.0, 1.0));
            t.powf(exponent)
        }
        Shape::SCurve => {
            // Exponent 2 at zero tension; 0.5..8 across the range. Below 1 the
            // curve flattens in the middle instead of at the ends.
            let exponent = 2.0 * 4f32.powf(tension.clamp(-1.0, 1.0));
            if t < 0.5 {
                0.5 * (2.0 * t).powf(exponent)
            } else {
                1.0 - 0.5 * (2.0 * (1.0 - t)).powf(exponent)
            }
        }
        Shape::Stairs(n) => {
            let n = f32::from(n.max(1));
            ((t * n).floor() / n).min(1.0)
        }
        Shape::Pulse(n) => {
            let n = f32::from(n.max(1));
            if ((t * n * 2.0).floor() as u32).is_multiple_of(2) { 0.0 } else { 1.0 }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Envelope {
    /// Sorted by time. Points may share a time to express a jump.
    pub points: Vec<Point>,
}

impl Envelope {
    /// Value at `time`. Before the first point the first value holds, after
    /// the last the last value holds. `None` for an empty envelope.
    pub fn value_at(&self, time: f64) -> Option<f32> {
        let first = self.points.first()?;
        if time <= first.time as f64 {
            return Some(first.value);
        }
        let after = self.points.partition_point(|p| (p.time as f64) <= time);
        if after >= self.points.len() {
            return self.points.last().map(|p| p.value);
        }
        let a = self.points[after - 1];
        let b = self.points[after];
        let span = (b.time - a.time) as f64;
        let t = if span == 0.0 { 1.0 } else { ((time - a.time as f64) / span) as f32 };
        let k = shape_factor(a.shape, a.tension, t);
        Some(a.value + (b.value - a.value) * k)
    }

    /// Insert keeping time order; returns the new point's index. A point
    /// inserted at an existing time goes after the existing ones.
    pub fn insert(&mut self, point: Point) -> usize {
        let index = self.points.partition_point(|p| p.time <= point.time);
        self.points.insert(index, point);
        index
    }

    pub fn sort(&mut self) {
        self.points.sort_by_key(|p| p.time);
    }

    /// Segment index containing `time`, i.e. the point that owns the shape.
    pub fn segment_at(&self, time: f64) -> Option<usize> {
        let after = self.points.partition_point(|p| (p.time as f64) <= time);
        (after > 0 && after < self.points.len()).then(|| after - 1)
    }
}

/// One automation clip placed on the playlist.
#[derive(Debug, Clone)]
pub struct Segment {
    pub start: Ticks,
    pub end: Ticks,
    pub offset: Ticks,
    /// Loop length of the automation clip; the envelope repeats past it.
    pub length: Ticks,
    pub envelope: Envelope,
}

impl Segment {
    /// The clip's value at song tick `tick`, which the caller has checked
    /// lies inside the clip.
    pub fn value_at(&self, tick: f64) -> Option<f32> {
        let local = tick - self.start as f64 + self.offset as f64;
        let local = if self.length > 0 { local % self.length as f64 } else { local };
        self.envelope.value_at(local)
    }
}

/// Value snapping choices for the automation editor.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ValueSnap {
    Off,
    /// Snap to multiples of this fraction of the full range.
    Step(f32),
    /// Snap to the parameter's own discrete steps (step count from the plugin).
    Discrete,
    /// Snap to the value of the neighboring points when close.
    Neighbors,
}

impl ValueSnap {
    pub const CHOICES: [ValueSnap; 7] = [
        ValueSnap::Off,
        ValueSnap::Step(0.01),
        ValueSnap::Step(0.05),
        ValueSnap::Step(0.10),
        ValueSnap::Step(0.25),
        ValueSnap::Discrete,
        ValueSnap::Neighbors,
    ];
}

impl std::fmt::Display for ValueSnap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValueSnap::Off => f.write_str("off"),
            ValueSnap::Step(s) => write!(f, "{}%", (s * 100.0).round()),
            ValueSnap::Discrete => f.write_str("param steps"),
            ValueSnap::Neighbors => f.write_str("neighbors"),
        }
    }
}

/// Time snapping for automation points: a grid, or the times of other points.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum TimeSnap {
    Grid(Grid),
    Points,
}

impl std::fmt::Display for TimeSnap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TimeSnap::Grid(grid) => grid.fmt(f),
            TimeSnap::Points => f.write_str("points"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SnapContext<'a> {
    pub time: TimeSnap,
    pub value: ValueSnap,
    pub signature: TimeSignature,
    /// Discrete step count of the target parameter, 0 when continuous.
    pub param_steps: u32,
    /// Points not being dragged, used by `Points` and `Neighbors` snapping.
    pub others: &'a [Point],
    /// Snap distance for `Points` and `Neighbors`, in ticks and value units.
    pub time_radius: Ticks,
    pub value_radius: f32,
}

impl SnapContext<'_> {
    pub fn snap_time(&self, time: Ticks) -> Ticks {
        match self.time {
            TimeSnap::Grid(grid) => grid.snap(time, self.signature),
            TimeSnap::Points => self
                .others
                .iter()
                .map(|p| p.time)
                .filter(|&t| t.abs_diff(time) <= self.time_radius)
                .min_by_key(|&t| t.abs_diff(time))
                .unwrap_or(time),
        }
    }

    pub fn snap_value(&self, value: f32) -> f32 {
        let value = value.clamp(0.0, 1.0);
        match self.value {
            ValueSnap::Off => value,
            ValueSnap::Step(step) => ((value / step).round() * step).clamp(0.0, 1.0),
            ValueSnap::Discrete if self.param_steps > 0 => {
                let n = self.param_steps as f32;
                (value * n).round() / n
            }
            ValueSnap::Discrete => value,
            ValueSnap::Neighbors => self
                .others
                .iter()
                .map(|p| p.value)
                .filter(|v| (v - value).abs() <= self.value_radius)
                .min_by(|a, b| (a - value).abs().total_cmp(&(b - value).abs()))
                .unwrap_or(value),
        }
    }
}

/// Reduce a freehand stroke to the points needed to stay within `tolerance`
/// (Ramer-Douglas-Peucker). Coordinates are (time, value) in any units where
/// distance is meaningful; the caller scales time to screen space first.
pub fn simplify(stroke: &[(f64, f64)], tolerance: f64) -> Vec<usize> {
    if stroke.len() <= 2 {
        return (0..stroke.len()).collect();
    }
    let mut keep = vec![false; stroke.len()];
    keep[0] = true;
    keep[stroke.len() - 1] = true;
    let mut stack = vec![(0, stroke.len() - 1)];
    while let Some((start, end)) = stack.pop() {
        let (ax, ay) = stroke[start];
        let (bx, by) = stroke[end];
        let (dx, dy) = (bx - ax, by - ay);
        let length = (dx * dx + dy * dy).sqrt();
        let mut farthest = (0.0, start);
        for (i, &(px, py)) in stroke.iter().enumerate().take(end).skip(start + 1) {
            let distance = if length == 0.0 {
                ((px - ax).powi(2) + (py - ay).powi(2)).sqrt()
            } else {
                (dy * px - dx * py + bx * ay - by * ax).abs() / length
            };
            if distance > farthest.0 {
                farthest = (distance, i);
            }
        }
        if farthest.0 > tolerance {
            keep[farthest.1] = true;
            stack.push((start, farthest.1));
            stack.push((farthest.1, end));
        }
    }
    keep.iter().enumerate().filter_map(|(i, &k)| k.then_some(i)).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum LfoShape {
    Sine,
    Triangle,
    Saw,
    Square,
}

impl LfoShape {
    pub const ALL: [LfoShape; 4] = [LfoShape::Sine, LfoShape::Triangle, LfoShape::Saw, LfoShape::Square];
}

impl std::fmt::Display for LfoShape {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            LfoShape::Sine => "sine",
            LfoShape::Triangle => "triangle",
            LfoShape::Saw => "saw",
            LfoShape::Square => "square",
        })
    }
}

/// Generate points for an LFO over `start..end`, one cycle per `period`
/// ticks, spanning `low..high`. Uses segment shapes rather than dense points
/// so the result stays editable.
pub fn lfo(shape: LfoShape, start: Ticks, end: Ticks, period: Ticks, low: f32, high: f32) -> Vec<Point> {
    let period = period.max(1);
    let mut points = Vec::new();
    let mut t = start;
    let point = |time, value, shape, tension| Point { time, value, shape, tension };
    while t < end {
        let half = period / 2;
        match shape {
            LfoShape::Sine => {
                // Two S-curves per cycle approximate a sine closely.
                let quarter = period / 4;
                points.push(point(t, (low + high) / 2.0, Shape::Curve, -0.35));
                points.push(point(t + quarter, high, Shape::Curve, 0.35));
                points.push(point(t + half, (low + high) / 2.0, Shape::Curve, -0.35));
                points.push(point(t + half + quarter, low, Shape::Curve, 0.35));
            }
            LfoShape::Triangle => {
                points.push(point(t, low, Shape::Linear, 0.0));
                points.push(point(t + half, high, Shape::Linear, 0.0));
            }
            LfoShape::Saw => {
                points.push(point(t, low, Shape::Linear, 0.0));
                points.push(point(t + period - 1, high, Shape::Hold, 0.0));
            }
            LfoShape::Square => {
                points.push(point(t, high, Shape::Hold, 0.0));
                points.push(point(t + half, low, Shape::Hold, 0.0));
            }
        }
        t += period;
    }
    points.retain(|p| p.time < end);
    let closing = match shape {
        LfoShape::Sine => (low + high) / 2.0,
        LfoShape::Triangle | LfoShape::Saw => low,
        LfoShape::Square => high,
    };
    points.push(point(end, closing, Shape::Hold, 0.0));
    points
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(points: &[(Ticks, f32, Shape, f32)]) -> Envelope {
        Envelope {
            points: points
                .iter()
                .map(|&(time, value, shape, tension)| Point { time, value, shape, tension })
                .collect(),
        }
    }

    #[test]
    fn values_interpolate_hold_and_jump_at_coincident_points() {
        let env = envelope(&[(0, 0.0, Shape::Linear, 0.0), (100, 1.0, Shape::Linear, 0.0)]);
        assert_eq!(env.value_at(-5.0), Some(0.0));
        assert!((env.value_at(25.0).unwrap() - 0.25).abs() < 1e-6);
        assert_eq!(env.value_at(500.0), Some(1.0));
        let hold = envelope(&[(0, 0.2, Shape::Hold, 0.0), (100, 0.8, Shape::Linear, 0.0)]);
        assert_eq!(hold.value_at(99.0), Some(0.2));
        assert_eq!(hold.value_at(100.0), Some(0.8));
        let jump = envelope(&[
            (0, 0.0, Shape::Linear, 0.0),
            (100, 1.0, Shape::Linear, 0.0),
            (100, 0.0, Shape::Linear, 0.0),
            (200, 0.5, Shape::Linear, 0.0),
        ]);
        assert!((jump.value_at(99.0).unwrap() - 0.99).abs() < 1e-5);
        assert!((jump.value_at(150.0).unwrap() - 0.25).abs() < 1e-5);
    }

    #[test]
    fn curve_tension_bends_toward_ends() {
        let mid = |tension| shape_factor(Shape::Curve, tension, 0.5);
        assert!((mid(0.0) - 0.5).abs() < 1e-6);
        assert!(mid(0.5) > 0.5, "positive tension rises early");
        assert!(mid(-0.5) < 0.5, "negative tension rises late");
        for tension in [-1.0, -0.3, 0.0, 0.3, 1.0] {
            assert_eq!(shape_factor(Shape::Curve, tension, 0.0), 0.0);
            assert!((shape_factor(Shape::Curve, tension, 1.0) - 1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn s_curve_is_symmetric_and_monotonic() {
        for tension in [-1.0, 0.0, 1.0] {
            let f = |t| shape_factor(Shape::SCurve, tension, t);
            assert!((f(0.5) - 0.5).abs() < 1e-6);
            assert!((f(0.25) + f(0.75) - 1.0).abs() < 1e-5);
            let mut last = 0.0;
            for i in 0..=20 {
                let v = f(i as f32 / 20.0);
                assert!(v >= last - 1e-6);
                last = v;
            }
        }
    }

    #[test]
    fn stairs_and_pulse() {
        assert_eq!(shape_factor(Shape::Stairs(4), 0.0, 0.3), 0.25);
        assert_eq!(shape_factor(Shape::Stairs(4), 0.0, 0.99), 0.75);
        assert_eq!(shape_factor(Shape::Pulse(2), 0.0, 0.1), 0.0);
        assert_eq!(shape_factor(Shape::Pulse(2), 0.0, 0.3), 1.0);
        assert_eq!(shape_factor(Shape::Pulse(2), 0.0, 0.6), 0.0);
    }

    fn context<'a>(time: TimeSnap, value: ValueSnap, others: &'a [Point]) -> SnapContext<'a> {
        SnapContext {
            time,
            value,
            signature: TimeSignature::default(),
            param_steps: 0,
            others,
            time_radius: 50,
            value_radius: 0.05,
        }
    }

    #[test]
    fn time_snap_modes() {
        let others = [Point::new(1000, 0.3)];
        let grid = context(TimeSnap::Grid(Grid::Beat), ValueSnap::Off, &others);
        assert_eq!(grid.snap_time(1300), 960);
        let points = context(TimeSnap::Points, ValueSnap::Off, &others);
        assert_eq!(points.snap_time(1030), 1000);
        assert_eq!(points.snap_time(1200), 1200);
    }

    #[test]
    fn value_snap_modes() {
        let others = [Point::new(0, 0.62)];
        let off = context(TimeSnap::Grid(Grid::Off), ValueSnap::Off, &others);
        assert_eq!(off.snap_value(1.4), 1.0);
        let step = context(TimeSnap::Grid(Grid::Off), ValueSnap::Step(0.25), &others);
        assert_eq!(step.snap_value(0.4), 0.5);
        let mut discrete = context(TimeSnap::Grid(Grid::Off), ValueSnap::Discrete, &others);
        discrete.param_steps = 2;
        assert_eq!(discrete.snap_value(0.3), 0.5);
        let neighbors = context(TimeSnap::Grid(Grid::Off), ValueSnap::Neighbors, &others);
        assert_eq!(neighbors.snap_value(0.6), 0.62);
        assert_eq!(neighbors.snap_value(0.4), 0.4);
    }

    #[test]
    fn simplify_keeps_corners() {
        let stroke: Vec<(f64, f64)> = (0..=20)
            .map(|i| {
                let x = f64::from(i);
                (x, if i <= 10 { x } else { 20.0 - x })
            })
            .collect();
        assert_eq!(simplify(&stroke, 0.1), vec![0, 10, 20]);
    }

    #[test]
    fn lfo_spans_range() {
        let points = lfo(LfoShape::Triangle, 0, 960 * 4, 960, 0.2, 0.8);
        let env = Envelope { points };
        assert!((env.value_at(480.0).unwrap() - 0.8).abs() < 1e-6);
        assert!((env.value_at(960.0).unwrap() - 0.2).abs() < 1e-6);
        let square = Envelope { points: lfo(LfoShape::Square, 0, 960, 960, 0.0, 1.0) };
        assert_eq!(square.value_at(100.0), Some(1.0));
        assert_eq!(square.value_at(500.0), Some(0.0));
    }
}
