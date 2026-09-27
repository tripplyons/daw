//! Automation clip editor.
//!
//! Edit tool: click adds a point, drag moves the selection, right click
//! deletes, right or Ctrl drag selects a box, Alt drag (or the square handle)
//! bends a segment, double click cycles the segment shape. Cmd bypasses
//! snapping and Shift locks the drag to one axis. Keys 1-6 set the shape of
//! the selected points, arrows nudge, Delete, Cmd A/C/V/D edit the selection.
//! The draw tool paints a curve and the line tool replaces a range with a ramp.
//! Clicking or dragging in the ruler sets where the clip ends.

use std::collections::HashMap;

use daw_engine::song::PlayMode;
use daw_model::automation::{LfoShape, Point as EnvPoint, Shape, SnapContext, TimeSnap, ValueSnap, lfo, simplify};
use daw_model::time::{Grid, Ticks};
use daw_model::{AutomationId, ClipSource, Target};
use iced::keyboard::{Key, Modifiers, key::Named};
use iced::widget::canvas::{self, Canvas, Frame, Geometry, Path, Stroke};
use iced::widget::row;
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Size, Theme, mouse};

use super::timeline::{self, Clicks, RULER_HEIGHT, TimeView};
use super::{label, pick, tool, toggle};
use crate::app::{App, Message as AppMessage};
use crate::theme;

const AXIS_WIDTH: f32 = 56.0;
const PAD: f32 = 8.0;
const POINT_RADIUS: f32 = 3.5;
const HIT_RADIUS: f32 = 6.0;
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

