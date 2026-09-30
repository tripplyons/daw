//! Automation clip editor.
//!
//! Edit tool: click adds a point, drag moves the selection, right click
//! deletes, right drag (or Ctrl drag on macOS) selects a box, Alt drag (or
//! the square handle) bends a segment, double click cycles the segment shape.
//! Cmd (Ctrl on Linux) bypasses snapping and Shift locks the drag to one
//! axis. Keys 1-6 set the shape of the selected points, arrows nudge, Delete
//! and Cmd A/C/V/D edit the selection.
//! The draw tool paints a curve and the line tool replaces a range with a ramp.
//! Clicking or dragging in the ruler sets where the clip ends.

use std::collections::HashMap;

use daw_model::automation::{LfoShape, Point as EnvPoint, Shape, SnapContext, TimeSnap, ValueSnap, lfo};
use daw_model::time::{Grid, Ticks};
use daw_model::{AutomationId, ClipSource, Target};
use iced::keyboard::{Key, Modifiers, key::Named};
use iced::widget::canvas::Canvas;
use iced::widget::{mouse_area, row};
use iced::{Element, Length};

use super::timeline::TimeView;
use super::{label, pick, tool, toggle};
use crate::app::{App, Message as AppMessage};
use crate::menu;

mod editor;
use editor::Editor;

const SNAP_PIXELS: f32 = 8.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Edit,
    Draw,
    Line,
}

/// LFO period in sixteenth notes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LfoRate(pub u32);

impl LfoRate {
    const CHOICES: [LfoRate; 6] = [LfoRate(2), LfoRate(4), LfoRate(8), LfoRate(16), LfoRate(32), LfoRate(64)];

    fn ticks(self) -> Ticks {
        Ticks::from(self.0) * daw_model::STEP_TICKS
    }
}

impl std::fmt::Display for LfoRate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            n if n < 16 => write!(f, "1/{}", 16 / n),
            16 => f.write_str("1 bar"),
            n => write!(f, "{} bars", n / 16),
        }
    }
}

/// The visible slice of normalized values, zoomed with Alt+scroll.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ValueRange {
    pub low: f32,
    pub high: f32,
}

impl ValueRange {
    pub const FULL: ValueRange = ValueRange { low: 0.0, high: 1.0 };
    const MIN_SPAN: f32 = 0.05;

    fn span(self) -> f32 {
        self.high - self.low
    }

    /// Zoom by wheel steps (positive zooms in), keeping `anchor` in place.
    pub fn zoom(self, steps: f32, anchor: f32) -> ValueRange {
        let span = (self.span() / 1.15f32.powf(steps)).clamp(Self::MIN_SPAN, 1.0);
        let ratio = ((anchor - self.low) / self.span()).clamp(0.0, 1.0);
        ValueRange { low: anchor - ratio * span, high: anchor - ratio * span + span }.clamped()
    }

    /// Scroll by wheel steps; positive moves toward higher values.
    pub fn scroll(self, steps: f32) -> ValueRange {
        let shift = steps * self.span() * 0.1;
        ValueRange { low: self.low + shift, high: self.high + shift }.clamped()
    }

    /// Keep the span, moved back inside 0..1.
    fn clamped(self) -> ValueRange {
        let span = self.span().min(1.0);
        if span > 1.0 - 1e-4 {
            return ValueRange::FULL;
        }
        let low = self.low.clamp(0.0, 1.0 - span);
        ValueRange { low, high: low + span }
    }
}

