//! The arrangement canvas: drawing tracks and clips, and turning mouse
//! input into playlist messages.

use daw_engine::input::Take;
use daw_engine::song::PlayMode;
use daw_model::tempo::TempoMap;
use daw_model::time::Ticks;
use daw_model::{Clip, ClipId, ClipSource, Source};
use iced::keyboard::Modifiers;
use iced::widget::canvas::{self, Frame, Geometry, Path, Stroke};
use iced::{Color, Point, Rectangle, Renderer, Size, Theme, mouse};

use super::{Edge, Message, State, loop_tick, source_name};
use crate::app::{App, Message as AppMessage};
use crate::menu;
use crate::panels::timeline::{self, Clicks, RULER_HEIGHT, Wheel};
use crate::processing::PEAK_FRAMES;
use crate::theme;

const HEADER_WIDTH: f32 = 84.0;
const EDGE: f32 = 5.0;
/// Left part of a track header that toggles mute.
const MUTE_WIDTH: f32 = 14.0;

#[cfg(test)]
mod tests {
    use super::*;
    use iced::widget::canvas::Program;

    #[test]
    fn right_click_on_a_clip_opens_its_menu_without_removing_it() {
        let mut app = App::boot().0;
        let id = app.project.add_clip(0, 0, ClipSource::Pattern(app.selected_pattern));
        let arrangement = Arrangement { tempo: app.project.tempo_map(), app: &app };
        let event = canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right));
        let cursor = mouse::Cursor::Available(Point::new(HEADER_WIDTH + 20.0, RULER_HEIGHT + 10.0));
        let action = arrangement.update(&mut CanvasState::default(), &event, Rectangle::with_size(Size::new(800.0, 500.0)), cursor).unwrap();
        let (message, _, status) = action.into_inner();
        assert!(matches!(message, Some(AppMessage::Menu(menu::Message::Open(menu::Item::Clip(clicked)))) if clicked == id));
        assert_eq!(status, iced::event::Status::Captured);
        assert_eq!(app.project.playlist.clips.len(), 1);
    }
}

pub(super) struct Arrangement<'a> {
    pub(super) app: &'a App,
    pub(super) tempo: TempoMap,
}

#[derive(Debug, Clone, Copy)]
enum Drag {
    Clips { tick: f64, track: i32 },
    Box { from: Point, to: Point, additive: bool },
    /// Moving the start marker in the ruler; it is set on release.
    Seek { tick: f64 },
    Loop { from: f64, to: f64 },
}

#[derive(Debug, Default)]
pub struct CanvasState {
    drag: Option<Drag>,
    modifiers: Modifiers,
    clicks: Clicks,
}

