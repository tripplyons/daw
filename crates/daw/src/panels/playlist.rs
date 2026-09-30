//! Song arrangement: pattern, automation, and audio clips on tracks.
//!
//! Left click places the brush clip or drags clips (either edge trims),
//! double click opens a clip, right click deletes, right drag (or Ctrl drag
//! on macOS) selects a box, Alt click splits a clip. Click a track name to
//! select the track, or its square to mute it; right click it for its menu.
//! In the ruler, left click seeks and right drag sets the loop.

use daw_engine::song::PlayMode;
use daw_model::time::{Grid, Ticks};
use daw_model::{AudioEdit, Clip, ClipId, ClipSource, Source};
use iced::keyboard::{Key, Modifiers};
use iced::widget::canvas::Canvas;
use iced::widget::{column, row, slider, text_input};
use iced::{Element, Length};

use super::timeline::{self, RULER_HEIGHT, TimeView};
use super::{label, pick, tool, toggle};
use crate::app::audio::audio_ticks;
use crate::app::{App, Message as AppMessage};
use crate::keys::Action;
use crate::theme;

mod arrangement;
use arrangement::Arrangement;

const MIN_TRACK_HEIGHT: f32 = 16.0;
const MAX_TRACK_HEIGHT: f32 = 120.0;

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
    pub drag_preview: Vec<Clip>,
    drag_added: bool,
    /// The edge being dragged, or `None` when moving clips.
    resizing: Option<Edge>,
    clipboard: Vec<Clip>,
    pub stretch_mode: bool,
    /// Slider preview; audio is prepared only when the drag ends.
    pub pitch_edit: Option<(Vec<ClipId>, f32)>,
    pub audio_text: Option<AudioText>,
}

#[derive(Debug, Clone, Copy)]
pub enum AudioField { Pitch, Cents, Stretch }

#[derive(Debug)]
pub struct AudioText {
    clip: ClipId,
    pitch: String,
    cents: String,
    stretch: String,
}

impl AudioText {
    fn new(clip: &Clip) -> Self {
        Self { clip: clip.id, pitch: clip.audio.semitones.trunc().to_string(),
            cents: format!("{:.3}", (clip.audio.semitones - clip.audio.semitones.trunc()) * 100.0), stretch: clip.audio.stretch.to_string() }
    }
}

/// A clip edge. Dragging the start trims the clip's beginning and keeps its
/// content in place; dragging the end changes its length.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Edge {
    Start,
    End,
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
            drag_preview: Vec::new(),
            drag_added: false,
            resizing: None,
            clipboard: Vec::new(),
            stretch_mode: false,
            pitch_edit: None,
            audio_text: None,
        }
    }
}

