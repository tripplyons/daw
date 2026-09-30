//! Shared time axis for the piano roll, playlist, and automation editor.

use std::time::{Duration, Instant};

use daw_model::time::{Grid, TICKS_PER_BEAT, Ticks, TimeSignature};
use iced::widget::canvas::{Frame, Path, Stroke, Text};
use iced::keyboard::Modifiers;
use iced::{Color, Point, Renderer, Size, mouse};

use crate::theme;

pub const RULER_HEIGHT: f32 = 16.0;
const MIN_SCALE: f32 = 2.0;
const MAX_SCALE: f32 = 800.0;

/// Horizontal view: pixels per beat and the first visible tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeView {
    pub scale: f32,
    pub scroll: f64,
}

impl TimeView {
    pub const fn new(scale: f32) -> Self {
        Self { scale, scroll: 0.0 }
    }

    fn px_per_tick(self) -> f64 {
        f64::from(self.scale) / f64::from(TICKS_PER_BEAT)
    }

    /// X of a tick, relative to the left edge of the time area.
    pub fn x(self, tick: f64) -> f32 {
        ((tick - self.scroll) * self.px_per_tick()) as f32
    }

    pub fn tick(self, x: f32) -> f64 {
        f64::from(x) / self.px_per_tick() + self.scroll
    }

    pub fn ticks(self, width: f32) -> f64 {
        f64::from(width) / self.px_per_tick()
    }

    /// Zoom time by `lines` wheel steps, keeping the tick under `x` in place.
    pub fn zoom(self, lines: f32, x: f32) -> Self {
        let anchor = self.tick(x);
        let scale = (self.scale * 1.15f32.powf(lines)).clamp(MIN_SCALE, MAX_SCALE);
        let view = TimeView { scale, scroll: 0.0 };
        let scroll = (anchor - f64::from(x) / view.px_per_tick()).max(0.0);
        TimeView { scale, scroll }
    }

    pub fn scroll_by_lines(self, lines: f32) -> Self {
        let scroll = (self.scroll - f64::from(lines) * 40.0 / self.px_per_tick()).max(0.0);
        TimeView { scroll, ..self }
    }
}

/// What a wheel event does in a timeline panel. The scheme is shared by the
/// piano roll, playlist, and automation editor:
///
/// - scroll: up and down
/// - Shift+scroll, or a sideways trackpad swipe: sideways
/// - Cmd+scroll (Ctrl+scroll on Linux): zoom time around the cursor
/// - Alt+scroll: zoom height around the cursor
/// - Alt+Shift+scroll: panel specific (note velocity in the piano roll)
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Wheel {
    Time(TimeView),
    /// Wheel steps up (positive) or down.
    Vertical(f32),
    /// Wheel steps of height zoom; positive zooms in.
    Height(f32),
    Alternate(f32),
}

impl Wheel {
    /// `x` is relative to the left edge of the time area.
    pub fn from_event(view: TimeView, delta: mouse::ScrollDelta, modifiers: Modifiers, x: f32) -> Wheel {
        let (dx, dy) = super::wheel_lines(delta);
        // macOS turns Shift+wheel into sideways steps, so take whichever axis moved.
        let steps = if dx.abs() > dy.abs() { dx } else { dy };
        if modifiers.command() {
            return Wheel::Time(view.zoom(steps, x));
        }
        match (modifiers.alt(), modifiers.shift()) {
            (true, true) => Wheel::Alternate(steps),
            (true, false) => Wheel::Height(steps),
            (false, true) => Wheel::Time(view.scroll_by_lines(steps)),
            (false, false) if dx.abs() > dy.abs() => Wheel::Time(view.scroll_by_lines(dx)),
            (false, false) => Wheel::Vertical(dy),
        }
    }
}

/// Whether a left drag selects a box. Ctrl does on macOS; on Linux Ctrl is
/// the command modifier, so only right drag selects there.
pub fn box_select_modifier(modifiers: Modifiers) -> bool {
    modifiers.control() && !modifiers.command()
}

