//! Note editor for the selected channel in the selected pattern.
//!
//! Left click places a note or drags notes (the right edge resizes), right
//! click deletes, right or Ctrl drag selects a box, Cmd bypasses snapping.
//! Clicking in the ruler sets where pattern playback starts; dragging the
//! handle at the pattern end changes the length.

use daw_engine::song::{PlayMode, channel_node};
use daw_model::time::{Grid, Ticks};
use daw_model::{ChannelId, Note};
use iced::keyboard::{Key, Modifiers, key::Named};
use iced::widget::canvas::{self, Canvas, Frame, Geometry};
use iced::widget::row;
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Size, Theme, mouse};

use super::timeline::{self, Clicks, RULER_HEIGHT, TimeView};
use super::{label, pick};
use crate::app::{App, Message as AppMessage};
use crate::theme;

const KEYS_WIDTH: f32 = 40.0;
const EDGE: f32 = 5.0;

#[derive(Debug)]
pub struct State {
    pub time: TimeView,
    pub key_height: f32,
    /// Highest key shown at the top edge.
    pub top_key: i32,
    pub selected: Vec<usize>,
    /// Length for new notes; follows the last resized note.
    pub length: Ticks,
    originals: Vec<(usize, Note)>,
    resizing: bool,
    clipboard: Vec<Note>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            time: TimeView::new(64.0),
            key_height: 12.0,
            top_key: 84,
            selected: Vec::new(),
            length: daw_model::STEP_TICKS,
            originals: Vec::new(),
            resizing: false,
            clipboard: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    View(TimeView),
    ScrollKeys(i32),
    Add { start: Ticks, key: u8 },
    Begin { index: usize, resize: bool, additive: bool },
    Drag { ticks: f64, keys: i32, bypass: bool },
    End,
    Delete(usize),
    BoxSelect { from: (f64, i32), to: (f64, i32), additive: bool },
    Preview(u8),
    Velocity(usize, f32),
    Grid(Grid),
    Channel(ChannelId),
    /// Drag of the end handle: move the pattern end to this tick, rounded to a bar.
    PatternEnd(f64),
    /// Click in the ruler: start pattern playback here.
    Seek(f64),
}

impl From<Message> for AppMessage {
    fn from(message: Message) -> Self {
        AppMessage::PianoRoll(message)
    }
}

fn notes(app: &App) -> &[Note] {
    match (app.project.pattern(app.selected_pattern), app.selected_channel) {
        (Some(pattern), Some(channel)) => pattern.notes(channel),
        _ => &[],
    }
}

fn notes_mut(app: &mut App) -> Option<&mut Vec<Note>> {
    let channel = app.selected_channel?;
    Some(app.project.pattern_mut(app.selected_pattern)?.notes_mut(channel))
}

fn preview(app: &mut App, key: u8) {
    let Some(channel) = app.selected_channel.and_then(|c| app.project.channel(c)) else { return };
    let node = channel_node(&channel.source, channel.id);
    app.session.note(node, key, 0.8);
    app.session.note(node, key, 0.0);
}

/// Sort notes by start and keep the selection pointing at the same notes.
fn sort(app: &mut App) {
    let selected = std::mem::take(&mut app.piano_roll.selected);
    let Some(notes) = notes_mut(app) else { return };
    let mut tagged: Vec<(bool, Note)> = notes.iter().enumerate().map(|(i, n)| (selected.contains(&i), *n)).collect();
    tagged.sort_by_key(|(_, n)| (n.start, n.key));
    *notes = tagged.iter().map(|(_, n)| *n).collect();
    app.piano_roll.selected = tagged.iter().enumerate().filter(|(_, (s, _))| *s).map(|(i, _)| i).collect();
    // Grow to fit notes just placed or moved past the end; notes already past
    // a shortened end stay there.
    let end = tagged.iter().filter(|(s, _)| *s).map(|(_, n)| n.end()).max().unwrap_or(0);
    let pattern = app.selected_pattern;
    if app.project.pattern(pattern).is_some_and(|p| end > p.length) {
        let length = app.project.bars_to(end);
        app.project.set_pattern_length(pattern, length);
    }
}

fn begin_drag(app: &mut App) {
    let notes = notes(app).to_vec();
    app.piano_roll.originals = app.piano_roll.selected.iter().filter_map(|&i| notes.get(i).map(|n| (i, *n))).collect();
}