impl Arrangement<'_> {
    fn state(&self) -> &State {
        &self.app.playlist
    }

    fn track_at(&self, y: f32) -> i32 {
        self.state().top_track as i32 + ((y - RULER_HEIGHT) / self.state().track_height).floor() as i32
    }

    fn track_y(&self, track: usize) -> f32 {
        RULER_HEIGHT + (track as f32 - self.state().top_track as f32) * self.state().track_height
    }

    fn tick_at(&self, x: f32) -> f64 {
        self.state().time.tick(x - HEADER_WIDTH)
    }

    fn marker_tick(&self, tick: f64) -> Ticks {
        timeline::marker_tick(tick, self.app.project.grid, self.app.project.signature)
    }

    fn clip_rect(&self, clip: &Clip) -> Rectangle {
        let view = self.state().time;
        let x = HEADER_WIDTH + view.x(clip.start as f64);
        let width = (view.x(clip.end() as f64) - view.x(clip.start as f64)).max(3.0);
        Rectangle { x, y: self.track_y(clip.track), width, height: self.state().track_height }
    }

    /// The clip under `p`, and the edge when `p` is on one.
    fn hit(&self, p: Point) -> Option<(ClipId, Option<Edge>)> {
        self.app.project.playlist.clips.iter().rev().find_map(|c| {
            let r = self.clip_rect(c);
            let edge = match p.x {
                _ if r.width <= EDGE * 3.0 => None,
                x if x > r.x + r.width - EDGE => Some(Edge::End),
                x if x < r.x + EDGE => Some(Edge::Start),
                _ => None,
            };
            r.contains(p).then_some((c.id, edge))
        })
    }

    fn draw_clip(&self, frame: &mut Frame<Renderer>, clip: &Clip, selected: bool) {
        let app = self.app;
        let r = self.clip_rect(clip);
        let (background, content, text) = if clip.muted {
            (if selected { theme::CONTROL_HOVER } else { theme::BG }, theme::FILL_DIM, theme::TEXT_DIM)
        } else if selected {
            (theme::SELECTED, theme::FILL_DIM, theme::BG)
        } else {
            (theme::CONTROL_ACTIVE, theme::FILL, theme::TEXT)
        };
        let body = Rectangle { x: r.x, y: r.y + 1.0, width: r.width - 1.0, height: r.height - 2.0 };
        frame.fill_rectangle(body.position(), body.size(), background);
        let inner = Rectangle { y: body.y + 13.0, height: (body.height - 15.0).max(1.0), ..body };
        let view = self.state().time;
        match clip.source {
            ClipSource::Pattern(id) => {
                let Some(pattern) = app.project.pattern(id) else { return };
                let keys: Vec<u8> = pattern.lanes.iter().flat_map(|l| l.notes.iter().map(|n| n.key)).collect();
                let (low, high) = (keys.iter().min().copied().unwrap_or(60), keys.iter().max().copied().unwrap_or(60));
                let span = f32::from(high - low + 1);
                let length = pattern.length.max(1);
                for lane in &pattern.lanes {
                    for note in &lane.notes {
                        // Repeat the pattern across the clip.
                        let mut repeat = 0;
                        loop {
                            let start = clip.start as i64 + (repeat * length + note.start) as i64 - clip.offset as i64;
                            if start >= clip.end() as i64 {
                                break;
                            }
                            repeat += 1;
                            if start < clip.start as i64 {
                                continue;
                            }
                            let x = HEADER_WIDTH + view.x(start as f64);
                            let end = (start + note.length as i64).min(clip.end() as i64);
                            let width = (HEADER_WIDTH + view.x(end as f64) - x).max(1.0);
                            let y = inner.y + inner.height * (1.0 - (f32::from(note.key - low) + 1.0) / span);
                            let height = (inner.height / span).clamp(1.0, 3.0);
                            frame.fill_rectangle(Point::new(x, y), Size::new(width, height), content);
                        }
                    }
                }
            }
            ClipSource::Automation(id) => {
                let Some(automation) = app.project.automation_clip(id) else { return };
                let length = automation.length.max(1) as f64;
                let right = inner.x + inner.width;
                let path = Path::new(|b| {
                    let mut x = inner.x;
                    let mut first = true;
                    loop {
                        let x_end = x.min(right);
                        // Clamp so rounding at the clip edges cannot land a
                        // sample outside the clip and wrap it to the other end.
                        let within = (view.tick(x_end - HEADER_WIDTH) - clip.start as f64).clamp(0.0, clip.length as f64);
                        let tick = loop_tick(within + clip.offset as f64, length);
                        let value = automation.envelope.value_at(tick).unwrap_or(0.5);
                        let point = Point::new(x_end, inner.y + inner.height * (1.0 - value));
                        if first {
                            b.move_to(point);
                            first = false;
                        } else {
                            b.line_to(point);
                        }
                        if x >= right {
                            break;
                        }
                        x += 2.0;
                    }
                });
                frame.stroke(&path, Stroke::default().with_color(content).with_width(1.0));
            }
            ClipSource::Audio(channel) => {
                let Some(Source::Audio { path }) = app.project.channel(channel).map(|c| &c.source) else { return };
                let prepared = app.project.playlist.clips.iter().find(|c| c.id == clip.id).map_or(clip.audio, |c| c.audio);
                let Some((sample, peaks)) = app.session.waveform(&daw_engine::audio::cache_key(path, prepared)) else { return };
                let ratio = clip.audio.stretch / prepared.stretch;
                let file_start = clip.start as f64 - clip.offset as f64;
                let frame_at = |x: f32| self.tempo.seconds_between(file_start, view.tick(x - HEADER_WIDTH)) * sample.sample_rate / ratio;
                let level = |from: usize, to: usize| {
                    let peaks = peaks.get(from / PEAK_FRAMES..(to - 1) / PEAK_FRAMES + 1).unwrap_or_default();
                    peaks.iter().fold(0.0f32, |m, &p| m.max(p))
                };
                draw_waveform(frame, inner, sample.left.len(), frame_at, level, content);
            }
        }
        let name = source_name(app, clip.source);
        let name = if clip.muted { format!("[muted] {name}") } else { name };
        let visible_x = body.x.max(HEADER_WIDTH);
        let name = timeline::fit(&name, body.x + body.width - visible_x - 4.0);
        timeline::label(frame, name, Point::new(visible_x + 3.0, body.y), text);
    }
}