#[derive(Debug)]
pub struct State {
    pub clip: Option<AutomationId>,
    pub selected: Vec<usize>,
    pub tool: Tool,
    pub time_snap: TimeSnap,
    pub value_snap: ValueSnap,
    pub time: TimeView,
    pub lfo_shape: LfoShape,
    pub lfo_rate: LfoRate,
    /// Points being dragged, the pressed one first.
    originals: Vec<(usize, EnvPoint)>,
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
            lfo_shape: LfoShape::Sine,
            lfo_rate: LfoRate(4),
            originals: Vec::new(),
            clipboard: Vec::new(),
            recording: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    View(TimeView),
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
        Message::Clip(choice) => {
            app.automation.clip = Some(choice.id);
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
            app.edited();
        }
        Message::Begin { index, additive } => {
            app.checkpoint();
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
            let Some(points) = points_mut(app) else { return };
            for (index, original) in originals {
                if let Some(point) = points.get_mut(index) {
                    point.time = (original.time as i64 + dt).max(0) as Ticks;
                    point.value = (original.value + dv).clamp(0.0, 1.0);
                }
            }
            app.edited();
        }
        Message::End => {
            app.automation.originals.clear();
            sort(app);
            app.edited();
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
            app.checkpoint();
            if let Some(point) = points_mut(app).and_then(|p| p.get_mut(index))
                && !point.shape.uses_tension()
            {
                point.shape = Shape::Curve;
                point.tension = 0.0;
            }
            app.edited();
        }
        Message::Tension(index, tension) => {
            if let Some(point) = points_mut(app).and_then(|p| p.get_mut(index)) {
                point.tension = tension.clamp(-1.0, 1.0);
            }
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
            app.checkpoint();
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
        app.checkpoint();
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
        Key::Character(c) if !modifiers.logo() && !modifiers.alt() => {
            let Some(digit) = c.as_str().parse::<usize>().ok().filter(|d| (1..=6).contains(d)) else { return false };
            set_shape(app, Shape::CYCLE[digit - 1]);
        }
        Key::Character(c) if modifiers.logo() && c.as_str() == "a" => app.automation.selected = (0..count).collect(),
        Key::Character(c) if modifiers.logo() && c.as_str() == "c" => {
            let points = points(app);
            app.automation.clipboard = app.automation.selected.iter().filter_map(|&i| points.get(i).copied()).collect();
        }
        Key::Character(c) if modifiers.logo() && (c.as_str() == "v" || c.as_str() == "d") => {
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
            app.checkpoint();
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
    row![
        super::pick_or(clips, current, "clip", |c| Message::Clip(c).into()),
        super::pick_or(touched, None, "automate last touched", |t| Message::Bind(t).into()),
        toggle("edit", state.tool == Tool::Edit, Message::Tool(Tool::Edit).into()),
        toggle("draw", state.tool == Tool::Draw, Message::Tool(Tool::Draw).into()),
        toggle("line", state.tool == Tool::Line, Message::Tool(Tool::Line).into()),
        label("time"),
        pick(time_snaps, Some(state.time_snap), |s| Message::TimeSnap(s).into()),
        label("value"),
        pick(ValueSnap::CHOICES.to_vec(), Some(state.value_snap), |s| Message::ValueSnap(s).into()),
        label("shape"),
        super::pick_or(Shape::CYCLE.to_vec(), selected_shape, "-", |s| Message::Shape(s).into()),
        pick(LfoShape::ALL.to_vec(), Some(state.lfo_shape), |s| Message::LfoShape(s).into()),
        pick(LfoRate::CHOICES.to_vec(), Some(state.lfo_rate), |r| Message::LfoRate(r).into()),
        tool("lfo", Message::Lfo.into()),
        label("bars"),
        pick(bar_choices, bars, |b| Message::Bars(b).into()),
    ]
    .spacing(3)
    .align_y(iced::Alignment::Center)
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

struct Editor<'a> {
    app: &'a App,
}

#[derive(Debug, Clone)]
enum Drag {
    Points { tick: f64, value: f32, origin: Point },
    Tension { index: usize, origin: f32, start: f32, rising: bool },
    Box { from: Point, to: Point, additive: bool },
    Stroke(Vec<Point>),
    Line { from: Point, to: Point },
    /// Moving the clip end in the ruler.
    End,
}

#[derive(Debug, Default)]
pub struct CanvasState {
    drag: Option<Drag>,
    modifiers: Modifiers,
    clicks: Clicks,
    hover: Option<Point>,
}

impl Editor<'_> {
    fn state(&self) -> &State {
        &self.app.automation
    }

    fn height(&self, bounds: Rectangle) -> f32 {
        (bounds.height - RULER_HEIGHT - PAD * 2.0).max(1.0)
    }

    fn y(&self, value: f32, bounds: Rectangle) -> f32 {
        RULER_HEIGHT + PAD + (1.0 - value) * self.height(bounds)
    }

    fn value(&self, y: f32, bounds: Rectangle) -> f32 {
        1.0 - (y - RULER_HEIGHT - PAD) / self.height(bounds)
    }

    fn x(&self, tick: f64) -> f32 {
        AXIS_WIDTH + self.state().time.x(tick)
    }

    fn tick(&self, x: f32) -> f64 {
        self.state().time.tick(x - AXIS_WIDTH)
    }

    fn point_at(&self, p: Point, bounds: Rectangle) -> Option<usize> {
        points(self.app)
            .iter()
            .enumerate()
            .rev()
            .map(|(i, q)| (i, Point::new(self.x(q.time as f64), self.y(q.value, bounds)).distance(p)))
            .filter(|(_, d)| *d <= HIT_RADIUS)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }

    /// Midpoint handle of a curved segment.
    fn handle(&self, index: usize, bounds: Rectangle) -> Option<Point> {
        let points = points(self.app);
        let (a, b) = (points.get(index)?, points.get(index + 1)?);
        if !a.shape.uses_tension() || b.time <= a.time {
            return None;
        }
        let mid = (a.time + b.time) as f64 / 2.0;
        let value = daw_model::automation::Envelope { points: vec![*a, *b] }.value_at(mid)?;
        Some(Point::new(self.x(mid), self.y(value, bounds)))
    }

    fn handle_at(&self, p: Point, bounds: Rectangle) -> Option<usize> {
        (0..points(self.app).len()).find(|&i| self.handle(i, bounds).is_some_and(|h| h.distance(p) <= HIT_RADIUS))
    }

    fn segment_at(&self, tick: f64) -> Option<usize> {
        let points = points(self.app);
        let index = points.iter().rposition(|p| p.time as f64 <= tick)?;
        (index + 1 < points.len()).then_some(index)
    }

    fn snap(&self, tick: f64, value: f32, bypass: bool) -> (Ticks, f32) {
        let tick = tick.max(0.0);
        if bypass {
            return (tick as Ticks, value.clamp(0.0, 1.0));
        }
        let others = points(self.app);
        let context = snap_context(self.app, others, ticks_per_px(self.app));
        (context.snap_time(tick as Ticks), context.snap_value(value))
    }

    fn tension_for(&self, index: usize) -> (f32, bool) {
        let points = points(self.app);
        let tension = points.get(index).map(|p| if p.shape.uses_tension() { p.tension } else { 0.0 }).unwrap_or(0.0);
        let rising = match (points.get(index), points.get(index + 1)) {
            (Some(a), Some(b)) => b.value >= a.value,
            _ => true,
        };
        (tension, rising)
    }

    /// Convert a freehand stroke into envelope points.
    fn stroke_points(&self, stroke: &[Point], bounds: Rectangle, bypass: bool) -> (Ticks, Ticks, Vec<EnvPoint>) {
        let mut samples: Vec<(f64, f32)> =
            stroke.iter().map(|p| (self.tick(p.x).max(0.0), self.value(p.y, bounds).clamp(0.0, 1.0))).collect();
        samples.sort_by(|a, b| a.0.total_cmp(&b.0));
        samples.dedup_by(|a, b| (a.0 - b.0).abs() < 1.0);
        let (Some(first), Some(last)) = (samples.first().copied(), samples.last().copied()) else { return (0, 0, Vec::new()) };
        let value_at = |t: f64| {
            let index = samples.partition_point(|s| s.0 < t);
            match (index.checked_sub(1).map(|i| samples[i]), samples.get(index)) {
                (Some(a), Some(b)) if b.0 > a.0 => a.1 + (b.1 - a.1) * ((t - a.0) / (b.0 - a.0)) as f32,
                (_, Some(b)) => b.1,
                (Some(a), None) => a.1,
                (None, None) => 0.5,
            }
        };
        let grid_step = match self.state().time_snap {
            TimeSnap::Grid(grid) if !bypass => grid.step(self.app.project.signature),
            _ => None,
        };
        // With a time grid, resample the stroke on grid lines.
        let resampled: Vec<(f64, f32)> = match grid_step {
            Some(step) => {
                let start = (first.0 / step as f64).round() as u64 * step;
                let end = (last.0 / step as f64).round() as u64 * step;
                (start..=end).step_by(step as usize).map(|t| (t as f64, value_at(t as f64))).collect()
            }
            None => samples,
        };
        let scaled: Vec<(f64, f64)> = resampled
            .iter()
            .map(|&(t, v)| (f64::from(self.x(t)), f64::from(self.y(v, bounds))))
            .collect();
        let keep = simplify(&scaled, 1.5);
        let others = points(self.app);
        let context = snap_context(self.app, others, ticks_per_px(self.app));
        let new: Vec<EnvPoint> = keep
            .into_iter()
            .map(|i| {
                let (t, v) = resampled[i];
                let v = if bypass { v } else { context.snap_value(v) };
                EnvPoint::new(t.round() as Ticks, v)
            })
            .collect();
        let start = new.first().map(|p| p.time).unwrap_or(0);
        let end = new.last().map(|p| p.time).unwrap_or(0);
        (start, end, new)
    }

    fn draw_envelope(&self, frame: &mut Frame<Renderer>, envelope: &daw_model::automation::Envelope, length: f64, bounds: Rectangle, color: Color, width: f32) {
        let right = bounds.width;
        let path = Path::new(|b| {
            let mut x = AXIS_WIDTH;
            let mut first = true;
            while x <= right {
                let tick = self.tick(x);
                let local = if tick < length || length <= 0.0 { tick } else { tick % length };
                let Some(value) = envelope.value_at(local) else { return };
                let point = Point::new(x, self.y(value, bounds));
                if first {
                    b.move_to(point);
                    first = false;
                } else {
                    b.line_to(point);
                }
                x += 1.0;
            }
        });
        frame.stroke(&path, Stroke::default().with_color(color).with_width(width));
    }
}

impl canvas::Program<AppMessage> for Editor<'_> {
    type State = CanvasState;