pub fn update(app: &mut App, message: Message) {
    match message {
        Message::View(view) => app.piano_roll.time = view,
        Message::ScrollKeys(keys) => {
            let visible = 24;
            app.piano_roll.top_key = (app.piano_roll.top_key + keys).clamp(visible, 127);
        }
        Message::PatternEnd(tick) => {
            let bar = app.project.signature.ticks_per_bar();
            let length = ((tick / bar as f64).round().max(1.0) as Ticks) * bar;
            if app.project.pattern(app.selected_pattern).is_some_and(|p| p.length != length) {
                app.begin_edit();
                app.project.set_pattern_length(app.selected_pattern, length);
                app.edited();
            }
        }
        Message::Seek(tick) => {
            if app.mode != PlayMode::Pattern(app.selected_pattern) {
                app.mode = PlayMode::Pattern(app.selected_pattern);
                app.refresh();
            }
            // Set the start marker; while playing, also jump there.
            let (grid, signature) = (app.project.grid, app.project.signature);
            app.pattern_start = grid.snap_floor(tick.max(0.0) as Ticks, signature) as f64;
            app.session.seek(app.pattern_start);
            app.position = app.pattern_start;
        }
        Message::Add { start, key } => {
            if app.selected_channel.is_none() {
                app.set_status("select a channel first");
                return;
            }
            app.checkpoint();
            let length = app.piano_roll.length;
            let Some(notes) = notes_mut(app) else { return };
            notes.push(Note { start, length, key, velocity: 0.8 });
            let index = notes.len() - 1;
            app.piano_roll.selected = vec![index];
            app.piano_roll.resizing = false;
            begin_drag(app);
            preview(app, key);
            app.edited();
        }
        Message::Begin { index, resize, additive } => {
            app.checkpoint();
            let selected = &mut app.piano_roll.selected;
            if additive {
                if let Some(position) = selected.iter().position(|&i| i == index) {
                    selected.remove(position);
                } else {
                    selected.push(index);
                }
            } else if !selected.contains(&index) {
                *selected = vec![index];
            }
            app.piano_roll.resizing = resize;
            begin_drag(app);
            if let Some(note) = notes(app).get(index).copied()
                && !resize
            {
                preview(app, note.key);
            }
        }
        Message::Drag { ticks, keys, bypass } => {
            let (grid, signature) = (app.project.grid, app.project.signature);
            let delta = timeline::snap_delta(ticks, grid, signature, bypass);
            let min_length = grid.step(signature).filter(|_| !bypass).unwrap_or(10);
            let originals = app.piano_roll.originals.clone();
            let resizing = app.piano_roll.resizing;
            let mut new_length = None;
            let Some(notes) = notes_mut(app) else { return };
            for (index, original) in originals {
                let Some(note) = notes.get_mut(index) else { continue };
                if resizing {
                    note.length = (original.length as i64 + delta).max(min_length as i64) as Ticks;
                    new_length = Some(note.length);
                } else {
                    note.start = (original.start as i64 + delta).max(0) as Ticks;
                    note.key = (i32::from(original.key) + keys).clamp(0, 127) as u8;
                }
            }
            if let Some(length) = new_length {
                app.piano_roll.length = length;
            }
            app.edited();
        }
        Message::End => {
            app.piano_roll.originals.clear();
            sort(app);
            app.edited();
        }
        Message::Delete(index) => {
            app.checkpoint();
            if let Some(notes) = notes_mut(app)
                && index < notes.len()
            {
                notes.remove(index);
            }
            app.piano_roll.selected.clear();
            app.edited();
        }
        Message::BoxSelect { from, to, additive } => {
            let (t0, t1) = (from.0.min(to.0), from.0.max(to.0));
            let (k0, k1) = (from.1.min(to.1), from.1.max(to.1));
            let hits: Vec<usize> = notes(app)
                .iter()
                .enumerate()
                .filter(|(_, n)| (n.end() as f64) > t0 && (n.start as f64) < t1 && (k0..=k1).contains(&i32::from(n.key)))
                .map(|(i, _)| i)
                .collect();
            let selected = &mut app.piano_roll.selected;
            if !additive {
                selected.clear();
            }
            for hit in hits {
                if !selected.contains(&hit) {
                    selected.push(hit);
                }
            }
        }
        Message::Preview(key) => preview(app, key),
        Message::Velocity(index, delta) => {
            app.checkpoint();
            let mut shown = None;
            if let Some(note) = notes_mut(app).and_then(|n| n.get_mut(index)) {
                note.velocity = (note.velocity + delta).clamp(0.01, 1.0);
                shown = Some(note.velocity);
            }
            if let Some(velocity) = shown {
                app.set_status(format!("velocity {:.0}%", velocity * 100.0));
            }
            app.edited();
        }
        Message::Grid(grid) => app.project.grid = grid,
        Message::Channel(id) => {
            super::channel_rack::select(app, id);
            app.piano_roll.selected.clear();
        }
    }
}