#[derive(Debug)]
pub struct State {
    pub clip: Option<AutomationId>,
    pub selected: Vec<usize>,
    pub tool: Tool,
    pub time_snap: TimeSnap,
    pub value_snap: ValueSnap,
    pub time: TimeView,
    /// Visible part of the 0..1 value range.
    pub values: ValueRange,
    pub lfo_shape: LfoShape,
    pub lfo_rate: LfoRate,
    /// Points being dragged, the pressed one first.
    originals: Vec<(usize, EnvPoint)>,
    drag_changed: bool,
    clipboard: Vec<EnvPoint>,
    /// Last recorded clip-local tick per target during this pass.
    recording: HashMap<Target, f64>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            clip: None,
            selected: Vec::new(),
            tool: Tool::Edit,
            time_snap: TimeSnap::Grid(Grid::Division(16)),
            value_snap: ValueSnap::Off,
            time: TimeView::new(48.0),
            values: ValueRange::FULL,
            lfo_shape: LfoShape::Sine,
            lfo_rate: LfoRate(4),
            originals: Vec::new(),
            drag_changed: false,
            clipboard: Vec::new(),
            recording: HashMap::new(),
        }
    }
}

impl State {
    pub fn cancel_drag(&mut self) {
        self.originals.clear();
        self.drag_changed = false;
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    View(TimeView),
    /// Visible value range, from scrolling or Alt+scroll zoom.
    Values(ValueRange),
    Clip(ClipChoice),
    Bind(TargetChoice),
    Tool(Tool),
    TimeSnap(TimeSnap),
    ValueSnap(ValueSnap),
    Shape(Shape),
    LfoShape(LfoShape),
    LfoRate(LfoRate),
    Lfo,
    Bars(u64),
    /// Drag in the ruler: move the clip end to this tick, rounded to a bar.
    ClipEnd(f64),
    Add { time: f64, value: f32, bypass: bool },
    Begin { index: usize, additive: bool },
    Drag { ticks: f64, value: f32, bypass: bool },
    End,
    Delete(usize),
    BeginTension(usize),
    Tension(usize, f32),
    CycleShape(usize),
    BoxSelect { t0: f64, t1: f64, v0: f32, v1: f32, additive: bool },
    Replace { start: Ticks, end: Ticks, points: Vec<EnvPoint> },
}

impl From<Message> for AppMessage {
    fn from(message: Message) -> Self {
        AppMessage::Automation(message)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClipChoice {
    id: AutomationId,
    name: String,
}

impl std::fmt::Display for ClipChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TargetChoice {
    target: Target,
    name: String,
}

impl std::fmt::Display for TargetChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)
    }
}

fn points(app: &App) -> &[EnvPoint] {
    app.automation.clip.and_then(|id| app.project.automation_clip(id)).map(|c| c.envelope.points.as_slice()).unwrap_or(&[])
}

fn points_mut(app: &mut App) -> Option<&mut Vec<EnvPoint>> {
    let id = app.automation.clip?;
    Some(&mut app.project.automation_clip_mut(id)?.envelope.points)
}

fn target(app: &App) -> Option<Target> {
    app.automation.clip.and_then(|id| app.project.automation_clip(id)).map(|c| c.target)
}

fn snap_context<'a>(app: &App, others: &'a [EnvPoint], ticks_per_px: f64) -> SnapContext<'a> {
    SnapContext {
        time: app.automation.time_snap,
        value: app.automation.value_snap,
        signature: app.project.signature,
        param_steps: target(app).map(|t| app.target_steps(t)).unwrap_or(0),
        others,
        time_radius: (ticks_per_px * f64::from(SNAP_PIXELS)) as Ticks,
        value_radius: 0.03,
    }
}

fn ticks_per_px(app: &App) -> f64 {
    app.automation.time.ticks(1.0)
}

/// Sort points by time and keep the selection on the same points. Also grows
/// the clip to cover points just placed or moved past its end.
fn sort(app: &mut App) {
    let selected = std::mem::take(&mut app.automation.selected);
    let Some(id) = app.automation.clip else { return };
    let Some(clip) = app.project.automation_clip_mut(id) else { return };
    let mut tagged: Vec<(bool, EnvPoint)> =
        clip.envelope.points.iter().enumerate().map(|(i, p)| (selected.contains(&i), *p)).collect();
    tagged.sort_by_key(|(_, p)| p.time);
    clip.envelope.points = tagged.iter().map(|(_, p)| *p).collect();
    let length = clip.length;
    app.automation.selected = tagged.iter().enumerate().filter(|(_, (s, _))| *s).map(|(i, _)| i).collect();
    let end = tagged.iter().filter(|(s, _)| *s).map(|(_, p)| p.time).max().unwrap_or(0);
    if end > length {
        let length = app.project.bars_to(end);
        app.project.set_automation_length(id, length);
    }
}

