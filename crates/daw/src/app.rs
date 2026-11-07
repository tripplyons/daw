//! Application state, messages, and top-level update and view.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use daw_engine::song::PlayMode;
use daw_model::layout::{Axis, Layout, Panel, Rect, TileId};
use daw_model::time::Ticks;
use daw_model::{ChannelId, ClipSource, InsertId, InstanceId, PatternId, Project, Target};
use daw_plugins::scan::{Catalog, Progress, ScanMode};
use crate::config;
use crate::input::{self as keyboard, Point, Size};
use crate::task::Task;
use crate::keys::{Action, Keymap};
use crate::menu::{self, Menu};
use crate::panels::{automation, browser, channel_rack, mixer, parameters, piano_roll, playlist, settings};
use crate::session::Session;

pub mod audio;
mod closing;
mod computer_keyboard;
mod history;
mod rendering;
mod saving;
pub mod scenes;
mod targets;
mod view;
pub(crate) use view::modal;

use closing::Pending;

/// Project given on the command line, set by `main` before the app starts.
pub static STARTUP_PROJECT: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();

pub const TRANSPORT_HEIGHT: f32 = 52.0;
/// Height of the background job bar under the transport, while it shows.
const JOBS_HEIGHT: f32 = 34.0;
/// Below this window width, one tile shows at a time under a tab strip.
pub const COMPACT_WIDTH: f32 = 1000.0;
const UNDO_LIMIT: usize = 200;
const LAST_TOUCHED: usize = 16;

/// A status message over the workspace. Errors stay until dismissed;
/// other messages fade after a few seconds.
#[derive(Debug, Clone)]
pub struct Toast {
    pub text: String,
    pub error: bool,
    pub action: Option<(&'static str, Message)>,
    at: Instant,
}

impl Toast {
    const SHOWN: Duration = Duration::from_secs(4);
    /// Longer, so there is time to use the button.
    const SHOWN_WITH_ACTION: Duration = Duration::from_secs(8);
    const FADE: Duration = Duration::from_millis(400);