/// Delete the selected notes. Returns false when nothing is selected.
pub fn delete_selected(app: &mut App) -> bool {
    if app.piano_roll.selected.is_empty() {
        return false;
    }
    app.checkpoint();
    let mut selected = std::mem::take(&mut app.piano_roll.selected);
    selected.sort_unstable();
    if let Some(notes) = notes_mut(app) {
        for index in selected.into_iter().rev() {
            if index < notes.len() {
                notes.remove(index);
            }
        }
    }
    app.edited();
    true
}

/// Keys handled while the piano roll has focus. Returns false when unused.
pub fn key(app: &mut App, key: &Key, modifiers: Modifiers) -> bool {
    let count = notes(app).len();
    match key {
        Key::Character(c) if c.as_str() == "q" && modifiers.is_empty() => {
            let (grid, signature) = (app.project.grid, app.project.signature);
            let selected = app.piano_roll.selected.clone();
            app.checkpoint();
            if let Some(notes) = notes_mut(app) {
                for (index, note) in notes.iter_mut().enumerate() {
                    if selected.is_empty() || selected.contains(&index) {
                        note.start = grid.snap(note.start, signature);
                    }
                }
            }
            sort(app);
            app.set_status(format!("quantized to {}", grid.label()));
            app.edited();
        }
        Key::Character(c) if modifiers.logo() && c.as_str() == "a" => app.piano_roll.selected = (0..count).collect(),
        Key::Character(c) if modifiers.logo() && c.as_str() == "c" => {
            let notes = notes(app);
            app.piano_roll.clipboard = app.piano_roll.selected.iter().filter_map(|&i| notes.get(i).copied()).collect();
        }
        Key::Character(c) if modifiers.logo() && (c.as_str() == "v" || c.as_str() == "d") => {
            let source: Vec<Note> = if c.as_str() == "d" {
                let notes = notes(app);
                app.piano_roll.selected.iter().filter_map(|&i| notes.get(i).copied()).collect()
            } else {
                app.piano_roll.clipboard.clone()
            };
            let (Some(first), Some(last)) = (source.iter().map(|n| n.start).min(), source.iter().map(Note::end).max()) else {
                return true;
            };
            let bar = app.project.signature.ticks_per_bar();
            // Duplicate lands after the selection, rounded to its bar span.
            let shift = (last - first).div_ceil(bar).max(1) * bar;
            let offset = if c.as_str() == "d" { shift as i64 } else { app.project.grid.snap_floor(app.position as Ticks, app.project.signature) as i64 - first as i64 };
            app.checkpoint();
            let Some(notes) = notes_mut(app) else { return true };
            let start = notes.len();
            notes.extend(source.iter().map(|n| Note { start: (n.start as i64 + offset).max(0) as Ticks, ..*n }));
            let end = notes.len();
            app.piano_roll.selected = (start..end).collect();
            sort(app);
            app.edited();
        }
        Key::Named(named @ (Named::ArrowLeft | Named::ArrowRight | Named::ArrowUp | Named::ArrowDown))
            if !app.piano_roll.selected.is_empty() =>
        {
            let step = app.project.grid.step(app.project.signature).unwrap_or(daw_model::STEP_TICKS) as f64;
            let octave = if modifiers.shift() { 12 } else { 1 };
            let (ticks, keys) = match named {
                Named::ArrowLeft => (-step, 0),
                Named::ArrowRight => (step, 0),
                Named::ArrowUp => (0.0, octave),
                _ => (0.0, -octave),
            };
            app.checkpoint();
            app.piano_roll.resizing = false;
            begin_drag(app);
            update(app, Message::Drag { ticks, keys, bypass: false });
            update(app, Message::End);
        }
        _ => return false,
    }
    true
}

#[derive(Debug, Clone, PartialEq)]
struct ChannelChoice {
    id: ChannelId,
    name: String,
}

impl std::fmt::Display for ChannelChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)
    }
}

