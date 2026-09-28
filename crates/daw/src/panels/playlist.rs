//! Song arrangement: pattern and automation clips on tracks.
//!
//! Left click places the brush clip or drags clips (the right edge resizes),
//! double click opens a clip, right click deletes, right drag (or Ctrl drag
//! on macOS) selects a box, Alt click splits a clip. Click a track name to
//! select the track, or its square to mute it; right click it for its menu.
//! In the ruler, left click seeks and right drag sets the loop.

use daw_engine::song::PlayMode;
use daw_model::time::{Grid, Ticks};
use daw_model::{Clip, ClipId, ClipSource};
use iced::keyboard::{Key, Modifiers};
use iced::widget::canvas::{self, Canvas, Frame, Geometry, Path, Stroke};
use iced::widget::row;
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Size, Theme, mouse};

use super::timeline::{self, Clicks, RULER_HEIGHT, TimeView, Wheel};
use super::{label, pick};
use crate::app::{App, Message as AppMessage};
use crate::menu;
use crate::theme;

const HEADER_WIDTH: f32 = 84.0;
const EDGE: f32 = 5.0;
const MIN_TRACK_HEIGHT: f32 = 16.0;
const MAX_TRACK_HEIGHT: f32 = 120.0;
/// Left part of a track header that toggles mute.
const MUTE_WIDTH: f32 = 14.0;

#[derive(Debug)]
pub struct State {
    pub time: TimeView,
    pub track_height: f32,
    pub top_track: usize,
    pub selected: Vec<ClipId>,
    pub selected_track: Option<usize>,
    /// What a click on empty space places.
    pub brush: Option<ClipSource>,
    originals: Vec<Clip>,
    resizing: bool,
    clipboard: Vec<Clip>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            time: TimeView::new(20.0),
            track_height: 30.0,
            top_track: 0,
            selected: Vec::new(),
            selected_track: None,
            brush: None,
            originals: Vec::new(),
            resizing: false,
            clipboard: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    View(TimeView),
    ScrollTracks(i32),
    /// Alt+scroll: track height, keeping the track under `y` in place.
    ZoomTracks { steps: f32, y: f32 },
    Add { start: Ticks, track: usize },
    Begin { id: ClipId, resize: bool, additive: bool },
    Drag { ticks: f64, tracks: i32, bypass: bool },
    End,
    Delete(ClipId),
    Split(ClipId, f64),
    Open(ClipId),
    BoxSelect { from: (f64, usize), to: (f64, usize), additive: bool },
    Seek(f64),
    Loop(Option<(f64, f64)>),
    Mute(usize),
    SelectTrack(usize),
    AddTrack,
    DeleteTrack,
    Brush(BrushChoice),
    Grid(Grid),
}

impl From<Message> for AppMessage {
    fn from(message: Message) -> Self {
        AppMessage::Playlist(message)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BrushChoice {
    source: ClipSource,
    name: String,
}

impl std::fmt::Display for BrushChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)
    }
}

fn source_name(app: &App, source: ClipSource) -> String {
    match source {
        ClipSource::Pattern(id) => app.project.pattern(id).map(|p| p.name.clone()).unwrap_or_default(),
        ClipSource::Automation(id) => app.project.automation_clip(id).map(|a| a.name.clone()).unwrap_or_default(),
    }
}

/// Position within an automation clip's envelope of `length` ticks at
/// `tick` ticks into its looping playback. The end of each loop maps to the
/// envelope's end, not the next loop's start, so a clip that is exactly one
/// loop long ends on its last value.
fn loop_tick(tick: f64, length: f64) -> f64 {
    if tick <= length {
        return tick.max(0.0);
    }
    let wrapped = tick % length;
    if wrapped == 0.0 { length } else { wrapped }
}

fn begin_drag(app: &mut App) {
    let selected = &app.playlist.selected;
    app.playlist.originals = app.project.playlist.clips.iter().filter(|c| selected.contains(&c.id)).cloned().collect();
}

fn grow_tracks(app: &mut App, needed: usize) {
    while app.project.playlist.tracks.len() < needed {
        let number = app.project.playlist.tracks.len() + 1;
        app.project.playlist.tracks.push(daw_model::Track { name: format!("track {number}"), mute: false });
    }
}

