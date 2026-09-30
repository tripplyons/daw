//! Application state, messages, and top-level update and view.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use daw_engine::song::PlayMode;
use daw_model::layout::{Axis, Layout, Panel, Rect, TileId};
use daw_model::time::Ticks;
use daw_model::{ChannelId, ClipSource, InsertId, InstanceId, PatternId, Project, Target};
use daw_plugins::scan::{Catalog, Progress};
use iced::futures::channel::mpsc::{self, UnboundedReceiver};
use iced::futures::{Stream, StreamExt, stream};
use iced::{Point, Size, Subscription, Task, event, keyboard, mouse, window};

use crate::config;
use crate::dialogs;
use crate::keys::{Action, Keymap};
use crate::menu::{self, Menu};
use crate::panels::{automation, browser, channel_rack, mixer, parameters, piano_roll, playlist, settings};
use crate::session::Session;

pub mod audio;
mod closing;
mod history;
mod rendering;
mod saving;
mod targets;
mod view;

use closing::Pending;

/// Project given on the command line, set by `main` before the app starts.
pub static STARTUP_PROJECT: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();

pub const TRANSPORT_HEIGHT: f32 = 64.0;
/// Height of the background job bar under the transport, while it shows.
const JOBS_HEIGHT: f32 = 26.0;
const UNDO_LIMIT: usize = 200;
const LAST_TOUCHED: usize = 16;

