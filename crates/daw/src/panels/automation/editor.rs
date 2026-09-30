//! The envelope canvas: drawing the clip's points and turning mouse input
//! into automation messages.

use daw_engine::song::PlayMode;
use daw_model::ClipSource;
use daw_model::automation::{Point as EnvPoint, TimeSnap, ValueSnap, simplify};
use daw_model::time::Ticks;
use iced::keyboard::Modifiers;
use iced::widget::canvas::{self, Frame, Geometry, Path, Stroke};
use iced::{Color, Point, Rectangle, Renderer, Size, Theme, mouse};

use super::{Message, State, Tool, points, snap_context, ticks_per_px};
use crate::app::{App, Message as AppMessage};
use crate::panels::timeline::{self, Clicks, RULER_HEIGHT, Wheel};
use crate::theme;

const AXIS_WIDTH: f32 = 56.0;
const PAD: f32 = 8.0;
const POINT_RADIUS: f32 = 3.5;
const HIT_RADIUS: f32 = 6.0;

pub(super) struct Editor<'a> {
    pub(super) app: &'a App,
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
        let range = self.state().values;
        RULER_HEIGHT + PAD + (range.high - value) / range.span() * self.height(bounds)
    }

    fn value(&self, y: f32, bounds: Rectangle) -> f32 {
        let range = self.state().values;
        range.high - (y - RULER_HEIGHT - PAD) / self.height(bounds) * range.span()
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
                    Tool::Draw if !timeline::box_select_modifier(modifiers) => {
                        state.drag = Some(Drag::Stroke(vec![p]));
                        return redraw();
                    }
                    Tool::Line if !timeline::box_select_modifier(modifiers) => {
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
                if timeline::box_select_modifier(modifiers) {
                    state.drag = Some(Drag::Box { from: p, to: p, additive: modifiers.shift() });
                    return redraw();
                }
                let (time, snapped) = self.snap(tick, value, modifiers.command());
                state.drag = Some(Drag::Points { tick: time as f64, value: snapped, origin: p });
                publish(Message::Add { time: tick, value, bypass: modifiers.command() })
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
                        publish(Message::Drag { ticks: dt, value: dv, bypass: modifiers.command() })
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
                let bypass = state.modifiers.command();
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
                let values = self.state().values;
                match Wheel::from_event(self.state().time, *delta, state.modifiers, p.x - AXIS_WIDTH) {
                    Wheel::Time(view) => publish(Message::View(view)),
                    Wheel::Vertical(steps) => publish(Message::Values(values.scroll(steps))),
                    Wheel::Height(steps) => publish(Message::Values(values.zoom(steps, self.value(p.y, bounds)))),
                    Wheel::Alternate(_) => None,
                }
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
        let mut guide = match editor.value_snap {
            ValueSnap::Step(step) if step >= 0.05 => step,
            ValueSnap::Discrete if (1..=24).contains(&steps) => 1.0 / steps as f32,
            _ => 0.25,
        };
        // Keep a few guides on screen when zoomed in.
        let range = editor.values;
        while range.span() / guide < 4.0 && guide > 0.001 {
            guide /= 2.0;
        }
        let mut v = (range.low / guide).floor() * guide;
        while v <= range.high + 0.0001 {
            if v >= range.low - 0.0001 {
                let y = self.y(v, bounds).round() + 0.5;
                frame.fill_rectangle(Point::new(AXIS_WIDTH, y), Size::new(area.width, 1.0), theme::GRID);
            }
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
                let bypass = state.modifiers.command();
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
        for i in 0..=4 {
            let v = editor.values.low + editor.values.span() * i as f32 / 4.0;
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
                    let (t, v) = self.snap(self.tick(hover.x), self.value(hover.y, bounds), state.modifiers.command());
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