    fn update(&self, state: &mut CanvasState, event: &canvas::Event, bounds: Rectangle, cursor: mouse::Cursor) -> Option<canvas::Action<AppMessage>> {
        use iced::keyboard::Event as KeyEvent;
        use iced::mouse::{Button, Event as MouseEvent};
        let publish = |m: Message| Some(canvas::Action::publish(AppMessage::from(m)).and_capture());
        let redraw = || Some(canvas::Action::request_redraw().and_capture());
        match event {
            canvas::Event::Keyboard(KeyEvent::ModifiersChanged(m)) => {
                state.modifiers = *m;
                Some(canvas::Action::request_redraw())
            }
            canvas::Event::Mouse(MouseEvent::ButtonPressed(button)) => {
                let p = cursor.position_in(bounds)?;
                if p.y < RULER_HEIGHT && p.x >= AXIS_WIDTH && *button == Button::Left {
                    state.drag = Some(Drag::End);
                    return publish(Message::ClipEnd(self.tick(p.x)));
                }
                if p.y < RULER_HEIGHT || p.x < AXIS_WIDTH {
                    return None;
                }
                let tick = self.tick(p.x);
                let value = self.value(p.y, bounds);
                let modifiers = state.modifiers;
                let double = state.clicks.press(p);
                let tool = self.state().tool;
                if *button == Button::Right {
                    if let Some(index) = self.point_at(p, bounds) {
                        return publish(Message::Delete(index));
                    }
                    state.drag = Some(Drag::Box { from: p, to: p, additive: modifiers.shift() });
                    return redraw();
                }
                if *button != Button::Left {
                    return None;
                }
                match tool {
                    Tool::Draw if !modifiers.control() => {
                        state.drag = Some(Drag::Stroke(vec![p]));
                        return redraw();
                    }
                    Tool::Line if !modifiers.control() => {
                        state.drag = Some(Drag::Line { from: p, to: p });
                        return redraw();
                    }
                    _ => {}
                }
                if let Some(index) = self.handle_at(p, bounds) {
                    let (origin, rising) = self.tension_for(index);
                    state.drag = Some(Drag::Tension { index, origin, start: p.y, rising });
                    return publish(Message::BeginTension(index));
                }
                if let Some(index) = self.point_at(p, bounds) {
                    if double {
                        return publish(Message::CycleShape(index));
                    }
                    let point = points(self.app)[index];
                    state.drag = Some(Drag::Points { tick: point.time as f64, value: point.value, origin: p });
                    return publish(Message::Begin { index, additive: modifiers.shift() });
                }
                if modifiers.alt()
                    && let Some(index) = self.segment_at(tick)
                {
                    let (origin, rising) = self.tension_for(index);
                    state.drag = Some(Drag::Tension { index, origin, start: p.y, rising });
                    return publish(Message::BeginTension(index));
                }
                if double && let Some(index) = self.segment_at(tick) {
                    return publish(Message::CycleShape(index));
                }
                if modifiers.control() {
                    state.drag = Some(Drag::Box { from: p, to: p, additive: modifiers.shift() });
                    return redraw();
                }
                let (time, snapped) = self.snap(tick, value, modifiers.logo());
                state.drag = Some(Drag::Points { tick: time as f64, value: snapped, origin: p });
                publish(Message::Add { time: tick, value, bypass: modifiers.logo() })
            }
            canvas::Event::Mouse(MouseEvent::CursorMoved { .. }) => {
                let p = cursor.position_from(bounds.position())?;
                state.hover = cursor.position_in(bounds);
                let modifiers = state.modifiers;
                let Some(drag) = state.drag.as_mut() else { return Some(canvas::Action::request_redraw()) };
                match drag {
                    Drag::Points { tick, value, origin } => {
                        let (mut dt, mut dv) = (self.tick(p.x) - *tick, self.value(p.y, bounds) - *value);
                        if modifiers.shift() {
                            // Lock to the axis with the larger movement.
                            if (p.x - origin.x).abs() >= (p.y - origin.y).abs() {
                                dv = 0.0;
                            } else {
                                dt = 0.0;
                            }
                        }
                        publish(Message::Drag { ticks: dt, value: dv, bypass: modifiers.logo() })
                    }
                    Drag::Tension { index, origin, start, rising } => {
                        let sign = if *rising { 1.0 } else { -1.0 };
                        let tension = *origin + sign * (*start - p.y) / 120.0;
                        publish(Message::Tension(*index, tension))
                    }
                    Drag::Box { to, .. } | Drag::Line { to, .. } => {
                        *to = p;
                        redraw()
                    }
                    Drag::Stroke(stroke) => {
                        stroke.push(p);
                        redraw()
                    }
                    Drag::End => publish(Message::ClipEnd(self.tick(p.x))),
                }
            }
            canvas::Event::Mouse(MouseEvent::ButtonReleased(_)) => {
                let bypass = state.modifiers.logo();
                match state.drag.take()? {
                    Drag::Points { .. } => publish(Message::End),
                    Drag::Tension { .. } => publish(Message::End),
                    Drag::End => Some(canvas::Action::publish(AppMessage::EndEdit).and_capture()),
                    Drag::Box { from, to, additive } => {
                        let (t0, t1) = (self.tick(from.x.min(to.x)), self.tick(from.x.max(to.x)));
                        let (v0, v1) = (self.value(from.y.max(to.y), bounds), self.value(from.y.min(to.y), bounds));
                        publish(Message::BoxSelect { t0, t1, v0, v1, additive })
                    }
                    Drag::Stroke(stroke) => {
                        let (start, end, points) = self.stroke_points(&stroke, bounds, bypass);
                        if points.len() < 2 {
                            return redraw();
                        }
                        publish(Message::Replace { start, end, points })
                    }
                    Drag::Line { from, to } => {
                        let a = self.snap(self.tick(from.x), self.value(from.y, bounds), bypass);
                        let b = self.snap(self.tick(to.x), self.value(to.y, bounds), bypass);
                        if a.0 == b.0 {
                            return redraw();
                        }
                        let (a, b) = if a.0 < b.0 { (a, b) } else { (b, a) };
                        let points = vec![EnvPoint::new(a.0, a.1), EnvPoint::new(b.0, b.1)];
                        publish(Message::Replace { start: a.0, end: b.0, points })
                    }
                }
            }
            canvas::Event::Mouse(MouseEvent::WheelScrolled { delta }) => {
                let p = cursor.position_in(bounds)?;
                let view = self.state().time;
                let zoom = state.modifiers.logo() || state.modifiers.control();
                if let Some(view) = view.wheel(*delta, zoom, p.x - AXIS_WIDTH) {
                    return publish(Message::View(view));
                }
                let (_, lines) = super::wheel_lines(*delta);
                publish(Message::View(view.scroll_by_lines(lines)))
            }
            _ => None,
        }
    }