    fn new(text: &str, action: Option<(&'static str, Message)>) -> Self {
        // Status text is plain sentences; failures use one of these words.
        let lower = text.to_lowercase();
        let error = ["fail", "could not", "cannot", "error", "no audio output", "config:"].iter().any(|w| lower.contains(w));
        Toast { text: text.to_string(), error, action, at: Instant::now() }
    }

    /// Opacity now: 1 while shown, falling to 0 over the fade. Errors stay
    /// until dismissed or replaced.
    pub fn opacity(&self) -> f32 {
        if self.error { return 1.0; }
        let shown = if self.action.is_some() { Self::SHOWN_WITH_ACTION } else { Self::SHOWN };
        let left = shown.saturating_sub(self.at.elapsed());
        (left.as_secs_f32() / Self::FADE.as_secs_f32()).min(1.0)
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    Tick,
    DismissToast,
    /// Bring a panel into view, replacing the focused tile's panel if no tile shows it.
    ShowPanel(Panel),
    Palette(crate::palette::Message),
    CancelAudio,
    RetryAudio,
    CancelRender,
    Key(keyboard::Event, bool),
    PluginKey(keyboard::Event),
    PluginNote(keyboard::Event),
    WindowUnfocused,
    WindowResized(Size),
    MouseMoved(Point),
    MousePressed,
    MouseReleased,
    SplitDrag(Vec<bool>),
    SetPanel(TileId, Panel),
    /// Focus a tile; in a narrow window this shows it.
    FocusTile(TileId),
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
    /// UI state that is not part of the project: dropdowns, text fields.
    Ui(crate::ui::Msg),
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
    pub computer_keyboard: computer_keyboard::State,
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
    /// The last status message, shown over the workspace until it expires.
    pub toast: Option<Toast>,
    /// The command palette, while open.
    pub palette: Option<crate::palette::Palette>,
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
    /// Text fields, the open dropdown, and canvas state.
    pub ui: crate::ui::State,
    /// Text in the bpm field until the user commits it.
    bpm_text: Option<String>,
    /// The unsaved changes prompt is showing, or a save is finishing, for this next step.
    pending: Option<Pending>,
    /// The unsaved changes prompt is showing.
    asking: bool,
    saving: saving::State,
    rendering: rendering::State,
}

impl App {
    pub fn boot() -> (Self, Task) {
        let project = Project::new();
        let selected_pattern = project.patterns[0].id;
        let selected_channel = project.channels.first().map(|c| c.id);
        let selected_insert = project.channels.first().map(|c| c.insert).unwrap_or(daw_model::MASTER);
        let mut app = App {
            project,
            path: None,
            session: Session::new(),
            midi: crate::midi::State::default(),
            computer_keyboard: computer_keyboard::State::default(),
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
            toast: None,
            palette: None,
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
            ui: crate::ui::State::default(),
            bpm_text: None,
            pending: None,
            asking: false,
            saving: saving::State::default(),
            rendering: rendering::State::default(),
        };
        app.load_config();
        if let Some(error) = &app.session.audio_error {
            app.set_status(format!("no audio output: {error}"));
        }
        app.refresh();
        if let Some(path) = STARTUP_PROJECT.get().cloned().flatten() {
            app.open(path);
        }
        #[cfg(not(test))]
        {
            if let Err(error) = app.midi.rescan() { app.set_status(format!("MIDI scan failed: {error}")); }
            if let Some(name) = app.config.midi_input.clone() {
                match app.midi.ports.iter().find(|p| p.name == name).cloned() {
                    Some(port) => { app.connect_midi(port); }
                    None => app.set_status(format!("MIDI input {name} unavailable; select an input in settings")),
                }
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
        (app, scan_task(ScanMode::Changed))
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

    /// Width of the tile showing `panel`, if one is on screen.
    pub fn panel_width(&self, panel: Panel) -> Option<f32> {
        let tile = self.layout().find_panel(panel)?;
        self.tile_rects().into_iter().find(|(id, _)| *id == tile).map(|(_, r)| r.width)
    }

    fn tile_area(&self) -> Rect {
        let tabs = if self.compact() { crate::theme::HEADER_HEIGHT } else { 0.0 };
        let top = TRANSPORT_HEIGHT + tabs + if self.showing_jobs() { JOBS_HEIGHT } else { 0.0 };
        // Tiles keep a gutter from the window edges and the bars above them, as between each other.
        let gap = view::GUTTER;
        Rect { x: gap, y: top + gap, width: (self.window.width - 2.0 * gap).max(1.0), height: (self.window.height - top - 2.0 * gap).max(1.0) }
    }

    /// Whether a tile showing `panel` is on screen.
    pub fn panel_visible(&self, panel: Panel) -> bool {
        self.tile_rects().iter().any(|(tile, _)| self.layout().panel(*tile) == panel)
    }

    /// Whether the window is too narrow for side-by-side tiles.
    pub fn compact(&self) -> bool {
        self.window.width < COMPACT_WIDTH && self.layout().leaves().len() > 1
    }

    /// The tiles on screen and where they are, with gaps for split lines.
    fn tile_rects(&self) -> Vec<(TileId, Rect)> {
        let area = self.tile_area();
        if self.compact() { return vec![(self.layout().focused, area)]; }
        self.layout().rects_with_gap(area, view::GUTTER)
    }

    /// Split lines the mouse can drag; none while one tile shows.
    fn split_handles(&self) -> Vec<daw_model::layout::Split> {
        if self.compact() { return Vec::new(); }
        self.layout().splits_with_gap(self.tile_area(), view::GUTTER)
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
            self.set_status(format!("config: {error}"));
        }
    }

    /// Write the key bindings to the config file and report `status`.
    pub fn save_config(&mut self, status: String) {
        self.config.keys = self.keymap.to_config();
        crate::native_menu::refresh(self);
        match config::save(&self.config_path, &self.config) {
            Ok(()) => {
                self.config_error = None;
                self.set_status(status);
            }
            Err(error) => {
                let message = format!("could not save {}: {error}", self.config_path.display());
                self.config_error = Some(message.clone());
                self.set_status(message);
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
        self.toast = Some(Toast::new(&self.status, None));
    }

    /// Report a result with a button that follows it up, such as showing
    /// the panel where an added plugin went.
    pub fn set_status_with(&mut self, status: impl Into<String>, label: &'static str, action: Message) {
        self.status = status.into();
        self.toast = Some(Toast::new(&self.status, Some((label, action))));
    }

    /// Make sure a panel is visible, replacing the focused tile if needed.
    pub fn show_panel(&mut self, panel: Panel) {
        let compact = self.compact();
        let layout = self.layout_mut();
        if let Some(tile) = layout.find_panel(panel) {
            if (layout.zoomed || compact) && layout.focused != tile {
                layout.focused = tile;
            }
            return;
        }
        let focused = layout.focused;
        layout.set_panel(focused, panel);
    }

    /// Share shortcut dispatch; plugin musical typing has its own priority path.
    fn key_event(&mut self, event: keyboard::Event, typing: bool, main_window: bool) -> Task {
        if matches!(&event, keyboard::Event::KeyReleased { .. })
            && self.computer_key(&event, typing) { return Task::none(); }
        if let keyboard::Event::KeyReleased { key: keyboard::Key::Named(keyboard::key::Named::ArrowUp | keyboard::key::Named::ArrowDown), .. } = &event {
            playlist::update(self, playlist::Message::AudioPitchDone);
            self.finish_edit();
        }
        if self.settings.capturing.is_some() {
            settings::key(self, &event);
            return Task::none();
        }
        if self.palette.is_some()
            && let keyboard::Event::KeyPressed { key: keyboard::Key::Named(key), .. } = &event
        {
            use keyboard::key::Named;
            let message = match key {
                Named::Escape => Some(crate::palette::Message::Close),
                Named::ArrowDown => Some(crate::palette::Message::Move(1)),
                Named::ArrowUp => Some(crate::palette::Message::Move(-1)),
                _ => None,
            };
            if let Some(message) = message { return crate::palette::update(self, message); }
        }
        if self.settings.open && self.menu.is_none()
            && let keyboard::Event::KeyPressed { key: keyboard::Key::Named(keyboard::key::Named::Escape), .. } = &event
        {
            self.settings.open = false;
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
        if main_window && self.computer_key(&event, typing) { return Task::none(); }
        if let keyboard::Event::KeyPressed { key, modifiers, .. } = &event
            && !typing
        {
            self.panel_key(key, *modifiers);
        }
        Task::none()
    }

    pub fn update(&mut self, message: Message) -> Task {
        match message {
            Message::Ui(message) => return crate::ui::update(self, message),
            Message::DismissToast => self.toast = None,
            Message::Palette(message) => return crate::palette::update(self, message),
            Message::ShowPanel(panel) => {
                self.toast = None;
                self.show_panel(panel);
            }
            Message::Tick => {
                if self.toast.as_ref().is_some_and(|t| t.opacity() <= 0.0) { self.toast = None; }
                self.tick();
                self.poll_renders();
                match self.session.poll_preparation(&mut self.project, self.mode) {
                    Some(Err(daw_engine::audio::Error::Cancelled)) => self.set_status("audio processing cancelled; retry audio processing to hear the pending edits"),
                    Some(Err(error)) => self.set_status(format!("audio preparation failed: {error}")),
                    Some(Ok(())) if self.status == "processing audio" => self.set_status("audio ready"),
                    _ => {}
                }
                return self.poll_saves();
            }
            Message::CancelAudio => self.session.cancel_audio(),
            Message::RetryAudio => { self.refresh(); if self.session.audio_progress().is_some() { self.set_status("processing audio"); } }
            Message::CancelRender => self.rendering.cancel(),
            Message::PluginKey(event) => return self.key_event(event, false, false),
            Message::PluginNote(event) => { self.computer_key(&event, false); }
            Message::WindowUnfocused => self.release_computer_keys(),
            Message::Key(event, typing) => return self.key_event(event, typing, true),
            Message::WindowResized(size) => {
                self.window = size;
                crate::native_menu::refresh(self);
            }
            Message::MouseMoved(point) => {
                self.cursor = point;
                browser::moved(self, point);
                if let Some(path) = self.dragging_split.clone() {
                    self.drag_split(&path, point);
                }
            }
            Message::MousePressed => {
                if self.menu.is_some() { return Task::none(); }
                let point = self.cursor;
                let contains = |r: Rect| point.x >= r.x && point.x < r.x + r.width && point.y >= r.y && point.y < r.y + r.height;
                if self.split_handles().iter().any(|s| contains(view::handle_rect(s))) { return Task::none(); }
                let hit = self.tile_rects().into_iter().find(|(_, r)| contains(*r));
                if let Some((tile, _)) = hit {
                    self.layout_mut().focused = tile;
                }
            }
            Message::MouseReleased => {
                self.dragging_split = None;
                self.finish_edit();
                return browser::released(self);
            }
            Message::SplitDrag(path) => self.dragging_split = Some(path),
            Message::SetPanel(tile, panel) => {
                self.layout_mut().set_panel(tile, panel);
                self.layout_mut().focused = tile;
            }
            Message::FocusTile(tile) => {
                if self.layout().leaves().contains(&tile) { self.layout_mut().focused = tile; }
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
                    return scan_task(ScanMode::RetryFailed);
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
            Message::CloseRequested => {
                log::info!("close requested (unsaved changes: {})", self.dirty);
                return self.guard(Pending::Close);
            }
            Message::SaveChoice(SaveChoice::Save) => {
                self.asking = false;
                if self.path.is_none() {
                    return save_as(Message::SavedAsThenContinue);
                }
                // `save_finished` runs the pending step once the save works.
                self.save();
            }
            Message::SaveChoice(SaveChoice::Discard) => {
                self.asking = false;
                self.saving.cancel_wait();
                if let Some(next) = self.pending.take() {
                    return self.proceed(next);
                }
            }
            Message::SaveChoice(SaveChoice::Cancel) | Message::SavedAsThenContinue(None) => {
                self.asking = false;
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
        let Some(split) = self.split_handles().into_iter().find(|s| s.path == path) else { return };
        let rect = split.area;
        let ratio = match split.axis {
            Axis::Horizontal => (point.x - rect.x - view::GUTTER / 2.0) / (rect.width - view::GUTTER).max(1.0),
            Axis::Vertical => (point.y - rect.y - view::GUTTER / 2.0) / (rect.height - view::GUTTER).max(1.0),
        };
        self.layout_mut().set_split_ratio(path, ratio);
    }

    fn action(&mut self, action: Action) -> Task {
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
                let rect = self.tile_rects().into_iter().find(|(id, _)| *id == tile).map_or(self.tile_area(), |(_, r)| r);
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
            Action::ToggleComputerKeyboard => self.toggle_computer_keyboard(),
            Action::ToggleAudioRecord => self.toggle_audio_recording(),
            Action::ImportAudio => return Task::open("WAV audio", &["wav", "wave"], None, Message::ImportAudio),
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
            // Settings opens over the workspace and leaves the tiles alone.
            Action::Palette => return crate::palette::open(self),
            Action::Settings => {
                self.settings.open = !self.settings.open;
                self.settings.capturing = None;
                self.menu = None;
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
                match self.undo.pop() {
                    Some(previous) => {
                        let current = self.snapshot(None);
                        let change = history::describe(&previous.project, &current.project);
                        self.redo.push(current);
                        self.set_status(change.map_or("undo".to_string(), |c| format!("undo: {c}")));
                        self.restore(previous);
                    }
                    None => self.set_status("nothing to undo"),
                }
            }
            Action::Redo => {
                self.finish_edit();
                self.finish_plugin_edits();
                match self.redo.pop() {
                    Some(next) => {
                        let current = self.snapshot(None);
                        let change = history::describe(&current.project, &next.project);
                        self.undo.push(current);
                        let redo = std::mem::take(&mut self.redo);
                        self.set_status(change.map_or("redo".to_string(), |c| format!("redo: {c}")));
                        self.restore(next);
                        self.redo = redo;
                    }
                    None => self.set_status("nothing to redo"),
                }
            }
            Action::New => return self.guard(Pending::New),
            Action::Open => return Task::open("project", &["dawproj", "dawzip"], None, Message::Opened),
            Action::Save if self.path.is_some() => self.save(),
            Action::Save | Action::SaveAs => return save_as(Message::SavedAs),
            Action::Pack => return Task::save("portable project", &["dawzip"], "project.dawzip", Message::PackPicked),
            Action::Recover => {
                let backups = crate::project_files::backups_dir();
                return Task::open("backup", &["dawproj"], Some(backups), Message::RecoverPicked);
            }
            Action::Export => return Task::save("WAV audio", &["wav"], "export.wav", Message::Exported),
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
}

#[cfg(target_os = "macos")]
static PLUGIN_KEYBOARD_ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Send keys that plugin editors do not use to the app's key bindings, so
/// Space plays while a plugin window is in front. Call on the main thread.
#[cfg(target_os = "macos")]
fn forward_plugin_keys() {
    let held = std::cell::RefCell::new(std::collections::HashSet::new());
    daw_plugins::set_editor_keys(move |press, typing| {
        use keyboard::{Event, Modifiers, key::{Code, Physical}};
        let Some(press) = press else {
            held.borrow_mut().clear();
            crate::ui::post(Message::WindowUnfocused);
            return false;
        };
        let Some(event) = crate::keys::plugin_key_event(press) else { return false };
        let used = match &event {
            Event::KeyReleased { physical_key: Physical::Code(code), .. } => held.borrow_mut().remove(code),
            Event::KeyPressed { physical_key: Physical::Code(code), modifiers, .. }
                if !typing && *modifiers == Modifiers::empty()
                    && PLUGIN_KEYBOARD_ENABLED.load(std::sync::atomic::Ordering::Relaxed)
                    && (computer_keyboard::PIANO.contains(code) || matches!(code, Code::KeyZ | Code::KeyX)) => {
                held.borrow_mut().insert(*code);
                true
            }
            _ => false,
        };
        if used { crate::ui::post(Message::PluginNote(event)); }
        used
    });
    daw_plugins::set_unhandled_keys(move |press| {
        let Some(event) = crate::keys::plugin_key_event(press) else { return false };
        crate::ui::post(Message::PluginKey(event));
        true
    });
}

/// Ask where to save the project, then send the answer as `then`.
fn save_as(then: fn(Option<PathBuf>) -> Message) -> Task {
    Task::save("project", &["dawproj"], "untitled.dawproj", then)
}

fn scan_task(mode: ScanMode) -> Task {
    scan_task_with(move |sender| {
        let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("daw"));
        let progress_sender = sender.clone();
        daw_plugins::scan::scan(&exe, &daw_plugins::scan::cache_path(), mode, move |p| {
            let _ = progress_sender.send(Message::ScanProgress(p));
            crate::ui::wake();
        })
    })
}

/// Defer discovery and child processes until the shell runs this task.
/// Constructing or discarding a task, as unit tests do, must have no side effects.
fn scan_task_with(scan: impl FnOnce(std::sync::mpsc::Sender<Message>) -> Catalog + Send + 'static) -> Task {
    Task::thread(move |sender| {
        let catalog = scan(sender.clone());
        let _ = sender.send(Message::ScanDone(catalog));
    })
}

/// What the shell asks of the app.
impl App {
    /// A digest of what a tick can change on screen, so idle ticks draw nothing.
    pub fn tick_signature(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.position.to_bits().hash(&mut h);
        self.playing.hash(&mut h);
        for (l, r) in &self.meters {
            ((l * 200.0) as u32, (r * 200.0) as u32).hash(&mut h);
        }
        self.toast.as_ref().map(|t| ((t.opacity() * 30.0) as u32, t.text.len())).hash(&mut h);
        self.scan.map(|p| (p.done, p.total)).hash(&mut h);
        self.session.audio_progress().map(|p| (p * 200.0) as u32).hash(&mut h);
        self.session.preparation_error.is_some().hash(&mut h);
        self.rendering.progress().map(|(l, p)| (l.to_string(), (p * 200.0) as u32)).hash(&mut h);
        (self.revision, self.dirty, self.undo.len(), self.redo.len()).hash(&mut h);
        (self.record, self.midi.recording, self.session.recording_armed(), self.computer_keyboard.enabled).hash(&mut h);
        self.status.hash(&mut h);
        self.path.hash(&mut h);
        self.pending.is_some().hash(&mut h);
        h.finish()
    }

    /// Whether something follows the pointer, so every move needs a new view.
    pub fn follows_pointer(&self) -> bool {
        self.browser.drag.is_some_and(|d| d.moved) || self.dragging_split.is_some() || self.browser.drag.is_some()
    }

    /// A text field lost keyboard focus: apply what was typed.
    pub fn field_blurred(&mut self, key: &'static str) -> Task {
        match key {
            "tempo" => self.update(Message::BpmFocused(false)),
            "tail-export" => self.update(Message::Settings(settings::Message::TailDone(settings::Tail::Export))),
            "tail-consolidation" => self.update(Message::Settings(settings::Message::TailDone(settings::Tail::Consolidation))),
            _ => Task::none(),
        }
    }
}

#[cfg(test)]
mod tests;