pub fn toolbar(app: &App) -> Element<'_, AppMessage> {
    let channels: Vec<ChannelChoice> =
        app.project.channels.iter().map(|c| ChannelChoice { id: c.id, name: c.name.clone() }).collect();
    let selected = channels.iter().find(|c| Some(c.id) == app.selected_channel).cloned();
    row![
        pick(channels, selected, |c: ChannelChoice| Message::Channel(c.id).into()),
        label("snap"),
        pick(Grid::CHOICES.to_vec(), Some(app.project.grid), |g| Message::Grid(g).into()),
    ]
    .spacing(4)
    .align_y(iced::Alignment::Center)
    .into()
}

pub fn view(app: &App, focused: bool) -> Element<'_, AppMessage> {
    Canvas::new(Roll { app, focused }).width(Length::Fill).height(Length::Fill).into()
}

struct Roll<'a> {
    app: &'a App,
    #[allow(dead_code)]
    focused: bool,
}

#[derive(Debug, Clone, Copy)]
enum Drag {
    Notes { tick: f64, key: i32 },
    Box { from: Point, to: Point, additive: bool },
    /// Moving the pattern end in the ruler.
    End,
    /// Moving the start marker in the ruler.
    Seek,
}

#[derive(Debug, Default)]
pub struct CanvasState {
    drag: Option<Drag>,
    modifiers: Modifiers,
    clicks: Clicks,
}

fn is_black(key: i32) -> bool {
    matches!(key.rem_euclid(12), 1 | 3 | 6 | 8 | 10)
}

fn key_name(key: i32) -> String {
    format!("C{}", key / 12 - 1)
}

impl Roll<'_> {
    fn state(&self) -> &State {
        &self.app.piano_roll
    }

    fn key_at(&self, y: f32) -> i32 {
        self.state().top_key - ((y - RULER_HEIGHT) / self.state().key_height).floor() as i32
    }

    fn key_y(&self, key: i32) -> f32 {
        RULER_HEIGHT + (self.state().top_key - key) as f32 * self.state().key_height
    }

    fn tick_at(&self, x: f32) -> f64 {
        self.state().time.tick(x - KEYS_WIDTH)
    }

    fn note_rect(&self, note: &Note) -> Rectangle {
        let view = self.state().time;
        let x = KEYS_WIDTH + view.x(note.start as f64);
        let width = (view.x(note.end() as f64) - view.x(note.start as f64)).max(3.0);
        Rectangle { x, y: self.key_y(i32::from(note.key)), width, height: self.state().key_height }
    }

    /// Whether `x` is on the pattern end handle in the ruler.
    fn near_end(&self, x: f32) -> bool {
        let Some(pattern) = self.app.project.pattern(self.app.selected_pattern) else { return false };
        (KEYS_WIDTH + self.state().time.x(pattern.length as f64) - x).abs() <= EDGE + 1.0
    }

    fn hit(&self, p: Point) -> Option<(usize, bool)> {
        notes(self.app).iter().enumerate().rev().find_map(|(i, n)| {
            let r = self.note_rect(n);
            r.contains(p).then_some((i, p.x > r.x + r.width - EDGE && r.width > EDGE * 2.0))
        })
    }
}