    fn draw(&self, state: &CanvasState, renderer: &Renderer, _theme: &Theme, bounds: Rectangle, _cursor: mouse::Cursor) -> Vec<Geometry> {
        let app = self.app;
        let editor = self.state();
        let mut frame = Frame::new(renderer, bounds.size());
        let size = bounds.size();
        let Some(clip) = editor.clip.and_then(|id| app.project.automation_clip(id)) else { return vec![] };
        let target = clip.target;
        let area = Size::new(size.width - AXIS_WIDTH, size.height - RULER_HEIGHT);

        // Value guides: snap steps when coarse enough, otherwise quarters.
        let steps = app.target_steps(target);
        let guide = match editor.value_snap {
            ValueSnap::Step(step) if step >= 0.05 => step,
            ValueSnap::Discrete if (1..=24).contains(&steps) => 1.0 / steps as f32,
            _ => 0.25,
        };
        let mut v = 0.0;
        while v <= 1.0001 {
            let y = self.y(v, bounds).round() + 0.5;
            frame.fill_rectangle(Point::new(AXIS_WIDTH, y), Size::new(area.width, 1.0), theme::GRID);
            v += guide;
        }
        let grid = match editor.time_snap {
            TimeSnap::Grid(grid) => grid,
            TimeSnap::Points => app.project.grid,
        };
        timeline::draw_grid(&mut frame, editor.time, AXIS_WIDTH, RULER_HEIGHT, area, app.project.signature, grid);

        // Past the clip end the envelope repeats; shade it.
        let end_x = self.x(clip.length as f64).max(AXIS_WIDTH);
        if end_x < size.width {
            frame.fill_rectangle(Point::new(end_x, RULER_HEIGHT), Size::new(size.width - end_x, area.height), Color { a: 0.4, ..Color::BLACK });
        }

        // Other clips on the same target, for reference.
        for other in app.project.automation.iter().filter(|a| a.target == target && a.id != clip.id) {
            self.draw_envelope(&mut frame, &other.envelope, other.length as f64, bounds, theme::FILL_DIM, 1.0);
        }
        self.draw_envelope(&mut frame, &clip.envelope, clip.length as f64, bounds, theme::FILL, 1.5);

        // Handles on curved segments.
        for index in 0..clip.envelope.points.len() {
            if let Some(h) = self.handle(index, bounds) {
                frame.stroke_rectangle(Point::new(h.x - 3.0, h.y - 3.0), Size::new(6.0, 6.0), Stroke::default().with_color(theme::TEXT_DIM).with_width(1.0));
            }
        }
        for (index, point) in clip.envelope.points.iter().enumerate() {
            let center = Point::new(self.x(point.time as f64), self.y(point.value, bounds));
            if center.x < AXIS_WIDTH - POINT_RADIUS {
                continue;
            }
            let selected = editor.selected.contains(&index);
            let color = if selected { theme::BRIGHT } else { theme::TEXT };
            let radius = if selected { POINT_RADIUS + 1.0 } else { POINT_RADIUS };
            frame.fill(&Path::circle(center, radius), color);
        }

        match &state.drag {
            Some(Drag::Box { from, to, .. }) => {
                let top_left = Point::new(from.x.min(to.x), from.y.min(to.y));
                let box_size = Size::new((from.x - to.x).abs(), (from.y - to.y).abs());
                frame.fill_rectangle(top_left, box_size, Color { a: 0.08, ..theme::BRIGHT });
                frame.stroke_rectangle(top_left, box_size, Stroke::default().with_color(theme::TEXT_DIM).with_width(1.0));
            }
            Some(Drag::Stroke(stroke)) if stroke.len() > 1 => {
                let path = Path::new(|b| {
                    b.move_to(stroke[0]);
                    for p in &stroke[1..] {
                        b.line_to(*p);
                    }
                });
                frame.stroke(&path, Stroke::default().with_color(theme::BRIGHT).with_width(1.0));
            }
            Some(Drag::Line { from, to }) => {
                let bypass = state.modifiers.logo();
                let a = self.snap(self.tick(from.x), self.value(from.y, bounds), bypass);
                let b = self.snap(self.tick(to.x), self.value(to.y, bounds), bypass);
                let pa = Point::new(self.x(a.0 as f64), self.y(a.1, bounds));
                let pb = Point::new(self.x(b.0 as f64), self.y(b.1, bounds));
                frame.stroke(&Path::line(pa, pb), Stroke::default().with_color(theme::BRIGHT).with_width(1.0));
            }
            _ => {}
        }

        // Playhead, when a playlist instance of this clip is playing.
        if app.mode == PlayMode::Song {
            let position = app.position;
            let instance = app.project.playlist.clips.iter().find(|c| {
                c.source == ClipSource::Automation(clip.id) && (c.start as f64) <= position && position < c.end() as f64
            });
            if let Some(instance) = instance {
                let local = (position - instance.start as f64 + instance.offset as f64) % clip.length.max(1) as f64;
                timeline::draw_playhead(&mut frame, editor.time, AXIS_WIDTH, size.height, local);
            }
        }

        // Value axis.
        frame.fill_rectangle(Point::new(0.0, RULER_HEIGHT), Size::new(AXIS_WIDTH, area.height), theme::HEADER);
        for v in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let y = self.y(v, bounds);
            let text = app.value_text(target, v);
            let text = timeline::fit(&text, AXIS_WIDTH - 4.0);
            timeline::label(&mut frame, text, Point::new(3.0, (y - 7.0).clamp(RULER_HEIGHT, size.height - 14.0)), theme::TEXT_DIM);
        }