fn begin_drag(app: &mut App, first: usize) {
    app.automation.cancel_drag();
    let points = points(app).to_vec();
    let mut order: Vec<usize> = vec![first];
    order.extend(app.automation.selected.iter().copied().filter(|&i| i != first));
    app.automation.originals = order.into_iter().filter_map(|i| points.get(i).map(|p| (i, *p))).collect();
}

/// Replace the points in `start..=end` with `new`.
fn replace(app: &mut App, start: Ticks, end: Ticks, new: Vec<EnvPoint>) {
    let Some(points) = points_mut(app) else { return };
    points.retain(|p| p.time < start || p.time > end);
    let count = new.len();
    points.extend(new);
    let first_new = points.len() - count;
    app.automation.selected = (first_new..points.len()).collect();
    sort(app);
}

pub fn update(app: &mut App, message: Message) {
    match message {
        Message::View(view) => app.automation.time = view,
        Message::Values(range) => app.automation.values = range,
        Message::Clip(choice) => {
            app.automation.cancel_drag();
            app.automation.clip = Some(choice.id);
            app.automation.values = ValueRange::FULL;
            app.automation.selected.clear();
        }
        Message::Bind(choice) => {
            app.checkpoint();
            app.bind(choice.target);
        }
        Message::Tool(tool) => app.automation.tool = tool,
        Message::TimeSnap(snap) => app.automation.time_snap = snap,
        Message::ValueSnap(snap) => app.automation.value_snap = snap,
        Message::Shape(shape) => set_shape(app, shape),
        Message::LfoShape(shape) => app.automation.lfo_shape = shape,
        Message::LfoRate(rate) => app.automation.lfo_rate = rate,
        Message::Lfo => apply_lfo(app),
        Message::Bars(bars) => {
            let bar = app.project.signature.ticks_per_bar();
            let Some(id) = app.automation.clip else { return };
            app.checkpoint();
            app.project.set_automation_length(id, bars.max(1) * bar);
            app.edited();
        }
        Message::ClipEnd(tick) => {
            let bar = app.project.signature.ticks_per_bar();
            let length = ((tick / bar as f64).round().max(1.0) as Ticks) * bar;
            let Some(id) = app.automation.clip else { return };
            if app.project.automation_clip(id).is_some_and(|c| c.length != length) {
                app.begin_edit();
                app.project.set_automation_length(id, length);
                app.edited();
            }
        }
        Message::Add { time, value, bypass } => {
            let Some(_) = app.automation.clip else { return };
            app.checkpoint();
            let existing = points(app).to_vec();
            let context = snap_context(app, &existing, ticks_per_px(app));
            let (time, value) = if bypass {
                (time.max(0.0) as Ticks, value.clamp(0.0, 1.0))
            } else {
                (context.snap_time(time.max(0.0) as Ticks), context.snap_value(value))
            };
            // New points continue the shape of the segment they split.
            let shape = existing.iter().rev().find(|p| p.time <= time).map(|p| (p.shape, p.tension));
            let mut point = EnvPoint::new(time, value);
            if let Some((shape, tension)) = shape {
                point.shape = shape;
                point.tension = tension;
            }
            let Some(points) = points_mut(app) else { return };
            points.push(point);
            let index = points.len() - 1;
            app.automation.selected = vec![index];
            begin_drag(app, index);
            app.automation.drag_changed = true;
            app.edited();
        }
        Message::Begin { index, additive } => {
            app.automation.cancel_drag();
            if points(app).get(index).is_none() { return; }
            let selected = &mut app.automation.selected;
            if additive {
                if let Some(position) = selected.iter().position(|&i| i == index) {
                    selected.remove(position);
                    app.automation.originals.clear();
                    return;
                }
                selected.push(index);
            } else if !selected.contains(&index) {
                *selected = vec![index];
            }
            begin_drag(app, index);
        }
        Message::Drag { ticks, value, bypass } => {
            let originals = app.automation.originals.clone();
            let Some(&(_, primary)) = originals.first() else { return };
            let moving: Vec<usize> = originals.iter().map(|(i, _)| *i).collect();
            let others: Vec<EnvPoint> =
                points(app).iter().enumerate().filter(|(i, _)| !moving.contains(i)).map(|(_, p)| *p).collect();
            let context = snap_context(app, &others, ticks_per_px(app));
            let raw_time = (primary.time as f64 + ticks).max(0.0);
            let raw_value = primary.value + value;
            let (time, new_value) = if bypass {
                (raw_time as Ticks, raw_value.clamp(0.0, 1.0))
            } else {
                // Snap the pressed point; the rest keep their offsets.
                let time = if ticks == 0.0 { primary.time } else { context.snap_time(raw_time as Ticks) };
                let value = if value == 0.0 { primary.value } else { context.snap_value(raw_value) };
                (time, value)
            };
            let dt = time as i64 - primary.time as i64;
            let dv = new_value - primary.value;
            let mut changed = Vec::new();
            for (index, original) in originals {
                let point = EnvPoint { time: (original.time as i64 + dt).max(0) as Ticks,
                    value: (original.value + dv).clamp(0.0, 1.0), ..original };
                if points(app).get(index).is_some_and(|old| *old != point) { changed.push((index, point)); }
            }
            if changed.is_empty() { return; }
            if !app.automation.drag_changed { app.checkpoint(); }
            app.automation.drag_changed = true;
            if let Some(points) = points_mut(app) { for (index, point) in changed { points[index] = point; } }
            app.edited();
        }
        Message::End => {
            if app.automation.drag_changed { sort(app); app.edited(); }
            app.automation.cancel_drag();
        }
        Message::Delete(index) => {
            app.checkpoint();
            if let Some(points) = points_mut(app)
                && index < points.len()
                && points.len() > 1
            {
                points.remove(index);
            }
            app.automation.selected.clear();
            app.edited();
        }
        Message::BeginTension(index) => {
            app.automation.cancel_drag();
            if let Some(point) = points(app).get(index).copied() { app.automation.originals.push((index, point)); }
        }
        Message::Tension(index, tension) => {
            if !tension.is_finite() { return; }
            let Some((_, original)) = app.automation.originals.iter().find(|(i, _)| *i == index) else { return };
            if !app.automation.drag_changed && original.tension == tension.clamp(-1.0, 1.0) { return; }
            let Some(old) = points(app).get(index).copied() else { return };
            let point = EnvPoint { shape: if old.shape.uses_tension() { old.shape } else { Shape::Curve },
                tension: tension.clamp(-1.0, 1.0), ..old };
            if old == point { return; }
            if !app.automation.drag_changed { app.checkpoint(); }
            app.automation.drag_changed = true;
            if let Some(points) = points_mut(app) { points[index] = point; }
            app.edited();
        }
        Message::CycleShape(index) => {
            app.checkpoint();
            let mut name = None;
            if let Some(point) = points_mut(app).and_then(|p| p.get_mut(index)) {
                point.shape = point.shape.next();
                name = Some(point.shape.name());
            }
            if let Some(name) = name {
                app.set_status(format!("segment shape: {name}"));
            }
            app.edited();
        }
        Message::BoxSelect { t0, t1, v0, v1, additive } => {
            let hits: Vec<usize> = points(app)
                .iter()
                .enumerate()
                .filter(|(_, p)| (t0..=t1).contains(&(p.time as f64)) && (v0..=v1).contains(&p.value))
                .map(|(i, _)| i)
                .collect();
            let selected = &mut app.automation.selected;
            if !additive {
                selected.clear();
            }
            for hit in hits {
                if !selected.contains(&hit) {
                    selected.push(hit);
                }
            }
        }
        Message::Replace { start, end, points } => {
            app.checkpoint();
            replace(app, start, end, points);
            app.edited();
        }
    }
}