#[derive(Debug, Clone)]
pub enum Message {
    Tick,
    CancelAudio,
    RetryAudio,
    CancelRender,
    Key(keyboard::Event, bool),
    WindowResized(Size),
    MouseMoved(Point),
    MousePressed,
    MouseReleased,
    SplitDrag(Vec<bool>),
    SetPanel(TileId, Panel),
    TileAction(TileId, Action),
    SplitTile(TileId, Axis),
    Action(Action),
    SetBpm(String),
    /// Enter in the bpm field, or leaving it: apply the finished tempo.
    BpmDone,
    BpmFocused(bool),
    SelectPattern(PatternId),
    NewPattern,
    ClonePattern(PatternId),
    PackPicked(Option<PathBuf>),
    RecoverPicked(Option<PathBuf>),
    /// Length of the selected pattern, in bars.
    PatternBars(u64),
    ScanProgress(Progress),
    ScanDone(Catalog),
    Rescan,
    /// End of a continuous edit.
    EndEdit,
    Browser(browser::Message),
    Rack(channel_rack::Message),
    PianoRoll(piano_roll::Message),
    Playlist(playlist::Message),
    Mixer(mixer::Message),
    Params(parameters::Message),
    Automation(automation::Message),
    Settings(settings::Message),
    Menu(menu::Message),
    /// Create (or open) an automation clip for a target, from any panel.
    Automate(Target),
    OpenPlugin(InstanceId),
    /// Show or hide a plugin's editor window; plugins without one show their
    /// parameters instead.
    TogglePlugin(InstanceId),
    Opened(Option<PathBuf>),
    /// A WAV file to import as an audio clip at the song start marker.
    ImportAudio(Option<PathBuf>),
    /// A file dropped on the window: a project to open or audio to import.
    Dropped(PathBuf),
    SavedAs(Option<PathBuf>),
    Exported(Option<PathBuf>),
    Screenshot,
    Captured(window::Screenshot),
    /// The window's close button or Cmd+Q.
    CloseRequested,
    /// Answer to the unsaved changes prompt.
    SaveChoice(SaveChoice),
    /// Save As finished for the prompt; then the pending step runs.
    SavedAsThenContinue(Option<PathBuf>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveChoice {
    Save,
    Discard,
    Cancel,
}

pub struct App {
    pub project: Project,
    pub path: Option<PathBuf>,
    pub session: Session,
    pub midi: crate::midi::State,
    pub last_autosave: Instant,
    pub backup_key: String,
    revision: u64,
    autosaved_revision: u64,
    pub catalog: Catalog,
    pub scan: Option<Progress>,
    pub mode: PlayMode,
    pub playing: bool,
    pub position: f64,
    /// Song-mode start marker: play starts here and pausing returns here.
    pub song_start: f64,
    /// Where pattern playback starts, set in the piano roll ruler.
    pub pattern_start: f64,
    pub bind_mode: bool,
    pub record: bool,
    pub split_axis: Axis,
    pub status: String,
    pub window: Size,
    pub cursor: Point,
    dragging_split: Option<Vec<bool>>,
    undo: Vec<history::Snapshot>,
    redo: Vec<history::Snapshot>,
    history: history::State,
    editing: bool,
    pub dirty: bool,
    pub selected_channel: Option<ChannelId>,
    pub selected_pattern: PatternId,
    pub selected_insert: InsertId,
    pub param_instance: Option<InstanceId>,
    pub last_touched: VecDeque<(Target, f32)>,
    pub meters: Vec<(f32, f32)>,
    pub browser: browser::State,
    pub piano_roll: piano_roll::State,
    pub playlist: playlist::State,
    pub automation: automation::State,
    pub params: parameters::State,
    pub settings: settings::State,
    /// The open right-click menu.
    pub menu: Option<Menu>,
    pub keymap: Keymap,
    pub config: config::Config,
    pub config_path: PathBuf,
    /// Problem reading the config file, shown in settings.
    pub config_error: Option<String>,
    screenshot: Option<PathBuf>,
    /// Text in the bpm field until the user commits it.
    bpm_text: Option<String>,
    /// Cmd+Q has been pointed at the window's close request.
    quit_routed: bool,
    /// The unsaved changes prompt is showing, for this next step.
    pending: Option<Pending>,
    saving: saving::State,
    rendering: rendering::State,
}

impl App {
    pub fn boot() -> (Self, Task<Message>) {
        let project = Project::new();
        let selected_pattern = project.patterns[0].id;
        let selected_channel = project.channels.first().map(|c| c.id);
        let selected_insert = project.channels.first().map(|c| c.insert).unwrap_or(daw_model::MASTER);
        let mut app = App {
            project,
            path: None,
            session: Session::new(),
            midi: crate::midi::State::default(),
            last_autosave: Instant::now(),
            backup_key: crate::project_files::stamp(),
            revision: 0,
            autosaved_revision: 0,
            catalog: Catalog::default(),
            scan: Some(Progress { done: 0, total: 0 }),
            mode: PlayMode::Song,
            playing: false,
            position: 0.0,
            song_start: 0.0,
            pattern_start: 0.0,
            bind_mode: false,
            record: false,
            split_axis: Axis::Horizontal,
            status: String::new(),
            window: Size::new(1400.0, 860.0),
            cursor: Point::ORIGIN,
            dragging_split: None,
            undo: Vec::new(),
            redo: Vec::new(),
            history: history::State::default(),
            editing: false,
            dirty: false,
            selected_channel,
            selected_pattern,
            selected_insert,
            param_instance: None,
            last_touched: VecDeque::new(),
            meters: Vec::new(),
            browser: browser::State::default(),
            piano_roll: piano_roll::State::default(),
            playlist: playlist::State::default(),
            automation: automation::State::default(),
            params: parameters::State::default(),
            settings: settings::State::default(),
            menu: None,
            keymap: Keymap::default(),
            config: config::Config::default(),
            config_path: config::path(),
            config_error: None,
            screenshot: std::env::var_os("DAW_SCREENSHOT").map(PathBuf::from),
            bpm_text: None,
            quit_routed: false,
            pending: None,
            saving: saving::State::default(),
            rendering: rendering::State::default(),
        };
        app.load_config();
        if let Some(error) = &app.session.audio_error {
            app.status = format!("no audio output: {error}");
        }
        app.refresh();
        if let Some(path) = STARTUP_PROJECT.get().cloned().flatten() {
            app.open(path);
        }
        if let Err(error) = app.midi.rescan() { app.set_status(format!("MIDI scan failed: {error}")); }
        if let Some(name) = app.config.midi_input.clone() {
            match app.midi.ports.iter().find(|p| p.name == name).cloned() {
                Some(port) => { app.connect_midi(port); }
                None => app.set_status(format!("MIDI input {name} unavailable; select an input in settings")),
            }
        }
        #[cfg(target_os = "macos")]
        forward_plugin_keys();
        if std::env::var_os("DAW_OPEN_EDITORS").is_some() {
            for instance in app.project.plugins.iter().map(|p| p.id).collect::<Vec<_>>() {
                let _ = app.update(Message::OpenPlugin(instance));
            }
        }
        if let Some(workspace) = std::env::var("DAW_WORKSPACE").ok().and_then(|w| w.parse::<usize>().ok()) {
            app.project.workspaces.active = workspace.saturating_sub(1).min(8);
        }
        (app, scan_task())
    }

    pub fn title(&self) -> String {
        let name = self.path.as_ref().and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().into_owned());
        format!("{}{} - DAW", name.unwrap_or_else(|| self.project.name.clone()), if self.dirty { " *" } else { "" })
    }

    pub fn layout(&self) -> &Layout {
        &self.project.workspaces.layouts[self.project.workspaces.active]
    }

    pub fn layout_mut(&mut self) -> &mut Layout {
        let active = self.project.workspaces.active;
        &mut self.project.workspaces.layouts[active]
    }

    pub fn focused_panel(&self) -> Panel {
        let layout = self.layout();
        layout.panel(layout.focused)
    }

    /// Whether the background job bar shows, for audio processing or a render.
    fn showing_jobs(&self) -> bool {
        self.session.audio_progress().is_some() || self.session.preparation_error.is_some() || self.rendering.progress().is_some()
    }

    fn tile_area(&self) -> Rect {
        let top = TRANSPORT_HEIGHT + if self.showing_jobs() { JOBS_HEIGHT } else { 0.0 };
        Rect { x: 0.0, y: top, width: self.window.width, height: (self.window.height - top).max(1.0) }
    }

    fn load_config(&mut self) {
        let (config, error) = config::load(&self.config_path);
        let (keymap, warnings) = Keymap::from_config(&config.keys);
        self.keymap = keymap;
        self.config = config;
        let mut problems: Vec<String> = error.into_iter().collect();
        problems.extend(warnings);
        self.config_error = (!problems.is_empty()).then(|| problems.join("; "));
        if let Some(error) = &self.config_error {
            log::warn!("config: {error}");
            self.status = format!("config: {error}");
        }
    }

    /// Write the key bindings to the config file and report `status`.
    pub fn save_config(&mut self, status: String) {
        self.config.keys = self.keymap.to_config();
        match config::save(&self.config_path, &self.config) {
            Ok(()) => {
                self.config_error = None;
                self.status = status;
            }
            Err(error) => {
                let message = format!("could not save {}: {error}", self.config_path.display());
                self.config_error = Some(message.clone());
                self.status = message;
            }
        }
    }

    /// Checkpoint once at the start of a continuous edit such as a slider drag.
    pub fn begin_edit(&mut self) {
        if !self.editing {
            self.checkpoint();
            self.editing = true;
        }
    }

    fn restore(&mut self, snapshot: history::Snapshot) {
        let mut project = snapshot.project;
        project.workspaces = self.project.workspaces.clone();
        self.project = project;
        self.dirty = true;
        self.revision += 1;
        self.validate_selection();
        self.refresh();
        if let Err(error) = self.session.restore_plugins(&self.project, &snapshot.parameters) {
            self.set_status(format!("could not restore plugin settings: {error}"));
        }
    }

    fn validate_selection(&mut self) {
        // The menu's item may be gone.
        self.menu = None;
        if self.project.pattern(self.selected_pattern).is_none() {
            self.selected_pattern = self.project.patterns[0].id;
        }
        if self.selected_channel.is_some_and(|c| self.project.channel(c).is_none()) {
            self.selected_channel = self.project.channels.first().map(|c| c.id);
        }
        if self.project.mixer.insert(self.selected_insert).is_none() {
            self.selected_insert = daw_model::MASTER;
        }
        if self.automation.clip.is_none_or(|a| self.project.automation_clip(a).is_none()) {
            self.automation.clip = self.project.automation.first().map(|a| a.id);
            self.automation.selected.clear();
        }
        if self.param_instance.is_some_and(|i| self.project.plugin(i).is_none()) {
            self.param_instance = None;
        }
        if let PlayMode::Pattern(p) = self.mode
            && self.project.pattern(p).is_none()
        {
            self.mode = PlayMode::Pattern(self.selected_pattern);
        }
        self.piano_roll.selected.clear();
        self.piano_roll.cancel_drag();
        self.automation.cancel_drag();
        self.playlist.cancel_drag();
        self.playlist.pitch_edit = None;
        self.playlist.audio_text = None;
        self.settings.tail_text = None;
        self.bpm_text = None;
        self.editing = false;
        self.history = history::State::default();
        self.playlist.selected.retain(|id| self.project.playlist.clips.iter().any(|c| c.id == *id));
    }

    /// Bring engine and plugins in line with the project after any edit.
    pub fn refresh(&mut self) {
        self.session.sync(&mut self.project);
        if let Err(error) = self.session.update_song(&self.project, self.mode) {
            self.set_status(format!("audio preparation failed: {error}"));
        }
        self.midi_target();
    }

    /// Mark an edit that already has a checkpoint.
    pub fn edited(&mut self) {
        self.mark_edited();
        self.refresh();
    }

    pub fn mark_edited(&mut self) {
        self.revision += 1;
        self.dirty = true;
    }

    pub fn set_status(&mut self, status: impl Into<String>) {
        self.status = status.into();
    }

    /// Make sure a panel is visible, replacing the focused tile if needed.
    pub fn show_panel(&mut self, panel: Panel) {
        let layout = self.layout_mut();
        if let Some(tile) = layout.find_panel(panel) {
            if layout.zoomed && layout.focused != tile {
                layout.focused = tile;
            }
            return;
        }
        let focused = layout.focused;
        layout.set_panel(focused, panel);
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tick => {
                self.tick();
                self.poll_renders();
                match self.session.poll_preparation(&mut self.project, self.mode) {
                    Some(Err(daw_engine::audio::Error::Cancelled)) => self.set_status("audio processing cancelled; retry audio processing to hear the pending edits"),
                    Some(Err(error)) => self.set_status(format!("audio preparation failed: {error}")),
                    Some(Ok(())) if self.status == "processing audio" => self.set_status("audio ready"),
                    _ => {}
                }
                let saves = self.poll_saves();
                if self.bpm_text.is_none() {
                    return saves;
                }
                return Task::batch([saves, iced::widget::operation::is_focused("tempo").map(Message::BpmFocused)]);
            }
            Message::CancelAudio => self.session.cancel_audio(),
            Message::RetryAudio => { self.refresh(); if self.session.audio_progress().is_some() { self.set_status("processing audio"); } }
            Message::CancelRender => self.rendering.cancel(),
            Message::Key(event, typing) => {
                if let keyboard::Event::KeyReleased { key: keyboard::Key::Named(keyboard::key::Named::ArrowUp | keyboard::key::Named::ArrowDown), .. } = &event {
                    playlist::update(self, playlist::Message::AudioPitchDone);
                    self.finish_edit();
                }
                if self.settings.capturing.is_some() {
                    settings::key(self, &event);
                    return Task::none();
                }
                if self.menu.is_some()
                    && let keyboard::Event::KeyPressed { key: keyboard::Key::Named(keyboard::key::Named::Escape), .. } = &event
                {
                    self.menu = None;
                    return Task::none();
                }
                if self.menu.as_ref().is_some_and(|m| m.rename.is_none())
                    && let keyboard::Event::KeyPressed { key: keyboard::Key::Named(key), .. } = &event
                {
                    use keyboard::key::Named;
                    let message = match key {
                        Named::ArrowDown => Some(menu::Message::Move(1)),
                        Named::ArrowUp => Some(menu::Message::Move(-1)),
                        Named::Enter => Some(menu::Message::Activate),
                        _ => None,
                    };
                    if let Some(message) = message { return menu::update(self, message); }
                }
                if let Some(action) = self.keymap.action(&event, typing) {
                    return self.update(Message::Action(action));
                }
                if let keyboard::Event::KeyPressed { key, modifiers, .. } = &event
                    && !typing
                {
                    self.panel_key(key, *modifiers);
                }
            }
            Message::WindowResized(size) => self.window = size,
            Message::MouseMoved(point) => {
                self.cursor = point;
                if let Some(path) = self.dragging_split.clone() {
                    self.drag_split(&path, point);
                }
            }
            Message::MousePressed => {
                if self.menu.is_some() { return Task::none(); }
                let point = self.cursor;
                let area = self.tile_area();
                let hit = self.layout().rects_with_gap(area, view::GUTTER).into_iter().find(|(_, r)| {
                    point.x >= r.x && point.x < r.x + r.width && point.y >= r.y && point.y < r.y + r.height
                });
                if let Some((tile, _)) = hit {
                    self.layout_mut().focused = tile;
                }
            }
            Message::MouseReleased => {
                self.dragging_split = None;
                self.finish_edit();
            }
            Message::SplitDrag(path) => self.dragging_split = Some(path),
            Message::SetPanel(tile, panel) => {
                self.layout_mut().set_panel(tile, panel);
                self.layout_mut().focused = tile;
            }
            Message::TileAction(tile, action) => {
                if !self.layout().leaves().contains(&tile) { return Task::none(); }
                self.layout_mut().focused = tile;
                return self.action(action);
            }
            Message::SplitTile(tile, axis) => {
                if !self.layout().leaves().contains(&tile) { return Task::none(); }
                self.layout_mut().focused = tile;
                self.split_axis = axis;
                return self.action(Action::Split);
            }
            Message::Action(action) => return self.action(action),
            Message::SetBpm(value) => self.bpm_text = Some(value),
            Message::BpmFocused(false) => return self.update(Message::BpmDone),
            Message::BpmFocused(true) => {}
            Message::BpmDone => {
                if let Some(value) = self.bpm_text.take()
                    && let Ok(bpm) = value.trim().parse::<f64>()
                    && Project::BPM.contains(&bpm) && bpm != self.project.bpm {
                    self.checkpoint();
                    self.project.bpm = bpm;
                    self.edited();
                }
            }
            Message::SelectPattern(id) => {
                if id != self.selected_pattern {
                    self.pattern_start = 0.0;
                }
                self.selected_pattern = id;
                self.piano_roll.selected.clear();
                if let PlayMode::Pattern(_) = self.mode {
                    self.mode = PlayMode::Pattern(id);
                    self.refresh();
                }
                self.playlist.brush = Some(ClipSource::Pattern(id));
            }
            Message::PatternBars(bars) => {
                self.checkpoint();
                let bar = self.project.signature.ticks_per_bar();
                self.project.set_pattern_length(self.selected_pattern, bars.max(1) * bar);
                self.edited();
            }
            Message::ClonePattern(source) => {
                if self.project.pattern(source).is_none() { return Task::none(); }
                self.checkpoint();
                let id = self.project.clone_pattern(source).expect("existing pattern");
                self.edited();
                return self.update(Message::SelectPattern(id));
            }
            Message::PackPicked(Some(path)) => {
                let path = if path.extension().is_none() { path.with_extension("dawzip") } else { path };
                self.pack(path);
            }
            Message::NewPattern => {
                self.checkpoint();
                let id = self.project.add_pattern();
                self.edited();
                return self.update(Message::SelectPattern(id));
            }
            Message::ScanProgress(progress) => self.scan = Some(progress),
            Message::ScanDone(catalog) => {
                self.scan = None;
                let failed = catalog.failed.len();
                self.set_status(format!("{} plugins, {failed} failed", catalog.plugins.len()));
                self.catalog = catalog;
            }
            Message::Rescan => {
                if self.scan.is_none() {
                    self.scan = Some(Progress { done: 0, total: 0 });
                    return scan_task();
                }
            }
            Message::EndEdit => {
                playlist::update(self, playlist::Message::AudioPitchDone);
                self.finish_edit();
            }
            Message::Browser(message) => return browser::update(self, message),
            Message::Rack(message) => return channel_rack::update(self, message),
            Message::PianoRoll(message) => piano_roll::update(self, message),
            Message::Playlist(message) => playlist::update(self, message),
            Message::Mixer(message) => return mixer::update(self, message),
            Message::Params(message) => parameters::update(self, message),
            Message::Automation(message) => automation::update(self, message),
            Message::Settings(message) => settings::update(self, message),
            Message::Menu(message) => return menu::update(self, message),
            Message::Automate(target) => {
                self.checkpoint();
                self.bind(target);
            }
            Message::OpenPlugin(instance) => {
                self.param_instance = Some(instance);
                if !self.session.has_editor(instance) {
                    self.show_panel(Panel::Parameters);
                    return Task::none();
                }
                let title = self.project.plugin(instance).map(|p| p.plugin.name.clone()).unwrap_or_default();
                if let Err(error) = self.session.open_editor(instance, &title) {
                    self.set_status(error);
                    self.show_panel(Panel::Parameters);
                }
            }
            Message::TogglePlugin(instance) => {
                self.param_instance = Some(instance);
                if !self.session.has_editor(instance) {
                    self.show_panel(Panel::Parameters);
                    return Task::none();
                }
                let title = self.project.plugin(instance).map(|p| p.plugin.name.clone()).unwrap_or_default();
                if let Err(error) = self.session.toggle_editor(instance, &title) {
                    self.set_status(error);
                    self.show_panel(Panel::Parameters);
                }
            }
            Message::Opened(Some(path)) => return self.guard(Pending::Open(path)),
            Message::ImportAudio(Some(path)) => {
                if let Err(error) = self.import_audio(&path, self.song_start.max(0.0).round() as Ticks) {
                    self.set_status(error);
                }
            }
            Message::Dropped(path) => {
                let extension = path.extension().map(|e| e.to_string_lossy().to_lowercase());
                return match extension.as_deref() {
                    Some("wav" | "wave") => self.update(Message::ImportAudio(Some(path))),
                    Some("dawproj" | "dawzip") => self.guard(Pending::Open(path)),
                    _ => {
                        self.set_status(format!("cannot open {}; drop a .wav, .dawproj, or .dawzip file", path.display()));
                        Task::none()
                    }
                };
            }
            Message::RecoverPicked(Some(path)) => return self.guard(Pending::Recover(path)),
            Message::SavedAs(Some(path)) | Message::SavedAsThenContinue(Some(path)) => {
                self.path = Some(path);
                self.save();
            }
            Message::Screenshot => {
                return window::latest().and_then(window::screenshot).map(Message::Captured);
            }
            Message::Captured(shot) => {
                for instance in self.project.plugins.iter().map(|p| (p.id, p.plugin.name.clone())).collect::<Vec<_>>() {
                    let open = self.session.editor_open(instance.0);
                    log::info!("editor {}: {}", instance.1, if open { "open" } else { "closed" });
                }
                if let Some(path) = self.screenshot.take() {
                    if let Err(error) = crate::capture::write_bmp(&path, &shot) {
                        log::error!("screenshot failed: {error}");
                    }
                    return iced::exit();
                }
            }
            Message::CloseRequested => {
                log::info!("close requested (unsaved changes: {})", self.dirty);
                return self.guard(Pending::Close);
            }
            Message::SaveChoice(SaveChoice::Save) => {
                if self.path.is_none() {
                    return save_as(Message::SavedAsThenContinue);
                }
                // `save_finished` runs the pending step once the save works.
                self.save();
            }
            Message::SaveChoice(SaveChoice::Discard) => {
                self.saving.cancel_wait();
                if let Some(next) = self.pending.take() {
                    return self.proceed(next);
                }
            }
            Message::SaveChoice(SaveChoice::Cancel) | Message::SavedAsThenContinue(None) => {
                self.pending = None;
                self.saving.cancel_wait();
            }
            Message::Exported(Some(path)) => {
                let path = if path.extension().is_none() { path.with_extension("wav") } else { path };
                match self.export(path) {
                    Ok(()) => self.set_status("exporting audio"),
                    Err(error) => self.set_status(format!("export failed: {error}")),
                }
            }
            Message::Opened(None)
            | Message::ImportAudio(None)
            | Message::PackPicked(None)
            | Message::RecoverPicked(None)
            | Message::SavedAs(None)
            | Message::Exported(None) => {}
        }
        Task::none()
    }