/// Scale a row height by `steps` wheel steps of zoom, within `min..=max`.
pub fn zoom_height(height: f32, steps: f32, min: f32, max: f32) -> f32 {
    (height * 1.15f32.powf(steps)).clamp(min, max)
}

fn line(frame: &mut Frame<Renderer>, from: Point, to: Point, color: Color) {
    frame.stroke(&Path::line(from, to), Stroke::default().with_color(color).with_width(1.0));
}

/// Vertical grid lines for bars, beats, and the snap grid, skipping any level
/// that would be denser than a few pixels.
pub fn draw_grid(frame: &mut Frame<Renderer>, view: TimeView, left: f32, top: f32, size: Size, signature: TimeSignature, grid: Grid) {
    let bar = signature.ticks_per_bar();
    let beat = Ticks::from(TICKS_PER_BEAT) * 4 / Ticks::from(signature.denominator);
    let step = grid.step(signature).unwrap_or(beat);
    let finest = [step, beat, bar].into_iter().find(|&s| view.x(s as f64) - view.x(0.0) >= 5.0).unwrap_or(bar);
    let first = (view.scroll as Ticks / finest) * finest;
    let last = view.tick(size.width) as Ticks;
    let mut tick = first;
    while tick <= last {
        let x = left + view.x(tick as f64).round() + 0.5;
        let color = if tick.is_multiple_of(bar) {
            theme::GRID_STRONG
        } else if tick.is_multiple_of(beat) {
            theme::LINE
        } else {
            theme::GRID
        };
        if x >= left {
            line(frame, Point::new(x, top), Point::new(x, top + size.height), color);
        }
        tick += finest;
    }
}

/// Bar numbers along the top of a time area.
pub fn draw_ruler(frame: &mut Frame<Renderer>, view: TimeView, left: f32, width: f32, signature: TimeSignature) {
    frame.fill_rectangle(Point::new(left, 0.0), Size::new(width, RULER_HEIGHT), theme::HEADER);
    let bar = signature.ticks_per_bar();
    let bar_px = view.x(bar as f64) - view.x(0.0);
    let every = [1, 2, 4, 8, 16, 32, 64].into_iter().find(|&n| bar_px * n as f32 >= 28.0).unwrap_or(128);
    let first = view.scroll as Ticks / bar;
    let last = view.tick(width) as Ticks / bar + 1;
    for index in first..=last {
        if index % every != 0 {
            continue;
        }
        let x = left + view.x((index * bar) as f64);
        if x < left {
            continue;
        }
        line(frame, Point::new(x + 0.5, RULER_HEIGHT - 5.0), Point::new(x + 0.5, RULER_HEIGHT), theme::TEXT_FAINT);
        frame.fill_text(Text {
            content: format!("{}", index + 1),
            position: Point::new(x + 3.0, 2.0),
            color: theme::TEXT_DIM,
            size: theme::SMALL.into(),
            ..Text::default()
        });
    }
}

pub fn draw_playhead(frame: &mut Frame<Renderer>, view: TimeView, left: f32, height: f32, tick: f64) {
    let x = left + view.x(tick).round() + 0.5;
    if x >= left {
        line(frame, Point::new(x, 0.0), Point::new(x, height), theme::BRIGHT);
    }
}

/// Where play starts: a triangle in the ruler and a faint line below it.
pub fn draw_start_marker(frame: &mut Frame<Renderer>, view: TimeView, left: f32, height: f32, tick: f64) {
    let x = left + view.x(tick).round();
    if x < left {
        return;
    }
    let marker = Path::new(|b| {
        b.move_to(Point::new(x - 5.0, 0.0));
        b.line_to(Point::new(x + 6.0, 0.0));
        b.line_to(Point::new(x + 0.5, 7.0));
        b.close();
    });
    frame.fill(&marker, theme::TEXT);
    frame.fill_rectangle(Point::new(x, RULER_HEIGHT), Size::new(1.0, height - RULER_HEIGHT), theme::TEXT_FAINT);
}

/// Text helper for canvases.
pub fn label(frame: &mut Frame<Renderer>, content: impl Into<String>, position: Point, color: Color) {
    frame.fill_text(Text { content: content.into(), position, color, size: theme::SMALL.into(), ..Text::default() });
}