pub fn update(app: &mut App, message: Message) {
    match message {
        Message::View(view) => app.playlist.time = view,
        Message::ZoomTracks { steps, y } => {
            let count = app.project.playlist.tracks.len().max(1);
            let playlist = &mut app.playlist;
            let rows = (y - RULER_HEIGHT).max(0.0);
            let track = playlist.top_track as f32 + rows / playlist.track_height;
            playlist.track_height = timeline::zoom_height(playlist.track_height, steps, MIN_TRACK_HEIGHT, MAX_TRACK_HEIGHT);
            playlist.top_track = (track - rows / playlist.track_height).round().clamp(0.0, (count - 1) as f32) as usize;
        }
        Message::ScrollTracks(tracks) => {
            let top = app.playlist.top_track as i32 + tracks;
            app.playlist.top_track = top.clamp(0, app.project.playlist.tracks.len().saturating_sub(1) as i32) as usize;
        }
        Message::Add { start, track } => {
            let brush = app.playlist.brush.unwrap_or(ClipSource::Pattern(app.selected_pattern));
            app.checkpoint();
            grow_tracks(app, track + 1);
            let id = app.project.add_clip(track, start, brush);
            app.playlist.selected = vec![id];
            app.playlist.resizing = false;
            begin_drag(app);
            app.edited();
        }
        Message::Begin { id, resize, additive } => {
            app.checkpoint();
            let selected = &mut app.playlist.selected;
            if additive {
                if let Some(position) = selected.iter().position(|&c| c == id) {
                    selected.remove(position);
                } else {
                    selected.push(id);
                }
            } else if !selected.contains(&id) {
                *selected = vec![id];
            }
            if let Some(clip) = app.project.playlist.clips.iter().find(|c| c.id == id) {
                app.playlist.brush = Some(clip.source);
            }
            app.playlist.resizing = resize;
            begin_drag(app);
        }
        Message::Drag { ticks, tracks, bypass } => {
            let (grid, signature) = (app.project.grid, app.project.signature);
            let delta = timeline::snap_delta(ticks, grid, signature, bypass);
            let min_length = grid.step(signature).filter(|_| !bypass).unwrap_or(10);
            let originals = app.playlist.originals.clone();
            let resizing = app.playlist.resizing;
            let max_track = originals.iter().map(|c| c.track as i32 + tracks).max().unwrap_or(0);
            grow_tracks(app, (max_track + 1).max(0) as usize);
            for original in originals {
                let Some(clip) = app.project.playlist.clips.iter_mut().find(|c| c.id == original.id) else { continue };
                if resizing {
                    clip.length = (original.length as i64 + delta).max(min_length as i64) as Ticks;
                } else {
                    clip.start = (original.start as i64 + delta).max(0) as Ticks;
                    clip.track = (original.track as i32 + tracks).max(0) as usize;
                }
            }
            app.edited();
        }
        Message::End => {
            app.playlist.originals.clear();
            app.edited();
        }
        Message::Delete(id) => {
            app.checkpoint();
            app.project.playlist.clips.retain(|c| c.id != id);
            app.playlist.selected.retain(|&c| c != id);
            app.edited();
        }
        Message::Split(id, tick) => {
            let (grid, signature) = (app.project.grid, app.project.signature);
            let at = grid.snap(tick.max(0.0) as Ticks, signature);
            let Some(index) = app.project.playlist.clips.iter().position(|c| c.id == id) else { return };
            let clip = app.project.playlist.clips[index].clone();
            if at <= clip.start || at >= clip.end() {
                return;
            }
            app.checkpoint();
            app.project.playlist.clips[index].length = at - clip.start;
            let new = app.project.add_clip(clip.track, at, clip.source);
            if let Some(right) = app.project.playlist.clips.iter_mut().find(|c| c.id == new) {
                right.length = clip.end() - at;
                right.offset = clip.offset + (at - clip.start);
            }
            app.playlist.selected = vec![new];
            app.edited();
        }
        Message::Open(id) => {
            let Some(clip) = app.project.playlist.clips.iter().find(|c| c.id == id) else { return };
            match clip.source {
                ClipSource::Pattern(pattern) => {
                    app.selected_pattern = pattern;
                    if let PlayMode::Pattern(_) = app.mode {
                        app.mode = PlayMode::Pattern(pattern);
                        app.refresh();
                    }
                    app.show_panel(daw_model::layout::Panel::PianoRoll);
                }
                ClipSource::Automation(automation) => app.open_automation(automation),
            }
        }
        Message::BoxSelect { from, to, additive } => {
            let (t0, t1) = (from.0.min(to.0), from.0.max(to.0));
            let (k0, k1) = (from.1.min(to.1), from.1.max(to.1));
            let hits: Vec<ClipId> = app
                .project
                .playlist
                .clips
                .iter()
                .filter(|c| (c.end() as f64) > t0 && (c.start as f64) < t1 && (k0..=k1).contains(&c.track))
                .map(|c| c.id)
                .collect();
            if !additive {
                app.playlist.selected.clear();
            }
            for hit in hits {
                if !app.playlist.selected.contains(&hit) {
                    app.playlist.selected.push(hit);
                }
            }
        }
        Message::Seek(tick) => {
            if let PlayMode::Pattern(_) = app.mode {
                app.mode = PlayMode::Song;
                app.refresh();
            }
            // Set the start marker; while playing, also jump there.
            let (grid, signature) = (app.project.grid, app.project.signature);
            app.song_start = grid.snap_floor(tick.max(0.0) as Ticks, signature) as f64;
            app.session.seek(app.song_start);
            app.position = app.song_start;
        }
        Message::Loop(range) => {
            app.checkpoint();
            let (grid, signature) = (app.project.grid, app.project.signature);
            app.project.playlist.loop_range = range.and_then(|(a, b)| {
                let (a, b) = (grid.snap(a.min(b).max(0.0) as Ticks, signature), grid.snap(a.max(b).max(0.0) as Ticks, signature));
                (b > a).then_some((a, b))
            });
            app.edited();
        }
        Message::Mute(track) => {
            app.checkpoint();
            if let Some(track) = app.project.playlist.tracks.get_mut(track) {
                track.mute = !track.mute;
            }
            app.edited();
        }
        Message::SelectTrack(track) => {
            app.playlist.selected_track = Some(track);
            app.playlist.selected.clear();
        }
        Message::AddTrack => {
            app.checkpoint();
            let index = app.playlist.selected_track.map(|t| t + 1).unwrap_or(app.project.playlist.tracks.len());
            let number = app.project.playlist.tracks.len() + 1;
            app.project.playlist.tracks.insert(index, daw_model::Track { name: format!("track {number}"), mute: false });
            for clip in &mut app.project.playlist.clips {
                if clip.track >= index {
                    clip.track += 1;
                }
            }
            app.playlist.selected_track = Some(index);
            app.edited();
        }
        Message::DeleteTrack => {
            app.playlist.selected.clear();
            delete_selected(app);
        }
        Message::Brush(choice) => app.playlist.brush = Some(choice.source),
        Message::Grid(grid) => app.project.grid = grid,
    }
}