    /// Keys for the focused panel, after global bindings.
    fn panel_key(&mut self, key: &keyboard::Key, modifiers: keyboard::Modifiers) {
        match self.focused_panel() {
            Panel::PianoRoll => piano_roll::key(self, key, modifiers),
            Panel::Playlist => playlist::key(self, key, modifiers),
            Panel::Automation => automation::key(self, key, modifiers),
            _ => false,
        };
    }

    fn tick(&mut self) {
        if !self.quit_routed {
            self.quit_routed = crate::quit::route_to_close();
        }
        let shared = self.session.shared();
        self.position = shared.position();
        let playing = shared.playing();
        let stopped = self.playing && !playing;
        self.playing = playing;
        self.meters = (0..self.project.mixer.inserts.len()).map(|i| shared.take_peak(i)).collect();
        if stopped {
            automation::stopped(self);
        }
        for (instance, touch) in self.session.poll() {
            self.plugin_touch(instance, touch);
        }
        self.finish_idle_plugin_edits();
        self.collect_takes();
        self.poll_midi();
        if stopped { self.finish_midi(); }
        let minutes = self.config.autosave_minutes;
        if minutes > 0 && self.dirty && self.revision != self.autosaved_revision
            && self.last_autosave.elapsed() >= Duration::from_secs(minutes.saturating_mul(60)) {
            self.autosave();
        }
    }