fn set_shape(app: &mut App, shape: Shape) {
    if app.automation.selected.is_empty() {
        return;
    }
    app.checkpoint();
    let selected = app.automation.selected.clone();
    if let Some(points) = points_mut(app) {
        for index in selected {
            if let Some(point) = points.get_mut(index) {
                point.shape = shape;
            }
        }
    }
    app.set_status(format!("segment shape: {}", shape.name()));
    app.edited();
}

fn apply_lfo(app: &mut App) {
    let Some(clip) = app.automation.clip.and_then(|id| app.project.automation_clip(id)) else { return };
    let selected: Vec<EnvPoint> = app.automation.selected.iter().filter_map(|&i| clip.envelope.points.get(i).copied()).collect();
    let (start, end, low, high) = if selected.len() >= 2 {
        let start = selected.iter().map(|p| p.time).min().unwrap_or(0);
        let end = selected.iter().map(|p| p.time).max().unwrap_or(clip.length);
        let low = selected.iter().map(|p| p.value).fold(1.0, f32::min);
        let high = selected.iter().map(|p| p.value).fold(0.0, f32::max);
        (start, end, low, high)
    } else {
        (0, clip.length, 0.0, 0.0)
    };
    let (low, high) = if high - low < 0.02 {
        let center = clip.envelope.value_at(start as f64).unwrap_or(0.5);
        ((center - 0.25).max(0.0), (center + 0.25).min(1.0))
    } else {
        (low, high)
    };
    let mut new = lfo(app.automation.lfo_shape, start, end, app.automation.lfo_rate.ticks(), low, high);
    let closing = clip.envelope.value_at(end as f64).unwrap_or(low);
    if new.last().is_none_or(|p| p.time < end) {
        new.push(EnvPoint::new(end, if selected.len() >= 2 { closing } else { new.first().map(|p| p.value).unwrap_or(low) }));
    }
    app.checkpoint();
    replace(app, start, end, new);
    app.edited();
}