/// Delete the selected clips, or the selected track when no clip is
/// selected. Returns false when nothing is selected.
pub fn delete_selected(app: &mut App) -> bool {
    if !app.playlist.selected.is_empty() {
        app.checkpoint();
        let selected = std::mem::take(&mut app.playlist.selected);
        app.project.playlist.clips.retain(|c| !selected.contains(&c.id));
        app.edited();
        return true;
    }
    let Some(track) = app.playlist.selected_track.filter(|&t| t < app.project.playlist.tracks.len()) else { return false };
    app.checkpoint();
    let name = app.project.playlist.tracks[track].name.clone();
    app.project.remove_track(track);
    let count = app.project.playlist.tracks.len();
    app.playlist.selected_track = Some(track.min(count - 1));
    app.playlist.top_track = app.playlist.top_track.min(count - 1);
    app.set_status(format!("deleted {name}"));
    app.edited();
    true
}

pub fn key(app: &mut App, key: &Key, modifiers: Modifiers) -> bool {
    match key {
        Key::Character(c) if modifiers.command() && c.as_str() == "a" => {
            app.playlist.selected = app.project.playlist.clips.iter().map(|c| c.id).collect();
        }
        Key::Character(c) if modifiers.command() && c.as_str() == "c" => {
            begin_drag(app);
            app.playlist.clipboard = std::mem::take(&mut app.playlist.originals);
        }
        Key::Character(c) if modifiers.command() && (c.as_str() == "v" || c.as_str() == "d") => {
            let source = if c.as_str() == "d" {
                begin_drag(app);
                std::mem::take(&mut app.playlist.originals)
            } else {
                app.playlist.clipboard.clone()
            };
            let (Some(first), Some(last)) = (source.iter().map(|c| c.start).min(), source.iter().map(Clip::end).max()) else {
                return true;
            };
            let offset = if c.as_str() == "d" {
                (last - first) as i64
            } else {
                app.project.grid.snap_floor(app.position as Ticks, app.project.signature) as i64 - first as i64
            };
            app.checkpoint();
            let mut added = Vec::new();
            for clip in source {
                let id = app.project.add_clip(clip.track, (clip.start as i64 + offset).max(0) as Ticks, clip.source);
                if let Some(new) = app.project.playlist.clips.iter_mut().find(|c| c.id == id) {
                    new.length = clip.length;
                    new.offset = clip.offset;
                }
                added.push(id);
            }
            app.playlist.selected = added;
            app.edited();
        }
        _ => return false,
    }
    true
}