        timeline::draw_ruler(&mut frame, editor.time, AXIS_WIDTH, area.width, app.project.signature);
        // Clip end handle in the ruler; drag it to change the length.
        let end_x = self.x(clip.length as f64).round();
        if end_x >= AXIS_WIDTH && end_x < size.width {
            frame.fill_rectangle(Point::new(end_x - 1.0, 0.0), Size::new(2.0, RULER_HEIGHT), theme::TEXT_DIM);
        }
        frame.fill_rectangle(Point::ORIGIN, Size::new(AXIS_WIDTH, RULER_HEIGHT), theme::HEADER);

        // Readout of the dragged or hovered point, else the cursor position.
        let readout = match (&state.drag, state.hover) {
            (Some(Drag::Points { .. } | Drag::Tension { .. }), _) => {
                editor.selected.first().and_then(|&i| clip.envelope.points.get(i)).map(|p| {
                    let tension = if p.shape.uses_tension() { format!("  tension {:+.2}", p.tension) } else { String::new() };
                    format!("{}  {}  {}{tension}", timeline::position_text(p.time as f64, app.project.signature), app.value_text(target, p.value), p.shape.name())
                })
            }
            (_, Some(hover)) if hover.x >= AXIS_WIDTH && hover.y >= RULER_HEIGHT => match self.point_at(hover, bounds) {
                Some(i) => {
                    let p = clip.envelope.points[i];
                    Some(format!("{}  {}  {}", timeline::position_text(p.time as f64, app.project.signature), app.value_text(target, p.value), p.shape.name()))
                }
                None => {
                    let (t, v) = self.snap(self.tick(hover.x), self.value(hover.y, bounds), state.modifiers.logo());
                    Some(format!("{}  {}", timeline::position_text(t as f64, app.project.signature), app.value_text(target, v)))
                }
            },
            _ => None,
        };
        if let Some(readout) = readout {
            let width = readout.len() as f32 * 6.5 + 8.0;
            let x = (size.width - width).max(AXIS_WIDTH);
            frame.fill_rectangle(Point::new(x, 0.0), Size::new(width, RULER_HEIGHT), theme::HEADER);
            timeline::label(&mut frame, readout, Point::new(x + 4.0, 2.0), theme::BRIGHT);
        }
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(&self, state: &CanvasState, bounds: Rectangle, cursor: mouse::Cursor) -> mouse::Interaction {
        let Some(p) = cursor.position_in(bounds) else { return mouse::Interaction::default() };
        if matches!(state.drag, Some(Drag::End)) || (p.x >= AXIS_WIDTH && p.y < RULER_HEIGHT) {
            return mouse::Interaction::ResizingHorizontally;
        }
        if p.x < AXIS_WIDTH || p.y < RULER_HEIGHT {
            return mouse::Interaction::default();
        }
        match &state.drag {
            Some(Drag::Points { .. }) => return mouse::Interaction::Grabbing,
            Some(Drag::Tension { .. }) => return mouse::Interaction::ResizingVertically,
            Some(_) => return mouse::Interaction::Crosshair,
            None => {}
        }
        if self.handle_at(p, bounds).is_some() || state.modifiers.alt() {
            return mouse::Interaction::ResizingVertically;
        }
        if self.point_at(p, bounds).is_some() {
            return mouse::Interaction::Grab;
        }
        mouse::Interaction::Crosshair
    }
}