impl Arrangement<'_> {
    /// The take being recorded, on the track it will land on.
    fn draw_take(&self, frame: &mut Frame<Renderer>, take: &Take) {
        let app = self.app;
        let channels = usize::from(take.channels.max(1));
        let frames = take.samples.len() / channels;
        let rate = f64::from(take.sample_rate);
        let start = take.start;
        let end = start + self.tempo.ticks_spanned(start, frames as f64 / rate);
        if end <= start {
            return;
        }
        let clips = &app.project.playlist.clips;
        let busy = |track: usize| clips.iter().any(|c| c.track == track && (c.start as f64) < end && (c.end() as f64) > start);
        let track = (0..app.project.playlist.tracks.len()).find(|&t| !busy(t)).unwrap_or(app.project.playlist.tracks.len());
        let view = self.state().time;
        let x = HEADER_WIDTH + view.x(start);
        let body = Rectangle {
            x,
            y: self.track_y(track) + 1.0,
            width: (HEADER_WIDTH + view.x(end) - x).max(1.0),
            height: self.state().track_height - 2.0,
        };
        frame.fill_rectangle(body.position(), body.size(), theme::CONTROL_HOVER);
        let inner = Rectangle { y: body.y + 13.0, height: (body.height - 15.0).max(1.0), ..body };
        let frame_at = |x: f32| self.tempo.seconds_between(start, view.tick(x - HEADER_WIDTH)) * rate;
        // Look at a bounded number of frames per column: the take has no
        // peak summary yet and grows every tick.
        let level = |from: usize, to: usize| {
            let stride = ((to - from) / 64).max(1);
            let samples = take.samples[from * channels..to * channels].chunks(channels).step_by(stride);
            samples.flatten().fold(0.0f32, |m, s| m.max(s.abs()))
        };
        draw_waveform(frame, inner, frames, frame_at, level, theme::FILL);
        let visible_x = body.x.max(HEADER_WIDTH);
        let name = timeline::fit("recording", body.x + body.width - visible_x - 4.0);
        timeline::label(frame, name, Point::new(visible_x + 3.0, body.y), theme::TEXT);
    }
}

/// Draw a waveform across `area`, two pixels per column. `frame_at` maps a
/// canvas x to a frame of the file, which has `frames` frames, and `level`
/// gives the peak of frames `from..to`, where `from < to`.
fn draw_waveform(
    frame: &mut Frame<Renderer>,
    area: Rectangle,
    frames: usize,
    frame_at: impl Fn(f32) -> f64,
    level: impl Fn(usize, usize) -> f32,
    color: Color,
) {
    const COLUMN: f32 = 2.0;
    let middle = area.y + area.height / 2.0;
    let mut x = area.x.max(HEADER_WIDTH);
    while x < area.x + area.width {
        let from = frame_at(x).max(0.0) as usize;
        let to = (frame_at(x + COLUMN).max(0.0) as usize).max(from + 1).min(frames);
        if from < to {
            let half = (level(from, to).min(1.0) * area.height / 2.0).max(0.5);
            frame.fill_rectangle(Point::new(x, middle - half), Size::new(COLUMN - 0.5, half * 2.0), color);
        }
        x += COLUMN;
    }
}