    fn drag_split(&mut self, path: &[bool], point: Point) {
        let area = self.tile_area();
        let Some((_, axis, rect)) = self.layout().splits_with_gap(area, view::GUTTER).into_iter().find(|(p, _, _)| p == path) else { return };
        let ratio = match axis {
            Axis::Horizontal => (point.x - rect.x - view::GUTTER / 2.0) / (rect.width - view::GUTTER).max(1.0),
            Axis::Vertical => (point.y - rect.y - view::GUTTER / 2.0) / (rect.height - view::GUTTER).max(1.0),
        };
        self.layout_mut().set_split_ratio(path, ratio);
    }

    fn action(&mut self, action: Action) -> Task<Message> {
        self.menu = None;
        match action {
            Action::Focus(d) => {
                self.layout_mut().focus(d);
            }
            Action::Swap(d) => {
                self.layout_mut().swap(d);
            }
            Action::Resize(d) => {
                self.layout_mut().resize(d, 0.04);
            }
            Action::SplitAxis(axis) => {
                self.split_axis = axis;
                self.set_status(match axis {
                    Axis::Horizontal => "next split: side by side",
                    Axis::Vertical => "next split: stacked",
                });
            }
            Action::Split => {
                let axis = self.split_axis;
                let visible: Vec<Panel> = self.layout().leaves().iter().map(|&t| self.layout().panel(t)).collect();
                let panel = Panel::ALL.into_iter().find(|p| !visible.contains(p)).unwrap_or(Panel::Playlist);
                self.layout_mut().split(axis, panel);
            }
            Action::Close => {
                self.layout_mut().close();
            }
            Action::CyclePanel => {
                let layout = self.layout_mut();
                let focused = layout.focused;
                let current = layout.panel(focused);
                let index = Panel::ALL.iter().position(|&p| p == current).unwrap_or(0);
                layout.set_panel(focused, Panel::ALL[(index + 1) % Panel::ALL.len()]);
            }
            Action::PanelTools | Action::ContextMenu => {
                let tile = self.layout().focused;
                let item = if action == Action::PanelTools { menu::Item::Tools(tile) } else {
                    match self.focused_panel() {
                        Panel::Playlist => self.playlist.selected.first().copied().map(menu::Item::Clip)
                            .or_else(|| self.playlist.selected_track.map(menu::Item::Track)),
                        Panel::ChannelRack => self.selected_channel.map(menu::Item::Channel),
                        Panel::Mixer => Some(menu::Item::Insert(self.selected_insert)),
                        Panel::PianoRoll => Some(menu::Item::Pattern(self.selected_pattern)),
                        Panel::Automation => self.automation.clip.map(menu::Item::Automation),
                        _ => None,
                    }.unwrap_or(menu::Item::Tools(tile))
                };
                let rect = self.layout().rects_with_gap(self.tile_area(), view::GUTTER).into_iter().find(|(id, _)| *id == tile).unwrap().1;
                let at = Point::new(rect.x + 8.0, rect.y + crate::theme::HEADER_HEIGHT + 2.0);
                return menu::update(self, menu::Message::OpenAt(item, at));
            }
            Action::Zoom => self.layout_mut().toggle_zoom(),
            Action::Workspace(n) => {
                if n < self.project.workspaces.layouts.len() {
                    self.project.workspaces.active = n;
                    self.set_status(format!("workspace {}", n + 1));
                }
            }
            Action::ToggleBind => {
                self.bind_mode = !self.bind_mode;
                self.set_status(if self.bind_mode {
                    "bind mode: move any parameter to create its automation clip"
                } else {
                    "bind mode off"
                });
            }
            Action::ToggleRecord => self.record = !self.record,
            Action::ToggleMidiRecord => self.toggle_midi_recording(),
            Action::ToggleAudioRecord => self.toggle_audio_recording(),
            Action::ImportAudio => return Task::perform(dialogs::open("wav", &["wav", "wave"], None), Message::ImportAudio),
            Action::Delete => {
                let deleted = match self.focused_panel() {
                    Panel::ChannelRack => channel_rack::delete_selected(self),
                    Panel::Mixer => mixer::delete_selected(self),
                    Panel::Playlist => playlist::delete_selected(self),
                    Panel::PianoRoll => piano_roll::delete_selected(self),
                    Panel::Automation => automation::delete_selected(self),
                    _ => false,
                };
                if !deleted {
                    self.set_status(format!("nothing selected to delete in {}", self.focused_panel()));
                }
            }
            Action::Settings => {
                let layout = self.layout_mut();
                match layout.find_panel(Panel::Settings) {
                    Some(tile) => layout.focused = tile,
                    None => {
                        let tile = layout.focused;
                        layout.set_panel(tile, Panel::Settings);
                    }
                }
            }
            Action::ToggleMode => {
                self.poll_midi();
                self.finish_midi();
                self.mode = match self.mode {
                    PlayMode::Song => {
                        self.stop_audio_recording();
                        PlayMode::Pattern(self.selected_pattern)
                    }
                    PlayMode::Pattern(_) => PlayMode::Song,
                };
                self.refresh();
                self.return_to_start();
            }
            Action::PlayPause => {
                if self.playing { self.poll_midi(); self.finish_midi(); }
                // Play starts at the pattern start or the song marker, and
                // pausing goes back there, so the marker never drifts.
                let was_playing = self.playing;
                if was_playing {
                    self.session.stop();
                }
                self.return_to_start();
                if !was_playing {
                    self.session.play();
                }
                self.playing = !was_playing;
            }
            Action::Stop => {
                self.poll_midi();
                self.finish_midi();
                self.session.stop();
                self.playing = false;
                self.song_start = self.project.playlist.loop_range.map(|(s, _)| s as f64).unwrap_or(0.0);
                self.pattern_start = 0.0;
                self.return_to_start();
            }
            Action::Undo => {
                self.finish_edit();
                self.finish_plugin_edits();
                if let Some(previous) = self.undo.pop() {
                    let current = self.snapshot(None);
                    self.redo.push(current);
                    self.restore(previous);
                }
            }
            Action::Redo => {
                self.finish_edit();
                self.finish_plugin_edits();
                if let Some(next) = self.redo.pop() {
                    let current = self.snapshot(None);
                    self.undo.push(current);
                    let redo = std::mem::take(&mut self.redo);
                    self.restore(next);
                    self.redo = redo;
                }
            }
            Action::New => return self.guard(Pending::New),
            Action::Open => return Task::perform(dialogs::open("project", &["dawproj", "dawzip"], None), Message::Opened),
            Action::Save if self.path.is_some() => self.save(),
            Action::Save | Action::SaveAs => return save_as(Message::SavedAs),
            Action::Pack => return Task::perform(dialogs::save("portable project", &["dawzip"], "project.dawzip"), Message::PackPicked),
            Action::Recover => {
                let backups = crate::project_files::backups_dir();
                return Task::perform(dialogs::open("backup", &["dawproj"], Some(backups)), Message::RecoverPicked);
            }
            Action::Export => return Task::perform(dialogs::save("wav", &["wav"], "export.wav"), Message::Exported),
        }
        Task::none()
    }