/// Cut text to about `width` pixels, since canvas text is not clipped.
pub fn fit(text: &str, width: f32) -> String {
    let chars = (width / 6.2).floor().max(0.0) as usize;
    text.chars().take(chars).collect()
}

/// Bar.beat.tick position text, with bars and beats counted from 1.
pub fn position_text(tick: f64, signature: TimeSignature) -> String {
    let beat_ticks = f64::from(TICKS_PER_BEAT) * 4.0 / f64::from(signature.denominator);
    let beats = tick / beat_ticks;
    let per_bar = f64::from(signature.numerator);
    let sub = ((beats.fract()) * 100.0).floor() as u32;
    format!("{}.{}.{sub:02}", (beats / per_bar).floor() as u64 + 1, (beats % per_bar).floor() as u64 + 1)
}

/// Snap a tick delta to the grid step so dragged items keep their offset
/// from the grid.
pub fn snap_delta(delta: f64, grid: Grid, signature: TimeSignature, bypass: bool) -> i64 {
    match grid.step(signature) {
        Some(step) if !bypass => (delta / step as f64).round() as i64 * step as i64,
        _ => delta.round() as i64,
    }
}

/// The start marker position for a ruler click at `tick`: snapped down to
/// the grid, never before the song start.
pub fn marker_tick(tick: f64, grid: Grid, signature: TimeSignature) -> Ticks {
    grid.snap_floor(tick.max(0.0) as Ticks, signature)
}

/// Detects double clicks inside a canvas.
#[derive(Debug, Default)]
pub struct Clicks {
    last: Option<(Instant, Point)>,
}

impl Clicks {
    pub fn press(&mut self, position: Point) -> bool {
        let now = Instant::now();
        let double = self
            .last
            .is_some_and(|(time, at)| now - time < Duration::from_millis(350) && at.distance(position) < 5.0);
        self.last = if double { None } else { Some((now, position)) };
        double
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wheel(dx: f32, dy: f32, modifiers: Modifiers) -> Wheel {
        Wheel::from_event(TimeView::new(64.0), mouse::ScrollDelta::Lines { x: dx, y: dy }, modifiers, 100.0)
    }

    #[test]
    fn modifiers_pick_scroll_or_zoom_axis() {
        let view = TimeView::new(64.0);
        assert_eq!(wheel(0.0, 1.0, Modifiers::empty()), Wheel::Vertical(1.0));
        assert_eq!(wheel(-2.0, 0.0, Modifiers::empty()), Wheel::Time(view.scroll_by_lines(-2.0)));
        // macOS reports Shift+wheel as sideways steps; either axis scrolls time.
        assert_eq!(wheel(-1.0, 0.0, Modifiers::SHIFT), Wheel::Time(view.scroll_by_lines(-1.0)));
        assert_eq!(wheel(0.0, -1.0, Modifiers::SHIFT), Wheel::Time(view.scroll_by_lines(-1.0)));
        assert_eq!(wheel(0.0, 1.0, Modifiers::ALT), Wheel::Height(1.0));
        assert_eq!(wheel(1.0, 0.0, Modifiers::ALT | Modifiers::SHIFT), Wheel::Alternate(1.0));
        let Wheel::Time(zoomed) = wheel(0.0, 1.0, Modifiers::COMMAND) else { panic!("command zooms time") };
        assert!(zoomed.scale > view.scale);
        // The other of Cmd and Ctrl does not zoom.
        let other = if cfg!(target_os = "macos") { Modifiers::CTRL } else { Modifiers::LOGO };
        assert_eq!(wheel(0.0, 1.0, other), Wheel::Vertical(1.0));
    }

    #[test]
    fn time_zoom_keeps_the_tick_under_the_cursor() {
        let view = TimeView { scale: 64.0, scroll: 960.0 };
        let before = view.tick(200.0);
        let after = view.zoom(3.0, 200.0).tick(200.0);
        assert!((before - after).abs() < 1.0, "{before} {after}");
    }
}