impl canvas::Program<AppMessage> for Arrangement<'_> {
    type State = CanvasState;

    fn update(&self, state: &mut CanvasState, event: &canvas::Event, bounds: Rectangle, cursor: mouse::Cursor) -> Option<canvas::Action<AppMessage>> {
        use iced::keyboard::Event as KeyEvent;
        use iced::mouse::{Button, Event as MouseEvent};
        let publish = |m: Message| Some(canvas::Action::publish(AppMessage::from(m)).and_capture());
        match event {
            canvas::Event::Keyboard(KeyEvent::ModifiersChanged(m)) => {
                state.modifiers = *m;
                None
            }
            canvas::Event::Mouse(MouseEvent::ButtonPressed(button)) => {
                let p = cursor.position_in(bounds)?;
                let tick = self.tick_at(p.x);
                if p.y < RULER_HEIGHT {
                    if p.x < HEADER_WIDTH {
                        return None;
                    }
                    return match button {
                        Button::Left => {
                            state.drag = Some(Drag::Seek { tick });
                            Some(canvas::Action::request_redraw().and_capture())
                        }
                        Button::Right => {
                            state.drag = Some(Drag::Loop { from: tick, to: tick });
                            Some(canvas::Action::request_redraw().and_capture())
                        }
                        _ => None,
                    };
                }
                let track = self.track_at(p.y).max(0);
                if p.x < HEADER_WIDTH {
                    let track = track as usize;
                    if track >= self.app.project.playlist.tracks.len() {
                        return None;
                    }
                    if *button == Button::Right {
                        let open = AppMessage::from(menu::Message::Open(menu::Item::Track(track)));
                        return Some(canvas::Action::publish(open).and_capture());
                    }
                    return publish(if p.x < MUTE_WIDTH { Message::Mute(track) } else { Message::SelectTrack(track) });
                }
                let double = state.clicks.press(p);
                match (button, self.hit(p)) {
                    (Button::Left, Some((id, _))) if state.modifiers.alt() => publish(Message::Split(id, tick)),
                    (Button::Left, Some((id, _))) if double => publish(Message::Open(id)),
                    (Button::Left, Some((id, edge))) => {
                        state.drag = Some(Drag::Clips { tick, track });
                        publish(Message::Begin { id, edge, additive: state.modifiers.shift() })
                    }
                    (Button::Left, None) if !timeline::box_select_modifier(state.modifiers) => {
                        let app = self.app;
                        let start = if state.modifiers.command() {
                            tick.max(0.0) as Ticks
                        } else {
                            app.project.grid.snap_floor(tick.max(0.0) as Ticks, app.project.signature)
                        };
                        state.drag = Some(Drag::Clips { tick: start as f64, track });
                        publish(Message::Add { start, track: track as usize })
                    }
                    (Button::Right, Some((id, _))) => {
                        let open = menu::Message::Open(menu::Item::Clip(id));
                        Some(canvas::Action::publish(open.into()).and_capture())
                    }
                    (Button::Left | Button::Right, _) => {
                        state.drag = Some(Drag::Box { from: p, to: p, additive: state.modifiers.shift() });
                        Some(canvas::Action::request_redraw().and_capture())
                    }
                    _ => None,
                }
            }
            canvas::Event::Mouse(MouseEvent::CursorMoved { .. }) => {
                let p = cursor.position_from(bounds.position())?;
                let tick = self.tick_at(p.x);
                match state.drag.as_mut()? {
                    Drag::Clips { tick: origin, track } => publish(Message::Drag {
                        ticks: tick - *origin,
                        tracks: self.track_at(p.y) - *track,
                        bypass: state.modifiers.command(),
                    }),
                    Drag::Box { to, .. } => {
                        *to = p;
                        Some(canvas::Action::request_redraw())
                    }
                    Drag::Seek { tick: to } | Drag::Loop { to, .. } => {
                        *to = tick;
                        Some(canvas::Action::request_redraw())
                    }
                }
            }
            canvas::Event::Mouse(MouseEvent::ButtonReleased(_)) => match state.drag.take()? {
                Drag::Clips { .. } => publish(Message::End),
                Drag::Box { from, to, additive } => publish(Message::BoxSelect {
                    from: (self.tick_at(from.x), self.track_at(from.y).max(0) as usize),
                    to: (self.tick_at(to.x), self.track_at(to.y).max(0) as usize),
                    additive,
                }),
                Drag::Seek { tick } => publish(Message::Seek(tick)),
                Drag::Loop { from, to } => {
                    let tiny = (self.state().time.x(to) - self.state().time.x(from)).abs() < 3.0;
                    publish(Message::Loop((!tiny).then_some((from, to))))
                }
            },
            canvas::Event::Mouse(MouseEvent::WheelScrolled { delta }) => {
                let p = cursor.position_in(bounds)?;
                match Wheel::from_event(self.state().time, *delta, state.modifiers, p.x - HEADER_WIDTH) {
                    Wheel::Time(view) => publish(Message::View(view)),
                    Wheel::Vertical(steps) => publish(Message::ScrollTracks(-steps.round() as i32)),
                    Wheel::Height(steps) => publish(Message::ZoomTracks { steps, y: p.y }),
                    Wheel::Alternate(_) => None,
                }
            }
            _ => None,
        }
    }

    fn draw(&self, state: &CanvasState, renderer: &Renderer, _theme: &Theme, bounds: Rectangle, _cursor: mouse::Cursor) -> Vec<Geometry> {
        let app = self.app;
        let playlist = self.state();
        let mut frame = Frame::new(renderer, bounds.size());
        let size = bounds.size();
        let area = Size::new(size.width - HEADER_WIDTH, size.height - RULER_HEIGHT);
        let th = playlist.track_height;

        timeline::draw_grid(&mut frame, playlist.time, HEADER_WIDTH, RULER_HEIGHT, area, app.project.signature, app.project.grid);
        let last_track = self.track_at(size.height).max(0) as usize;
        for track in playlist.top_track..=last_track {
            let y = self.track_y(track) + th - 1.0;
            frame.fill_rectangle(Point::new(HEADER_WIDTH, y), Size::new(area.width, 1.0), theme::GRID);
        }

        for clip in &app.project.playlist.clips {
            let clip = playlist.drag_preview.iter().find(|c| c.id == clip.id).unwrap_or(clip);
            if clip.track < playlist.top_track || clip.track > last_track { continue; }
            let selected = playlist.selected.contains(&clip.id);
            self.draw_clip(&mut frame, clip, selected);
        }
        if let Some(take) = app.session.current_take() {
            self.draw_take(&mut frame, take);
        }

        if let Some(Drag::Box { from, to, .. }) = state.drag {
            let top_left = Point::new(from.x.min(to.x), from.y.min(to.y));
            let box_size = Size::new((from.x - to.x).abs(), (from.y - to.y).abs());
            frame.fill_rectangle(top_left, box_size, Color { a: 0.08, ..theme::BRIGHT });
            frame.stroke_rectangle(top_left, box_size, Stroke::default().with_color(theme::TEXT_DIM).with_width(1.0));
        }

        // Track headers.
        frame.fill_rectangle(Point::new(0.0, RULER_HEIGHT), Size::new(HEADER_WIDTH, area.height), theme::HEADER);
        for track in playlist.top_track..=last_track {
            let Some(info) = app.project.playlist.tracks.get(track) else { break };
            let y = self.track_y(track);
            if playlist.selected_track == Some(track) {
                frame.fill_rectangle(Point::new(0.0, y), Size::new(HEADER_WIDTH, th - 1.0), theme::CONTROL_ACTIVE);
            }
            let color = if info.mute { theme::TEXT_FAINT } else { theme::TEXT };
            timeline::label(&mut frame, info.name.clone(), Point::new(16.0, y + th / 2.0 - 7.0), color);
            let dot = if info.mute { theme::FILL_DIM } else { theme::SELECTED };
            frame.fill_rectangle(Point::new(5.0, y + th / 2.0 - 3.0), Size::new(6.0, 6.0), dot);
        }

        timeline::draw_ruler(&mut frame, playlist.time, HEADER_WIDTH, area.width, app.project.signature);
        let loop_range = match state.drag {
            Some(Drag::Loop { from, to }) => Some((from.min(to), from.max(to))),
            _ => app.project.playlist.loop_range.map(|(a, b)| (a as f64, b as f64)),
        };
        if let Some((a, b)) = loop_range {
            let x0 = HEADER_WIDTH + playlist.time.x(a).max(0.0);
            let x1 = HEADER_WIDTH + playlist.time.x(b);
            if x1 > x0 {
                frame.fill_rectangle(Point::new(x0, RULER_HEIGHT - 4.0), Size::new(x1 - x0, 4.0), theme::FILL);
            }
        }
        frame.fill_rectangle(Point::ORIGIN, Size::new(HEADER_WIDTH, RULER_HEIGHT), theme::HEADER);
        // While dragging in the ruler, the marker follows the pointer.
        let seeking = match state.drag {
            Some(Drag::Seek { tick }) => Some(self.marker_tick(tick) as f64),
            _ => None,
        };
        if let Some(tick) = seeking {
            timeline::draw_start_marker(&mut frame, playlist.time, HEADER_WIDTH, size.height, tick);
        }
        if app.mode == PlayMode::Song {
            // The start marker stays put; the playhead only moves away from it while playing.
            if seeking.is_none() {
                timeline::draw_start_marker(&mut frame, playlist.time, HEADER_WIDTH, size.height, app.song_start);
            }
            if app.playing {
                timeline::draw_playhead(&mut frame, playlist.time, HEADER_WIDTH, size.height, app.position);
            }
        }
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(&self, state: &CanvasState, bounds: Rectangle, cursor: mouse::Cursor) -> mouse::Interaction {
        let Some(p) = cursor.position_in(bounds) else { return mouse::Interaction::default() };
        match (state.drag, self.hit(p)) {
            (Some(Drag::Clips { .. }), _) => mouse::Interaction::Grabbing,
            (_, Some((_, Some(_)))) => mouse::Interaction::ResizingHorizontally,
            (_, Some(_)) => mouse::Interaction::Grab,
            _ if p.x < HEADER_WIDTH || p.y < RULER_HEIGHT => mouse::Interaction::Pointer,
            _ => mouse::Interaction::Crosshair,
        }
    }
}