    /// Move the playhead to where playback starts: the pattern start, or the
    /// song marker.
    pub fn return_to_start(&mut self) {
        let start = match self.mode {
            // A marker past a shortened pattern's end starts at the top.
            PlayMode::Pattern(id) => {
                let length = self.project.pattern(id).map_or(0.0, |p| p.length as f64);
                if self.pattern_start < length { self.pattern_start } else { 0.0 }
            }
            PlayMode::Song => self.song_start,
        };
        self.session.seek(start);
        self.position = start;
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let events = event::listen_with(|event, status, _| match event {
            iced::Event::Keyboard(event) => Some(Message::Key(event, status == event::Status::Captured)),
            iced::Event::Window(window::Event::Resized(size)) => Some(Message::WindowResized(size)),
            iced::Event::Window(window::Event::Opened { size, .. }) => Some(Message::WindowResized(size)),
            iced::Event::Window(window::Event::FileDropped(path)) => Some(Message::Dropped(path)),
            iced::Event::Mouse(mouse::Event::CursorMoved { position }) => Some(Message::MouseMoved(position)),
            iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => Some(Message::MouseReleased),
            // Wheel steps do not emit the slider's release message.
            iced::Event::Mouse(mouse::Event::WheelScrolled { .. }) => Some(Message::EndEdit),
            _ => None,
        });
        // Presses are listened to raw so a click focuses its tile even when a
        // widget inside the tile handles it.
        let presses = event::listen_raw(|event, _, _| match event {
            iced::Event::Mouse(mouse::Event::ButtonPressed(_)) => Some(Message::MousePressed),
            _ => None,
        });
        let tick = iced::time::every(Duration::from_millis(33)).map(|_| Message::Tick);
        let close = window::close_requests().map(|_| Message::CloseRequested);
        let plugin_keys = Subscription::run(plugin_keys);
        let opened = Subscription::run(opened_files);
        let mut subscriptions = vec![events, presses, tick, close, plugin_keys, opened];
        if self.screenshot.is_some() {
            let delay = std::env::var("DAW_SCREENSHOT_DELAY").ok().and_then(|d| d.parse().ok()).unwrap_or(8);
            subscriptions.push(iced::time::every(Duration::from_secs(delay)).map(|_| Message::Screenshot));
        }
        Subscription::batch(subscriptions)
    }
}