impl State {
    pub fn cancel_drag(&mut self) {
        self.originals.clear();
        self.drag_preview.clear();
        self.drag_added = false;
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    MakeUnique,
    Consolidate,
    Reverse,
    AudioPitch(f32),
    AudioPitchDone,
    AudioStretch(f64),
    AudioText(AudioField, String),
    AudioApply(AudioField),
    AudioReset(AudioField),
    StretchMode,
    MuteSelection,
    View(TimeView),
    ScrollTracks(i32),
    /// Alt+scroll: track height, keeping the track under `y` in place.
    ZoomTracks { steps: f32, y: f32 },
    Add { start: Ticks, track: usize },
    Begin { id: ClipId, edge: Option<Edge>, additive: bool },
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
        ClipSource::Audio(id) => app.project.channel(id).map(|c| c.name.clone()).unwrap_or_default(),
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
    app.playlist.cancel_drag();
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
        Message::MakeUnique => {
            let selected = app.playlist.selected.clone();
            if selected.is_empty() { app.set_status("select clips to make unique"); return; }
            app.checkpoint();
            for id in selected {
                if let Some(source) = app.project.make_unique(id) {
                    app.playlist.brush = Some(source);
                    if let ClipSource::Pattern(id) = source { app.selected_pattern = id; }
                }
            }
            app.edited();
            app.set_status("selected clips now have independent sources");
        }
        Message::Consolidate => {
            if let Err(error) = app.consolidate_selection() { app.set_status(format!("consolidation failed: {error}")); }
        }
        Message::StretchMode => app.playlist.stretch_mode = !app.playlist.stretch_mode,
        Message::MuteSelection => {
            if app.playlist.selected.is_empty() { return; }
            app.checkpoint();
            for clip in &mut app.project.playlist.clips {
                if app.playlist.selected.contains(&clip.id) { clip.muted = !clip.muted; }
            }
            app.edited();
        }
        Message::AudioPitch(value) => {
            if !AudioEdit::SEMITONES.contains(&value) { return; }
            match &mut app.playlist.pitch_edit {
                Some((_, pitch)) => *pitch = value,
                None => {
                    let selected: Vec<_> = app.project.playlist.clips.iter()
                        .filter(|c| app.playlist.selected.contains(&c.id) && matches!(c.source, ClipSource::Audio(_)))
                        .map(|c| c.id).collect();
                    if !selected.is_empty() { app.playlist.pitch_edit = Some((selected, value)); }
                }
            }
        }
        Message::AudioPitchDone => {
            let Some((selected, pitch)) = app.playlist.pitch_edit.take() else { return };
            if !app.project.playlist.clips.iter().any(|c| selected.contains(&c.id)
                && matches!(c.source, ClipSource::Audio(_)) && c.audio.semitones != pitch) { return; }
            app.checkpoint();
            app.playlist.audio_text = None;
            for clip in &mut app.project.playlist.clips {
                if selected.contains(&clip.id) && matches!(clip.source, ClipSource::Audio(_)) {
                    clip.audio.semitones = pitch;
                }
            }
            app.edited();
        }
        Message::Reverse | Message::AudioStretch(_) => {
            if let Message::AudioStretch(value) = message
                && !AudioEdit::STRETCH.contains(&value) { return; }
            let changed = app.project.playlist.clips.iter().any(|c| app.playlist.selected.contains(&c.id)
                && matches!(c.source, ClipSource::Audio(_)) && match message {
                    Message::AudioStretch(value) => c.audio.stretch != value,
                    _ => true,
                });
            if !changed { return; }
            app.begin_edit();
            app.playlist.audio_text = None;
            for clip in &mut app.project.playlist.clips {
                if !app.playlist.selected.contains(&clip.id) || !matches!(clip.source, ClipSource::Audio(_)) { continue; }
                match message {
                    Message::Reverse => clip.audio.reverse = !clip.audio.reverse,
                    Message::AudioStretch(value) => {
                        let ratio = value / clip.audio.stretch;
                        clip.length = (clip.length as f64 * ratio).round().max(1.0) as Ticks;
                        clip.offset = (clip.offset as f64 * ratio).round() as Ticks;
                        clip.audio.stretch = value;
                    }
                    _ => unreachable!(),
                }
            }
            app.edited();
            let _ = app.update(AppMessage::EndEdit);
        }
        Message::AudioText(field, value) => {
            let Some(clip) = app.project.playlist.clips.iter().find(|c| app.playlist.selected.contains(&c.id) && matches!(c.source, ClipSource::Audio(_))) else { return };
            if app.playlist.audio_text.as_ref().is_none_or(|text| text.clip != clip.id) { app.playlist.audio_text = Some(AudioText::new(clip)); }
            let text = app.playlist.audio_text.as_mut().unwrap();
            match field { AudioField::Pitch => text.pitch = value, AudioField::Cents => text.cents = value, AudioField::Stretch => text.stretch = value }
        }
        Message::AudioApply(field) => {
            let Some(text) = app.playlist.audio_text.as_ref() else { return };
            if !app.playlist.selected.contains(&text.clip) { app.playlist.audio_text = None; return; }
            match field {
                AudioField::Stretch => {
                    let Ok(value) = text.stretch.trim().parse::<f64>() else { app.set_status("stretch must be a number"); return };
                    let value = match AudioEdit::check_stretch(value) {
                        Ok(value) => value,
                        Err(error) => { app.set_status(error); return; }
                    };
                    update(app, Message::AudioStretch(value));
                }
                _ => {
                    let parsed = text.pitch.trim().parse::<f32>().ok().zip(text.cents.trim().parse::<f32>().ok());
                    let Some((pitch, cents)) = parsed else { app.set_status("pitch and cents must be numbers"); return };
                    let value = pitch + cents / 100.0;
                    let value = match AudioEdit::check_semitones(value) {
                        Ok(value) => value,
                        Err(error) => { app.set_status(format!("combined {error}")); return; }
                    };
                    update(app, Message::AudioPitch(value));
                    update(app, Message::AudioPitchDone);
                }
            }
            app.playlist.audio_text = None;
        }
        Message::AudioReset(field) => {
            match field {
                AudioField::Stretch => update(app, Message::AudioStretch(1.0)),
                AudioField::Pitch => { update(app, Message::AudioPitch(0.0)); update(app, Message::AudioPitchDone); }
                AudioField::Cents => {
                    let Some(clip) = app.project.playlist.clips.iter().find(|c| app.playlist.selected.contains(&c.id) && matches!(c.source, ClipSource::Audio(_))) else { return };
                    update(app, Message::AudioPitch(clip.audio.semitones.trunc()));
                    update(app, Message::AudioPitchDone);
                }
            }
            app.playlist.audio_text = None;
        }
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
            let id = match brush {
                ClipSource::Audio(channel) => {
                    let length = app.audio_seconds(channel)
                        .map_or(app.project.signature.ticks_per_bar(), |seconds| audio_ticks(&app.project.tempo_map(), start as f64, seconds));
                    app.project.add_audio_clip(track, start, channel, length)
                }
                _ => app.project.add_clip(track, start, brush),
            };
            app.playlist.selected = vec![id];
            app.playlist.resizing = None;
            begin_drag(app);
            app.playlist.drag_added = true;
            app.edited();
        }
        Message::Begin { id, edge, additive } => {
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
            app.playlist.resizing = edge;
            begin_drag(app);
        }
        Message::Drag { ticks, tracks, bypass } => {
            let (grid, signature) = (app.project.grid, app.project.signature);
            let delta = timeline::snap_delta(ticks, grid, signature, bypass);
            let min_length = grid.step(signature).filter(|_| !bypass).unwrap_or(10);
            let originals = app.playlist.originals.clone();
            let tempo = app.project.tempo_map();
            let mut preview = Vec::with_capacity(originals.len());
            let resizing = app.playlist.resizing;
            for original in originals {
                // Audio clips end where their file does.
                let file_end = match original.source {
                    ClipSource::Audio(channel) => app.audio_seconds(channel)
                        .map(|seconds| audio_ticks(&tempo, original.start as f64 - original.offset as f64, seconds * original.audio.stretch)),
                    _ => None,
                };
                let mut clip = original.clone();
                if app.playlist.stretch_mode && matches!(original.source, ClipSource::Audio(_)) && resizing.is_some() {
                    let length = match resizing {
                        Some(Edge::End) => (original.length as i64 + delta).max(min_length as i64) as Ticks,
                        _ => (original.length as i64 - delta.clamp(-(original.start as i64), original.length as i64 - 1)).max(min_length as i64) as Ticks,
                    };
                    let ratio = length as f64 / original.length.max(1) as f64;
                    let stretch = original.audio.stretch * ratio;
                    if AudioEdit::STRETCH.contains(&stretch) {
                        clip.audio.stretch = stretch;
                        clip.offset = (original.offset as f64 * ratio).round() as Ticks;
                        clip.length = length;
                        if resizing == Some(Edge::Start) { clip.start = original.end().saturating_sub(length); }
                    }
                    preview.push(clip);
                    continue;
                }
                match resizing {
                    Some(Edge::End) => {
                        let length = (original.length as i64 + delta).max(min_length as i64) as Ticks;
                        clip.length = file_end.map_or(length, |end| length.min(end.saturating_sub(original.offset).max(1)));
                    }
                    Some(Edge::Start) => {
                        // The start cannot go before the song or the source's start.
                        let earliest = -(original.offset.min(original.start) as i64);
                        let latest = (original.length as i64 - min_length as i64).max(0);
                        let delta = delta.clamp(earliest, latest);
                        clip.start = (original.start as i64 + delta) as Ticks;
                        clip.offset = (original.offset as i64 + delta) as Ticks;
                        clip.length = (original.length as i64 - delta) as Ticks;
                    }
                    None => {
                        clip.start = (original.start as i64 + delta).max(0) as Ticks;
                        clip.track = (original.track as i32 + tracks).max(0) as usize;
                    }
                }
                preview.push(clip);
            }
            app.playlist.drag_preview = preview;
        }
        Message::End => {
            let preview = std::mem::take(&mut app.playlist.drag_preview);
            let changed = preview.iter().any(|c| app.project.playlist.clips.iter().any(|old| old.id == c.id && old != c));
            if changed {
                if !app.playlist.drag_added { app.checkpoint(); }
                let tracks = preview.iter().map(|c| c.track + 1).max().unwrap_or(0);
                grow_tracks(app, tracks);
                for clip in preview {
                    if let Some(old) = app.project.playlist.clips.iter_mut().find(|c| c.id == clip.id) { *old = clip; }
                }
                app.edited();
            }
            app.playlist.cancel_drag();
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
                right.audio = clip.audio;
                right.muted = clip.muted;
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
                ClipSource::Audio(channel) => {
                    super::channel_rack::select(app, channel);
                    app.show_panel(daw_model::layout::Panel::ChannelRack);
                }
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
            app.song_start = timeline::marker_tick(tick, app.project.grid, app.project.signature) as f64;
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
        Key::Character(c) if modifiers.command() && c.as_str() == "u" => update(app, Message::MakeUnique),
        Key::Character(c) if modifiers.command() && modifiers.alt() && c.as_str() == "c" => update(app, Message::Consolidate),
        Key::Character(c) if modifiers.is_empty() && c.as_str() == "m" => update(app, Message::MuteSelection),
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
                    new.audio = clip.audio;
                    new.muted = clip.muted;
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
    choices.extend(app.project.channels.iter().filter(|c| matches!(c.source, Source::Audio { .. })).map(|c| BrushChoice {
        source: ClipSource::Audio(c.id),
        name: c.name.clone(),
    }));
    let brush = app.playlist.brush.unwrap_or(ClipSource::Pattern(app.selected_pattern));
    let current = choices.iter().find(|c| c.source == brush).cloned();
    let delete: Element<'_, AppMessage> = match app.playlist.selected_track {
        Some(_) => super::tool("delete track", Message::DeleteTrack.into()),
        None => label(""),
    };
    row![
        tool("unique", Message::MakeUnique.into()),
        tool("consolidate", Message::Consolidate.into()),
        toggle("stretch", app.playlist.stretch_mode, Message::StretchMode.into()),
        super::tool("+ track", Message::AddTrack.into()),
        delete,
        super::tool("+ audio", AppMessage::Action(Action::ImportAudio)),
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
    let canvas = Canvas::new(Arrangement { app, tempo: app.project.tempo_map() }).width(Length::Fill).height(Length::Fill);
    let selected = app.project.playlist.clips.iter().find(|c| app.playlist.selected.contains(&c.id) && matches!(c.source, ClipSource::Audio(_)));
    let Some(clip) = selected else { return canvas.into() };
    let semitones = app.playlist.pitch_edit.as_ref().filter(|(ids, _)| ids.contains(&clip.id))
        .map_or(clip.audio.semitones, |(_, pitch)| *pitch);
    let mut current = AudioText::new(clip);
    current.pitch = semitones.trunc().to_string();
    current.cents = format!("{:.3}", (semitones - semitones.trunc()) * 100.0);
    let draft = app.playlist.audio_text.as_ref().filter(|text| text.clip == clip.id).unwrap_or(&current);
    let audio_input = |placeholder: &str, value: &str, field| text_input(placeholder, value)
        .on_input(move |s| Message::AudioText(field, s).into()).on_submit(Message::AudioApply(field).into())
        .width(72).size(theme::SMALL).padding([3, 6]).style(theme::input);
    let pitch = row![
        label("audio pitch"), slider(AudioEdit::SEMITONES, semitones, |p| Message::AudioPitch(p).into())
            .step(0.01_f32).on_release(Message::AudioPitchDone.into()).width(100).style(theme::fader),
        audio_input("pitch", &draft.pitch, AudioField::Pitch), label("st"),
        audio_input("cents", &draft.cents, AudioField::Cents), label("cents"),
        tool("set pitch", Message::AudioApply(AudioField::Pitch).into()),
        tool("reset pitch", Message::AudioReset(AudioField::Pitch).into()),
        tool("reset cents", Message::AudioReset(AudioField::Cents).into()),
    ].spacing(6).align_y(iced::Alignment::Center);
    let options = row![
        label("duration"), pick(vec![0.125, 0.25, 0.5, 1.0, 2.0, 4.0, 8.0], Some(clip.audio.stretch), |s| Message::AudioStretch(s).into()),
        audio_input("ratio", &draft.stretch, AudioField::Stretch), label("x"),
        tool("set ratio", Message::AudioApply(AudioField::Stretch).into()),
        tool("reset ratio", Message::AudioReset(AudioField::Stretch).into()),
        toggle("reverse", clip.audio.reverse, Message::Reverse.into()),
        tool("mute clips", Message::MuteSelection.into()),
    ].spacing(6).align_y(iced::Alignment::Center);
    let controls = column![pitch, options].spacing(4).padding(4);
    column![canvas, super::scroll(controls, false, true).width(Length::Fill).height(58)].into()
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