pub fn toolbar(app: &App) -> Element<'_, AppMessage> {
    let mut choices: Vec<BrushChoice> = app
        .project
        .patterns
        .iter()
        .map(|p| BrushChoice { source: ClipSource::Pattern(p.id), name: p.name.clone() })
        .collect();
    choices.extend(
        app.project.automation.iter().map(|a| BrushChoice { source: ClipSource::Automation(a.id), name: a.name.clone() }),
    );
    let brush = app.playlist.brush.unwrap_or(ClipSource::Pattern(app.selected_pattern));
    let current = choices.iter().find(|c| c.source == brush).cloned();
    let delete: Element<'_, AppMessage> = match app.playlist.selected_track {
        Some(_) => super::tool("delete track", Message::DeleteTrack.into()),
        None => label(""),
    };
    row![
        super::tool("+ track", Message::AddTrack.into()),
        delete,
        label("brush"),
        pick(choices, current, |c| Message::Brush(c).into()),
        label("snap"),
        pick(Grid::CHOICES.to_vec(), Some(app.project.grid), |g| Message::Grid(g).into()),
    ]
    .spacing(4)
    .align_y(iced::Alignment::Center)
    .into()
}

pub fn view(app: &App, _focused: bool) -> Element<'_, AppMessage> {
    Canvas::new(Arrangement { app }).width(Length::Fill).height(Length::Fill).into()
}

struct Arrangement<'a> {
    app: &'a App,
}

#[derive(Debug, Clone, Copy)]
enum Drag {
    Clips { tick: f64, track: i32 },
    Box { from: Point, to: Point, additive: bool },
    Seek,
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

    fn clip_rect(&self, clip: &Clip) -> Rectangle {
        let view = self.state().time;
        let x = HEADER_WIDTH + view.x(clip.start as f64);
        let width = (view.x(clip.end() as f64) - view.x(clip.start as f64)).max(3.0);
        Rectangle { x, y: self.track_y(clip.track), width, height: self.state().track_height }
    }

    fn hit(&self, p: Point) -> Option<(ClipId, bool)> {
        self.app.project.playlist.clips.iter().rev().find_map(|c| {
            let r = self.clip_rect(c);
            r.contains(p).then_some((c.id, p.x > r.x + r.width - EDGE && r.width > EDGE * 2.0))
        })
    }

    fn draw_clip(&self, frame: &mut Frame<Renderer>, clip: &Clip, selected: bool) {
        let app = self.app;
        let r = self.clip_rect(clip);
        let (background, content, text) = if selected {
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
        }
        let name = source_name(app, clip.source);
        let visible_x = body.x.max(HEADER_WIDTH);
        let name = timeline::fit(&name, body.x + body.width - visible_x - 4.0);
        timeline::label(frame, name, Point::new(visible_x + 3.0, body.y), text);
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
                            state.drag = Some(Drag::Seek);
                            publish(Message::Seek(tick))
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
                    (Button::Left, Some((id, resize))) => {
                        state.drag = Some(Drag::Clips { tick, track });
                        publish(Message::Begin { id, resize, additive: state.modifiers.shift() })
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
                    (Button::Right, Some((id, _))) => publish(Message::Delete(id)),
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
                    Drag::Seek => publish(Message::Seek(tick)),
                    Drag::Loop { to, .. } => {
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
                Drag::Seek => None,
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

        let visible = |clip: &&Clip| clip.track >= playlist.top_track && clip.track <= last_track;
        for clip in app.project.playlist.clips.iter().filter(visible) {
            let selected = playlist.selected.contains(&clip.id);
            self.draw_clip(&mut frame, clip, selected);
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
        if app.mode == PlayMode::Song {
            // The start marker stays put; the playhead only moves away from it while playing.
            timeline::draw_start_marker(&mut frame, playlist.time, HEADER_WIDTH, size.height, app.song_start);
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
            (_, Some((_, true))) => mouse::Interaction::ResizingHorizontally,
            (_, Some(_)) => mouse::Interaction::Grab,
            _ if p.x < HEADER_WIDTH || p.y < RULER_HEIGHT => mouse::Interaction::Pointer,
            _ => mouse::Interaction::Crosshair,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::loop_tick;

    #[test]
    fn loop_tick_keeps_clip_edges() {
        assert_eq!(loop_tick(-1e-9, 100.0), 0.0);
        assert_eq!(loop_tick(100.0, 100.0), 100.0);
        assert_eq!(loop_tick(150.0, 100.0), 50.0);
        assert_eq!(loop_tick(200.0, 100.0), 100.0);
    }
}