/// Record a parameter movement into its automation clip at the playhead.
/// Points between the previous recorded time and now are overwritten.
pub fn record(app: &mut App, target: Target, value: f32) {
    let id = match app.project.automation_for(target) {
        Some(id) => id,
        None => {
            app.checkpoint_parameter(target);
            app.bind_at(target, value)
        }
    };
    let position = app.position;
    let Some(clip) = app
        .project
        .playlist
        .clips
        .iter()
        .find(|c| c.source == ClipSource::Automation(id) && (c.start as f64) <= position && position < c.end() as f64)
        .cloned()
    else {
        return;
    };
    let Some(length) = app.project.automation_clip(id).map(|a| a.length.max(1)) else { return };
    let local = (position - clip.start as f64 + clip.offset as f64) % length as f64;
    if !app.automation.recording.contains_key(&target) {
        app.checkpoint_parameter(target);
    }
    let previous = app.automation.recording.insert(target, local);
    let Some(automation) = app.project.automation_clip_mut(id) else { return };
    let points = &mut automation.envelope.points;
    if let Some(previous) = previous
        && previous < local
    {
        points.retain(|p| (p.time as f64) <= previous || (p.time as f64) > local);
    }
    points.push(EnvPoint::new(local as Ticks, value));
    points.sort_by_key(|p| p.time);
    app.edited();
}

/// Forget recording passes once playback stops.
pub fn stopped(app: &mut App) {
    app.automation.recording.clear();
}