/// Keys from plugin editor windows, waiting for the subscription to take them.
static PLUGIN_KEYS: Mutex<Option<UnboundedReceiver<keyboard::Event>>> = Mutex::new(None);

/// Send keys that plugin editors do not use to the app's key bindings, so
/// Space plays while a plugin window is in front. Call on the main thread.
#[cfg(target_os = "macos")]
fn forward_plugin_keys() {
    let (sender, receiver) = mpsc::unbounded();
    *PLUGIN_KEYS.lock().unwrap() = Some(receiver);
    daw_plugins::set_unhandled_keys(move |press| {
        let Some(event) = crate::keys::plugin_key_event(press) else { return false };
        sender.unbounded_send(event).is_ok()
    });
}

/// Projects opened from Finder, loaded the same way as Cmd+O.
fn opened_files() -> impl Stream<Item = Message> {
    stream::iter(crate::open_files::take()).flatten().map(|path| Message::Opened(Some(path)))
}

fn plugin_keys() -> impl Stream<Item = Message> {
    let receiver = PLUGIN_KEYS.lock().unwrap().take();
    stream::iter(receiver).flatten().map(|event| Message::Key(event, false))
}

/// Ask where to save the project, then send the answer as `then`.
fn save_as(then: fn(Option<PathBuf>) -> Message) -> Task<Message> {
    Task::perform(dialogs::save("project", &["dawproj"], "untitled.dawproj"), then)
}

fn scan_task() -> Task<Message> {
    let (sender, receiver) = mpsc::unbounded();
    std::thread::spawn(move || {
        let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("daw"));
        let progress_sender = sender.clone();
        let catalog = daw_plugins::scan::scan(&exe, &daw_plugins::scan::cache_path(), move |p| {
            let _ = progress_sender.unbounded_send(Message::ScanProgress(p));
        });
        let _ = sender.unbounded_send(Message::ScanDone(catalog));
    });
    Task::stream(receiver)
}

#[cfg(test)]
mod tests;
