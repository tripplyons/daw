//! Note editor for the selected channel in the selected pattern.
//!
//! Left click places a note or drags notes (the right edge resizes), right
//! click deletes, right drag (or Ctrl drag on macOS) selects a box, Cmd
//! (Ctrl on Linux) bypasses snapping.
//! Clicking in the ruler sets where pattern playback starts; dragging the
//! handle at the pattern end changes the length. Alt+Shift+scroll over a
//! note changes its velocity; other scrolling follows `timeline::Wheel`.

use daw_engine::song::{PlayMode, channel_node};
use daw_model::time::{Grid, Ticks};
use daw_model::{ChannelId, Note};
use iced::keyboard::{Key, Modifiers, key::Named};
use iced::widget::canvas::Canvas;
use iced::widget::row;
use iced::{Element, Length};

use super::timeline::{self, RULER_HEIGHT, TimeView};
use super::{label, pick};
use crate::app::{App, Message as AppMessage};

mod roll;
use roll::Roll;

const MIN_KEY_HEIGHT: f32 = 6.0;
const MAX_KEY_HEIGHT: f32 = 40.0;

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
    drag_changed: bool,
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
            drag_changed: false,
            resizing: false,
            clipboard: Vec::new(),
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
    ScrollKeys(i32),
    /// Alt+scroll: key height, keeping the key under `y` in place.
    ZoomKeys { steps: f32, y: f32 },
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
    app.piano_roll.cancel_drag();
    let notes = notes(app).to_vec();
    app.piano_roll.originals = app.piano_roll.selected.iter().filter_map(|&i| notes.get(i).map(|n| (i, *n))).collect();
}

pub fn update(app: &mut App, message: Message) {
    match message {
        Message::View(view) => app.piano_roll.time = view,
        Message::ZoomKeys { steps, y } => {
            let roll = &mut app.piano_roll;
            let rows = (y - RULER_HEIGHT).max(0.0);
            let key = roll.top_key as f32 - rows / roll.key_height;
            roll.key_height = timeline::zoom_height(roll.key_height, steps, MIN_KEY_HEIGHT, MAX_KEY_HEIGHT);
            roll.top_key = (key + rows / roll.key_height).round().clamp(0.0, 127.0) as i32;
        }
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
                app.stop_audio_recording();
                app.mode = PlayMode::Pattern(app.selected_pattern);
                app.refresh();
            }
            // Set the start marker; while playing, also jump there.
            app.pattern_start = timeline::marker_tick(tick, app.project.grid, app.project.signature) as f64;
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
            app.piano_roll.drag_changed = true;
            preview(app, key);
            app.edited();
        }
        Message::Begin { index, resize, additive } => {
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
            let mut changed = Vec::new();
            for (index, original) in originals {
                let mut note = original;
                if resizing {
                    note.length = (original.length as i64 + delta).max(min_length as i64) as Ticks;
                } else {
                    note.start = (original.start as i64 + delta).max(0) as Ticks;
                    note.key = (i32::from(original.key) + keys).clamp(0, 127) as u8;
                }
                if notes(app).get(index).is_some_and(|old| *old != note) { changed.push((index, note)); }
            }
            if changed.is_empty() { return; }
            if !app.piano_roll.drag_changed { app.checkpoint(); }
            app.piano_roll.drag_changed = true;
            if resizing { app.piano_roll.length = changed.last().unwrap().1.length; }
            if let Some(notes) = notes_mut(app) {
                for (index, note) in changed { notes[index] = note; }
            }
            app.edited();
        }
        Message::End => {
            if app.piano_roll.drag_changed { sort(app); app.edited(); }
            app.piano_roll.cancel_drag();
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
        Key::Character(c) if modifiers.command() && c.as_str() == "a" => app.piano_roll.selected = (0..count).collect(),
        Key::Character(c) if modifiers.command() && c.as_str() == "c" => {
            let notes = notes(app);
            app.piano_roll.clipboard = app.piano_roll.selected.iter().filter_map(|&i| notes.get(i).copied()).collect();
        }
        Key::Character(c) if modifiers.command() && (c.as_str() == "v" || c.as_str() == "d") => {
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