/// Delete the selected points, keeping at least one. Returns false when
/// nothing is selected.
pub fn delete_selected(app: &mut App) -> bool {
    if app.automation.selected.is_empty() {
        return false;
    }
    app.checkpoint();
    let mut selected = std::mem::take(&mut app.automation.selected);
    selected.sort_unstable();
    if let Some(points) = points_mut(app) {
        for index in selected.into_iter().rev() {
            if index < points.len() && points.len() > 1 {
                points.remove(index);
            }
        }
    }
    app.edited();
    true
}

pub fn key(app: &mut App, key: &Key, modifiers: Modifiers) -> bool {
    let count = points(app).len();
    match key {
        Key::Character(c) if !modifiers.command() && !modifiers.alt() => {
            let Some(digit) = c.as_str().parse::<usize>().ok().filter(|d| (1..=6).contains(d)) else { return false };
            set_shape(app, Shape::CYCLE[digit - 1]);
        }
        Key::Character(c) if modifiers.command() && c.as_str() == "a" => app.automation.selected = (0..count).collect(),
        Key::Character(c) if modifiers.command() && c.as_str() == "c" => {
            let points = points(app);
            app.automation.clipboard = app.automation.selected.iter().filter_map(|&i| points.get(i).copied()).collect();
        }
        Key::Character(c) if modifiers.command() && (c.as_str() == "v" || c.as_str() == "d") => {
            let duplicate = c.as_str() == "d";
            let source: Vec<EnvPoint> = if duplicate {
                let points = points(app);
                app.automation.selected.iter().filter_map(|&i| points.get(i).copied()).collect()
            } else {
                app.automation.clipboard.clone()
            };
            let (Some(first), Some(last)) = (source.iter().map(|p| p.time).min(), source.iter().map(|p| p.time).max()) else {
                return true;
            };
            let step = app.project.grid.step(app.project.signature).unwrap_or(daw_model::STEP_TICKS);
            // Duplicates land one grid step after the selection; pastes after the last point.
            let at = if duplicate { last + step } else { points(app).iter().map(|p| p.time).max().unwrap_or(0) + step };
            let moved: Vec<EnvPoint> = source.iter().map(|p| EnvPoint { time: p.time - first + at, ..*p }).collect();
            let end = moved.iter().map(|p| p.time).max().unwrap_or(at);
            app.checkpoint();
            replace(app, at, end, moved);
            app.edited();
        }
        Key::Named(named @ (Named::ArrowLeft | Named::ArrowRight | Named::ArrowUp | Named::ArrowDown))
            if !app.automation.selected.is_empty() =>
        {
            let time_step = match app.automation.time_snap {
                TimeSnap::Grid(grid) => grid.step(app.project.signature),
                TimeSnap::Points => None,
            }
            .unwrap_or(daw_model::STEP_TICKS / 4) as f64;
            let steps = target(app).map(|t| app.target_steps(t)).unwrap_or(0);
            let value_step = match app.automation.value_snap {
                ValueSnap::Step(step) => step,
                ValueSnap::Discrete if steps > 0 => 1.0 / steps as f32,
                _ => 0.01,
            };
            let scale = if modifiers.shift() { 4.0 } else { 1.0 };
            let (ticks, value) = match named {
                Named::ArrowLeft => (-time_step * f64::from(scale), 0.0),
                Named::ArrowRight => (time_step * f64::from(scale), 0.0),
                Named::ArrowUp => (0.0, value_step * scale),
                _ => (0.0, -value_step * scale),
            };
            let first = app.automation.selected[0];
            begin_drag(app, first);
            update(app, Message::Drag { ticks, value, bypass: true });
            update(app, Message::End);
        }
        _ => return false,
    }
    true
}