impl canvas::Program<AppMessage> for Roll<'_> {
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
                if p.y < RULER_HEIGHT {
                    if p.x < KEYS_WIDTH || *button != Button::Left {
                        return None;
                    }
                    if self.near_end(p.x) {
                        state.drag = Some(Drag::End);
                        return publish(Message::PatternEnd(self.tick_at(p.x)));
                    }
                    state.drag = Some(Drag::Seek);
                    return publish(Message::Seek(self.tick_at(p.x)));
                }
                let key = self.key_at(p.y).clamp(0, 127);
                if p.x < KEYS_WIDTH {
                    return publish(Message::Preview(key as u8));
                }
                let tick = self.tick_at(p.x);
                let double = state.clicks.press(p);
                match (button, self.hit(p)) {
                    (Button::Left, Some((index, _))) if double => publish(Message::Delete(index)),
                    (Button::Left, Some((index, resize))) => {
                        state.drag = Some(Drag::Notes { tick, key });
                        publish(Message::Begin { index, resize, additive: state.modifiers.shift() })
                    }
                    (Button::Left, None) if !state.modifiers.control() => {
                        let app = self.app;
                        let start = if state.modifiers.logo() {
                            tick.max(0.0) as Ticks
                        } else {
                            app.project.grid.snap_floor(tick.max(0.0) as Ticks, app.project.signature)
                        };
                        state.drag = Some(Drag::Notes { tick: start as f64, key });
                        publish(Message::Add { start, key: key as u8 })
                    }
                    (Button::Right, Some((index, _))) => publish(Message::Delete(index)),
                    (Button::Left | Button::Right, _) => {
                        state.drag = Some(Drag::Box { from: p, to: p, additive: state.modifiers.shift() });
                        Some(canvas::Action::request_redraw().and_capture())
                    }
                    _ => None,
                }
            }
            canvas::Event::Mouse(MouseEvent::CursorMoved { .. }) => {
                let p = cursor.position_from(bounds.position())?;
                match state.drag.as_mut()? {
                    Drag::Notes { tick, key } => publish(Message::Drag {
                        ticks: self.tick_at(p.x) - *tick,
                        keys: self.key_at(p.y) - *key,
                        bypass: state.modifiers.logo(),
                    }),
                    Drag::Box { to, .. } => {
                        *to = p;
                        Some(canvas::Action::request_redraw())
                    }
                    Drag::End => publish(Message::PatternEnd(self.tick_at(p.x))),
                    Drag::Seek => publish(Message::Seek(self.tick_at(p.x))),
                }
            }
            canvas::Event::Mouse(MouseEvent::ButtonReleased(_)) => match state.drag.take()? {
                Drag::Notes { .. } => publish(Message::End),
                Drag::End => Some(canvas::Action::publish(AppMessage::EndEdit).and_capture()),
                Drag::Seek => None,
                Drag::Box { from, to, additive } => publish(Message::BoxSelect {
                    from: (self.tick_at(from.x), self.key_at(from.y)),
                    to: (self.tick_at(to.x), self.key_at(to.y)),
                    additive,
                }),
            },
            canvas::Event::Mouse(MouseEvent::WheelScrolled { delta }) => {
                let p = cursor.position_in(bounds)?;
                let view = self.state().time;
                if state.modifiers.alt()
                    && let Some((index, _)) = self.hit(p)
                {
                    let (_, lines) = super::wheel_lines(*delta);
                    return publish(Message::Velocity(index, lines * 0.05));
                }
                let zoom = state.modifiers.logo() || state.modifiers.control();
                if let Some(view) = view.wheel(*delta, zoom, p.x - KEYS_WIDTH) {
                    return publish(Message::View(view));
                }
                let (_, lines) = super::wheel_lines(*delta);
                if state.modifiers.shift() {
                    return publish(Message::View(view.scroll_by_lines(lines)));
                }
                publish(Message::ScrollKeys((lines * 3.0).round() as i32))
            }
            _ => None,
        }
    }

    fn draw(&self, state: &CanvasState, renderer: &Renderer, _theme: &Theme, bounds: Rectangle, _cursor: mouse::Cursor) -> Vec<Geometry> {
        let app = self.app;
        let roll = self.state();
        let mut frame = Frame::new(renderer, bounds.size());
        let size = bounds.size();
        let area = Size::new(size.width - KEYS_WIDTH, size.height - RULER_HEIGHT);
        let kh = roll.key_height;
        let bottom_key = self.key_at(size.height) - 1;

        // Key rows.
        for key in bottom_key.max(0)..=roll.top_key.min(127) {
            let y = self.key_y(key);
            if is_black(key) {
                frame.fill_rectangle(Point::new(KEYS_WIDTH, y), Size::new(area.width, kh), Color::from_rgb8(0x10, 0x10, 0x10));
            }
            if key % 12 == 0 {
                frame.fill_rectangle(Point::new(KEYS_WIDTH, y + kh - 1.0), Size::new(area.width, 1.0), theme::GRID_STRONG);
            }
        }
        timeline::draw_grid(&mut frame, roll.time, KEYS_WIDTH, RULER_HEIGHT, area, app.project.signature, app.project.grid);

        // Pattern end.
        if let Some(pattern) = app.project.pattern(app.selected_pattern) {
            let x = (KEYS_WIDTH + roll.time.x(pattern.length as f64)).max(KEYS_WIDTH);
            if x < size.width {
                frame.fill_rectangle(Point::new(x, RULER_HEIGHT), Size::new(size.width - x, area.height), Color { a: 0.45, ..Color::BLACK });
            }
        }

        // Other channels as outlines for context.
        if let Some(pattern) = app.project.pattern(app.selected_pattern) {
            for lane in pattern.lanes.iter().filter(|l| Some(l.channel) != app.selected_channel) {
                for note in &lane.notes {
                    let r = self.note_rect(note);
                    frame.fill_rectangle(Point::new(r.x, r.y + 1.0), Size::new(r.width - 1.0, r.height - 2.0), theme::GRID_STRONG);
                }
            }
        }

        for (index, note) in notes(app).iter().enumerate() {
            let r = self.note_rect(note);
            if r.x + r.width < KEYS_WIDTH {
                continue;
            }
            // Brightness follows velocity.
            let color = if roll.selected.contains(&index) {
                theme::SELECTED
            } else {
                let level = 0.25 + 0.35 * note.velocity;
                Color::from_rgb(level, level, level)
            };
            frame.fill_rectangle(Point::new(r.x, r.y + 1.0), Size::new(r.width - 1.0, r.height - 1.0), color);
        }

        if let Some(Drag::Box { from, to, .. }) = state.drag {
            let top_left = Point::new(from.x.min(to.x), from.y.min(to.y));
            let box_size = Size::new((from.x - to.x).abs(), (from.y - to.y).abs());
            frame.fill_rectangle(top_left, box_size, Color { a: 0.08, ..theme::BRIGHT });
            frame.stroke_rectangle(top_left, box_size, canvas::Stroke::default().with_color(theme::TEXT_DIM).with_width(1.0));
        }

        // Keyboard.
        frame.fill_rectangle(Point::new(0.0, RULER_HEIGHT), Size::new(KEYS_WIDTH, area.height), Color::from_rgb8(0x2c, 0x2c, 0x2c));
        for key in bottom_key.max(0)..=roll.top_key.min(127) {
            let y = self.key_y(key);
            if is_black(key) {
                frame.fill_rectangle(Point::new(0.0, y), Size::new(KEYS_WIDTH * 0.62, kh), theme::HEADER);
            } else if key % 12 == 0 || key % 12 == 5 {
                frame.fill_rectangle(Point::new(0.0, y + kh - 1.0), Size::new(KEYS_WIDTH, 1.0), theme::HEADER);
            }
            if key % 12 == 0 {
                timeline::label(&mut frame, key_name(key), Point::new(KEYS_WIDTH - 18.0, y), theme::TEXT_DIM);
            }
        }

        timeline::draw_ruler(&mut frame, roll.time, KEYS_WIDTH, area.width, app.project.signature);
        // Pattern end handle in the ruler; drag it to change the length.
        if let Some(pattern) = app.project.pattern(app.selected_pattern) {
            let x = KEYS_WIDTH + roll.time.x(pattern.length as f64).round();
            if x >= KEYS_WIDTH && x < size.width {
                frame.fill_rectangle(Point::new(x - 1.0, 0.0), Size::new(2.0, RULER_HEIGHT), theme::TEXT_DIM);
            }
        }
        frame.fill_rectangle(Point::ORIGIN, Size::new(KEYS_WIDTH, RULER_HEIGHT), theme::HEADER);
        if app.mode == PlayMode::Pattern(app.selected_pattern) {
            // The start marker stays put; the playhead only moves away from it while playing.
            timeline::draw_start_marker(&mut frame, roll.time, KEYS_WIDTH, size.height, app.pattern_start);
            if app.playing {
                timeline::draw_playhead(&mut frame, roll.time, KEYS_WIDTH, size.height, app.position);
            }
        }
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(&self, state: &CanvasState, bounds: Rectangle, cursor: mouse::Cursor) -> mouse::Interaction {
        let Some(p) = cursor.position_in(bounds) else { return mouse::Interaction::default() };
        match (state.drag, self.hit(p)) {
            (Some(Drag::Notes { .. }), _) => mouse::Interaction::Grabbing,
            (Some(Drag::End), _) => mouse::Interaction::ResizingHorizontally,
            _ if p.y < RULER_HEIGHT && p.x >= KEYS_WIDTH && self.near_end(p.x) => mouse::Interaction::ResizingHorizontally,
            (_, Some((_, true))) => mouse::Interaction::ResizingHorizontally,
            (_, Some(_)) => mouse::Interaction::Grab,
            _ if p.x >= KEYS_WIDTH && p.y >= RULER_HEIGHT => mouse::Interaction::Crosshair,
            _ => mouse::Interaction::default(),
        }
    }
}