pub fn toolbar(app: &App) -> Element<'_, AppMessage> {
    let state = &app.automation;
    let clips: Vec<ClipChoice> = app.project.automation.iter().map(|a| ClipChoice { id: a.id, name: a.name.clone() }).collect();
    let current = clips.iter().find(|c| Some(c.id) == state.clip).cloned();
    let touched: Vec<TargetChoice> = app
        .last_touched
        .iter()
        .map(|&(target, value)| TargetChoice {
            target,
            name: format!("{} = {}", app.target_name(target), app.value_text(target, value)),
        })
        .collect();
    let mut time_snaps: Vec<TimeSnap> = Grid::CHOICES.iter().map(|&g| TimeSnap::Grid(g)).collect();
    time_snaps.push(TimeSnap::Points);
    let bar = app.project.signature.ticks_per_bar();
    let bars = state.clip.and_then(|id| app.project.automation_clip(id)).map(|c| c.length.div_ceil(bar).max(1));
    let mut bar_choices: Vec<u64> = (1..=16).collect();
    if let Some(bars) = bars
        && !bar_choices.contains(&bars)
    {
        bar_choices.push(bars);
    }
    let selected_shape = state.selected.first().and_then(|&i| points(app).get(i)).map(|p| p.shape);
    let mut clip_picker = mouse_area(super::pick_or(clips, current, "clip", |c| Message::Clip(c).into()));
    if let Some(id) = state.clip {
        clip_picker = clip_picker.on_right_press(menu::Message::Open(menu::Item::Automation(id)).into());
    }
    row![
        clip_picker,
        super::pick_or(touched, None, "automate last touched", |t| Message::Bind(t).into()),
        toggle("edit", state.tool == Tool::Edit, Message::Tool(Tool::Edit).into()),
        toggle("draw", state.tool == Tool::Draw, Message::Tool(Tool::Draw).into()),
        toggle("line", state.tool == Tool::Line, Message::Tool(Tool::Line).into()),
        super::labeled("time", pick(time_snaps, Some(state.time_snap), |s| Message::TimeSnap(s).into())),
        super::labeled("value", pick(ValueSnap::CHOICES.to_vec(), Some(state.value_snap), |s| Message::ValueSnap(s).into())),
        super::labeled("shape", super::pick_or(Shape::CYCLE.to_vec(), selected_shape, "-", |s| Message::Shape(s).into())),
        row![
            pick(LfoShape::ALL.to_vec(), Some(state.lfo_shape), |s| Message::LfoShape(s).into()),
            pick(LfoRate::CHOICES.to_vec(), Some(state.lfo_rate), |r| Message::LfoRate(r).into()),
            tool("lfo", Message::Lfo.into()),
        ].spacing(3).align_y(iced::Alignment::Center),
        super::labeled("bars", pick(bar_choices, bars, |b| Message::Bars(b).into())),
    ]
    .spacing(3)
    .align_y(iced::Alignment::Center)
    .wrap()
    .into()
}

pub fn view(app: &App, _focused: bool) -> Element<'_, AppMessage> {
    if app.automation.clip.and_then(|id| app.project.automation_clip(id)).is_none() {
        let hint = "no clip open. Right click a control and choose automate, pick a last touched \
                    parameter above, or turn on bind (Alt+A) and move any parameter.";
        return iced::widget::container(label(hint)).padding(8).into();
    }
    Canvas::new(Editor { app }).width(Length::Fill).height(Length::Fill).into()
}

#[cfg(test)]
mod tests {
    use super::ValueRange;

    #[test]
    fn value_range_zooms_around_the_anchor_and_stays_inside() {
        let zoomed = ValueRange::FULL.zoom(5.0, 0.8);
        assert!(zoomed.high - zoomed.low < 0.6);
        let ratio = (0.8 - zoomed.low) / (zoomed.high - zoomed.low);
        assert!((ratio - 0.8).abs() < 1e-4, "anchor stays at the same height: {ratio}");
        let top = zoomed.scroll(100.0);
        assert!((top.high - 1.0).abs() < 1e-6 && top.low > 0.0);
        assert_eq!(ValueRange::FULL.scroll(3.0), ValueRange::FULL);
        let tight = ValueRange::FULL.zoom(100.0, 0.0);
        assert!((tight.high - tight.low - 0.05).abs() < 1e-6 && tight.low == 0.0);
        let out = tight.zoom(-100.0, 0.0);
        assert_eq!(out, ValueRange::FULL);
    }
}
