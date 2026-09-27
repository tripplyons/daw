//! Application state, messages, and top-level update and view.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use daw_engine::output::BitDepth;
use daw_engine::song::PlayMode;
use daw_model::layout::{Axis, Layout, Panel, Rect, TileId};
use daw_model::time::Ticks;
use daw_model::{AutomationId, ChannelId, ClipSource, InsertId, InstanceId, PatternId, Project, Target};
use daw_plugins::scan::{Catalog, Progress};
use iced::futures::channel::mpsc::{self, UnboundedReceiver};
use iced::futures::{Stream, StreamExt, stream};
use iced::widget::{button, column, container, mouse_area, pick_list, row, rule, text, text_input};
use iced::{Element, Length, Point, Size, Subscription, Task, event, keyboard, mouse, window};

use crate::config;
use crate::keys::{Action, Keymap};
use crate::panels::{automation, browser, channel_rack, mixer, parameters, piano_roll, playlist, settings};
use crate::session::Session;
use crate::theme;

/// Project given on the command line, set by `main` before the app starts.
pub static STARTUP_PROJECT: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();

pub const TRANSPORT_HEIGHT: f32 = 26.0;
const GUTTER: f32 = 3.0;
const UNDO_LIMIT: usize = 200;
const LAST_TOUCHED: usize = 16;

#[derive(Debug, Clone)]
pub enum Message {
    Tick,
    Key(keyboard::Event, bool),
    WindowResized(Size),
    MouseMoved(Point),
    MousePressed,
    MouseReleased,
    SplitDrag(Vec<bool>),
    SetPanel(TileId, Panel),
    Action(Action),
    SetBpm(String),
    SelectPattern(PatternId),
    NewPattern,
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
    /// Create (or open) an automation clip for a target, from any panel.
    Automate(Target),
    OpenPlugin(InstanceId),
    /// Show or hide a plugin's editor window; plugins without one show their
    /// parameters instead.
    TogglePlugin(InstanceId),
    Opened(Option<PathBuf>),
    SavedAs(Option<PathBuf>),
    Exported(Option<PathBuf>),
    Screenshot,
    Captured(window::Screenshot),
    /// The window's close button or Cmd+Q.
    CloseRequested,
    CloseChoice(CloseChoice),
    SavedAsThenClose(Option<PathBuf>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseChoice {
    Save,
    Discard,
    Cancel,
}

pub struct App {
    pub project: Project,
    pub path: Option<PathBuf>,
    pub session: Session,
    pub catalog: Catalog,
    pub scan: Option<Progress>,
    pub mode: PlayMode,
    pub playing: bool,
    pub position: f64,
    /// Song-mode start marker: play starts here and pausing returns here.
    pub song_start: f64,
    pub bind_mode: bool,
    pub record: bool,
    pub split_axis: Axis,
    pub status: String,
    pub window: Size,
    pub cursor: Point,
    dragging_split: Option<Vec<bool>>,
    undo: Vec<Project>,
    redo: Vec<Project>,
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
    pub keymap: Keymap,
    pub config: config::Config,
    pub config_path: PathBuf,
    /// Problem reading the config file, shown in settings.
    pub config_error: Option<String>,
    screenshot: Option<PathBuf>,
    /// Cmd+Q has been pointed at the window's close request.
    quit_routed: bool,
    /// A save prompt for closing is showing.
    closing: bool,
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
            catalog: Catalog::default(),
            scan: Some(Progress { done: 0, total: 0 }),
            mode: PlayMode::Pattern(selected_pattern),
            playing: false,
            position: 0.0,
            song_start: 0.0,
            bind_mode: false,
            record: false,
            split_axis: Axis::Horizontal,
            status: String::new(),
            window: Size::new(1400.0, 860.0),
            cursor: Point::ORIGIN,
            dragging_split: None,
            undo: Vec::new(),
            redo: Vec::new(),
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
            keymap: Keymap::default(),
            config: config::Config::default(),
            config_path: config::path(),
            config_error: None,
            screenshot: std::env::var_os("DAW_SCREENSHOT").map(PathBuf::from),
            quit_routed: false,
            closing: false,
        };
        app.load_config();
        if let Some(error) = &app.session.audio_error {
            app.status = format!("no audio output: {error}");
        }
        app.refresh();
        if let Some(path) = STARTUP_PROJECT.get().cloned().flatten() {
            app.open(path);
        }
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
        format!("{}{} - daw", name.unwrap_or_else(|| self.project.name.clone()), if self.dirty { " *" } else { "" })
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

    fn tile_area(&self) -> Rect {
        Rect { x: 0.0, y: TRANSPORT_HEIGHT, width: self.window.width, height: (self.window.height - TRANSPORT_HEIGHT).max(1.0) }
    }

    /// Record an undo snapshot before an edit.
    pub fn checkpoint(&mut self) {
        // Layout changes are not undoable; `restore` keeps the current workspaces.
        self.undo.push(self.project.clone());
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.dirty = true;
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

    fn restore(&mut self, mut project: Project) {
        project.workspaces = self.project.workspaces.clone();
        self.project = project;
        self.dirty = true;
        self.validate_selection();
        self.refresh();
    }

    fn validate_selection(&mut self) {
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
        self.playlist.selected.retain(|id| self.project.playlist.clips.iter().any(|c| c.id == *id));
    }

    /// Bring engine and plugins in line with the project after any edit.
    pub fn refresh(&mut self) {
        self.session.sync(&mut self.project);
        self.session.update_song(&self.project, self.mode);
    }

    /// Mark an edit that already has a checkpoint.
    pub fn edited(&mut self) {
        self.dirty = true;
        self.refresh();
    }

    pub fn set_status(&mut self, status: impl Into<String>) {
        self.status = status.into();
    }

    pub fn target_name(&self, target: Target) -> String {
        let insert_name = |id| self.project.mixer.insert(id).map(|i| i.name.clone()).unwrap_or_default();
        let channel_name = |id| self.project.channel(id).map(|c| c.name.clone()).unwrap_or_default();
        match target {
            Target::Plugin { instance, param } => {
                let plugin = self.project.plugin(instance).map(|p| p.plugin.name.clone()).unwrap_or_default();
                format!("{plugin}: {}", self.session.param_name(instance, param))
            }
            Target::InsertVolume(id) => format!("{} volume", insert_name(id)),
            Target::InsertPan(id) => format!("{} pan", insert_name(id)),
            Target::ChannelVolume(id) => format!("{} volume", channel_name(id)),
            Target::ChannelPan(id) => format!("{} pan", channel_name(id)),
            Target::SynthCutoff(id) => format!("{} cutoff", channel_name(id)),
            Target::Tempo => "tempo".into(),
        }
    }

    /// Current normalized value of a target, for new automation clips.
    pub fn target_value(&self, target: Target) -> f32 {
        let insert = |id| self.project.mixer.insert(id);
        let channel = |id| self.project.channel(id);
        match target {
            Target::Plugin { instance, param } => self.session.param_value(instance, param),
            Target::InsertVolume(id) => insert(id).map(|i| i.volume / 2.0).unwrap_or(0.4),
            Target::InsertPan(id) => insert(id).map(|i| (i.pan + 1.0) / 2.0).unwrap_or(0.5),
            Target::ChannelVolume(id) => channel(id).map(|c| c.volume).unwrap_or(0.8),
            Target::ChannelPan(id) => channel(id).map(|c| (c.pan + 1.0) / 2.0).unwrap_or(0.5),
            Target::SynthCutoff(id) => match channel(id).map(|c| &c.source) {
                Some(daw_model::Source::Synth(params)) => params.cutoff,
                _ => 0.5,
            },
            Target::Tempo => daw_engine::song::tempo_to_normalized(self.project.bpm),
        }
    }

    /// Text for a target value in the parameter's own units.
    pub fn value_text(&self, target: Target, value: f32) -> String {
        match target {
            Target::Plugin { instance, param } => self.session.param_text(instance, param, value),
            Target::InsertVolume(_) => gain_text(value * 2.0),
            Target::ChannelVolume(_) => gain_text(value),
            Target::InsertPan(_) | Target::ChannelPan(_) => pan_text(value * 2.0 - 1.0),
            Target::SynthCutoff(_) => format!("{:.0} Hz", 40.0 * (18000.0f32 / 40.0).powf(value)),
            Target::Tempo => format!("{:.1} bpm", daw_engine::song::tempo_from_normalized(value)),
        }
    }

    pub fn target_steps(&self, target: Target) -> u32 {
        match target {
            Target::Plugin { instance, param } => self.session.param_steps(instance, param),
            _ => 0,
        }
    }

    /// Note that a parameter was touched: feeds the last-touched list, bind
    /// mode, and automation recording.
    pub fn touched(&mut self, target: Target, value: f32) {
        // Plugin parameters live in plugin state, which is saved with the project.
        self.dirty = true;
        self.last_touched.retain(|(t, _)| *t != target);
        self.last_touched.push_front((target, value));
        self.last_touched.truncate(LAST_TOUCHED);
        if self.bind_mode && self.project.automation_for(target).is_none() {
            self.checkpoint();
            self.bind_at(target, value);
        }
        if self.record && self.playing {
            automation::record(self, target, value);
        }
    }

    /// Create an automation clip for `target` at the playhead (or loop range)
    /// on a free playlist track, and open it in the automation editor.
    pub fn bind(&mut self, target: Target) -> AutomationId {
        let value = self.target_value(target);
        self.bind_at(target, value)
    }

    /// Like `bind`, starting the new clip flat at `value`.
    pub fn bind_at(&mut self, target: Target, value: f32) -> AutomationId {
        if let Some(existing) = self.project.automation_for(target) {
            self.open_automation(existing);
            return existing;
        }
        let name = self.target_name(target);
        let id = self.project.add_automation(&name, target, value);
        let bar = self.project.signature.ticks_per_bar();
        let (start, length) = match self.project.playlist.loop_range {
            Some((start, end)) => (start, end - start),
            None => (self.project.grid.snap_floor(self.position as Ticks, self.project.signature) / bar * bar, bar * 4),
        };
        if let Some(clip) = self.project.automation_clip_mut(id) {
            clip.length = length;
            clip.envelope.points[1].time = length;
        }
        let track = self.project.free_track(start, start + length);
        self.project.add_clip(track, start, ClipSource::Automation(id));
        self.set_status(format!("bound {name}"));
        self.open_automation(id);
        self.edited();
        id
    }

    pub fn open_automation(&mut self, id: AutomationId) {
        self.automation.clip = Some(id);
        self.automation.selected.clear();
        self.show_panel(Panel::Automation);
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
            Message::Tick => self.tick(),
            Message::Key(event, typing) => {
                if self.settings.capturing.is_some() {
                    settings::key(self, &event);
                    return Task::none();
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
                let point = self.cursor;
                let area = self.tile_area();
                let hit = self.layout().rects(area).into_iter().find(|(_, r)| {
                    point.x >= r.x && point.x < r.x + r.width && point.y >= r.y && point.y < r.y + r.height
                });
                if let Some((tile, _)) = hit {
                    self.layout_mut().focused = tile;
                }
            }
            Message::MouseReleased => {
                self.dragging_split = None;
                if self.editing {
                    self.editing = false;
                }
            }
            Message::SplitDrag(path) => self.dragging_split = Some(path),
            Message::SetPanel(tile, panel) => {
                self.layout_mut().set_panel(tile, panel);
                self.layout_mut().focused = tile;
            }
            Message::Action(action) => return self.action(action),
            Message::SetBpm(value) => {
                if let Ok(bpm) = value.trim().parse::<f64>()
                    && (20.0..=400.0).contains(&bpm)
                {
                    self.checkpoint();
                    self.project.bpm = bpm;
                    self.edited();
                }
            }
            Message::SelectPattern(id) => {
                self.selected_pattern = id;
                self.piano_roll.selected.clear();
                if let PlayMode::Pattern(_) = self.mode {
                    self.mode = PlayMode::Pattern(id);
                    self.refresh();
                }
                self.playlist.brush = Some(ClipSource::Pattern(id));
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
            Message::EndEdit => self.editing = false,
            Message::Browser(message) => return browser::update(self, message),
            Message::Rack(message) => return channel_rack::update(self, message),
            Message::PianoRoll(message) => piano_roll::update(self, message),
            Message::Playlist(message) => playlist::update(self, message),
            Message::Mixer(message) => mixer::update(self, message),
            Message::Params(message) => parameters::update(self, message),
            Message::Automation(message) => automation::update(self, message),
            Message::Settings(message) => settings::update(self, message),
            Message::Automate(target) => {
                self.checkpoint();
                self.bind(target);
            }
            Message::OpenPlugin(instance) => {
                self.param_instance = Some(instance);
                let title = self.project.plugin(instance).map(|p| p.plugin.name.clone()).unwrap_or_default();
                if self.session.has_editor(instance) {
                    if let Err(error) = self.session.open_editor(instance, &title) {
                        self.set_status(error);
                        self.show_panel(Panel::Parameters);
                    }
                } else {
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
            Message::Opened(path) => {
                if let Some(path) = path {
                    self.open(path);
                }
            }
            Message::SavedAs(path) => {
                if let Some(path) = path {
                    self.path = Some(path);
                    self.save();
                }
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
                if self.closing {
                    return Task::none();
                }
                if !self.dirty {
                    return iced::exit();
                }
                self.closing = true;
                return self.ask_to_save();
            }
            Message::CloseChoice(CloseChoice::Save) => {
                if self.path.is_none() {
                    return Task::perform(save_dialog(), Message::SavedAsThenClose);
                }
                return self.save_and_close();
            }
            Message::CloseChoice(CloseChoice::Discard) => return iced::exit(),
            Message::CloseChoice(CloseChoice::Cancel) | Message::SavedAsThenClose(None) => self.closing = false,
            Message::SavedAsThenClose(Some(path)) => {
                self.path = Some(path);
                return self.save_and_close();
            }
            Message::Exported(path) => {
                if let Some(path) = path {
                    let path = if path.extension().is_none() { path.with_extension("wav") } else { path };
                    match self.session.export(&self.project, &path, BitDepth::Int24, self.mode) {
                        Ok(()) => self.set_status(format!("exported {}", path.display())),
                        Err(error) => self.set_status(format!("export failed: {error}")),
                    }
                    self.playing = false;
                    self.return_to_start();
                }
            }
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
        let was_playing = self.playing;
        self.playing = shared.playing();
        if was_playing && !self.playing {
            automation::stopped(self);
        }
        let shared = self.session.shared();
        self.meters = (0..self.project.mixer.inserts.len()).map(|i| shared.take_peak(i)).collect();
        for (instance, touch) in self.session.poll() {
            self.touched(Target::Plugin { instance, param: touch.param }, touch.value);
            if self.param_instance.is_none() {
                self.param_instance = Some(instance);
            }
        }
    }

    fn drag_split(&mut self, path: &[bool], point: Point) {
        let area = self.tile_area();
        let Some((_, axis, rect)) = self.layout().splits(area).into_iter().find(|(p, _, _)| p == path) else { return };
        let ratio = match axis {
            Axis::Horizontal => (point.x - rect.x) / rect.width,
            Axis::Vertical => (point.y - rect.y) / rect.height,
        };
        self.layout_mut().set_split_ratio(path, ratio);
    }

    fn action(&mut self, action: Action) -> Task<Message> {
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
                self.mode = match self.mode {
                    PlayMode::Song => PlayMode::Pattern(self.selected_pattern),
                    PlayMode::Pattern(_) => PlayMode::Song,
                };
                self.refresh();
                self.return_to_start();
            }
            Action::PlayPause => {
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
                self.session.stop();
                self.playing = false;
                self.song_start = self.project.playlist.loop_range.map(|(s, _)| s as f64).unwrap_or(0.0);
                self.return_to_start();
            }
            Action::Undo => {
                if let Some(previous) = self.undo.pop() {
                    self.redo.push(self.project.clone());
                    self.restore(previous);
                }
            }
            Action::Redo => {
                if let Some(next) = self.redo.pop() {
                    self.undo.push(self.project.clone());
                    let redo = std::mem::take(&mut self.redo);
                    self.restore(next);
                    self.redo = redo;
                }
            }
            Action::New => {
                self.session.clear();
                self.project = Project::new();
                self.path = None;
                self.undo.clear();
                self.redo.clear();
                self.dirty = false;
                self.song_start = 0.0;
                self.selected_pattern = self.project.patterns[0].id;
                self.mode = PlayMode::Pattern(self.selected_pattern);
                self.validate_selection();
                self.refresh();
            }
            Action::Open => {
                return Task::perform(
                    async {
                        rfd::AsyncFileDialog::new()
                            .add_filter("project", &["dawproj"])
                            .pick_file()
                            .await
                            .map(|f| f.path().to_owned())
                    },
                    Message::Opened,
                );
            }
            Action::Save if self.path.is_some() => self.save(),
            Action::Save | Action::SaveAs => return Task::perform(save_dialog(), Message::SavedAs),
            Action::Export => {
                return Task::perform(
                    async {
                        rfd::AsyncFileDialog::new()
                            .add_filter("wav", &["wav"])
                            .set_file_name("export.wav")
                            .save_file()
                            .await
                            .map(|f| f.path().to_owned())
                    },
                    Message::Exported,
                );
            }
        }
        Task::none()
    }

    /// Move the playhead to where playback starts: the pattern start, or the
    /// song marker.
    pub fn return_to_start(&mut self) {
        let start = match self.mode {
            PlayMode::Pattern(_) => 0.0,
            PlayMode::Song => self.song_start,
        };
        self.session.seek(start);
        self.position = start;
    }

    fn ask_to_save(&self) -> Task<Message> {
        let name = self.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());
        let name = name.unwrap_or_else(|| self.project.name.clone());
        Task::perform(
            async move {
                let result = rfd::AsyncMessageDialog::new()
                    .set_level(rfd::MessageLevel::Warning)
                    .set_title("Unsaved changes")
                    .set_description(format!("Save changes to \"{name}\" before closing?"))
                    .set_buttons(rfd::MessageButtons::YesNoCancelCustom("Save".into(), "Don't Save".into(), "Cancel".into()))
                    .show()
                    .await;
                match result {
                    rfd::MessageDialogResult::Custom(label) if label == "Save" => CloseChoice::Save,
                    rfd::MessageDialogResult::Custom(label) if label == "Don't Save" => CloseChoice::Discard,
                    rfd::MessageDialogResult::Yes => CloseChoice::Save,
                    rfd::MessageDialogResult::No => CloseChoice::Discard,
                    _ => CloseChoice::Cancel,
                }
            },
            Message::CloseChoice,
        )
    }

    /// Save, then quit only if the save worked; otherwise stay open with the error.
    fn save_and_close(&mut self) -> Task<Message> {
        self.save();
        if self.dirty {
            self.closing = false;
            return Task::none();
        }
        iced::exit()
    }

    fn save(&mut self) {
        let Some(path) = self.path.clone() else { return };
        self.session.store_states(&mut self.project);
        if let Some(name) = path.file_stem() {
            self.project.name = name.to_string_lossy().into_owned();
        }
        let result = self.project.to_ron().map_err(|e| e.to_string()).and_then(|text| std::fs::write(&path, text).map_err(|e| e.to_string()));
        match result {
            Ok(()) => {
                self.dirty = false;
                self.set_status(format!("saved {}", path.display()));
            }
            Err(error) => self.set_status(format!("save failed: {error}")),
        }
    }

    pub fn open(&mut self, path: PathBuf) {
        let loaded = std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|text| Project::from_ron(&text).map_err(|e| e.to_string()));
        match loaded {
            Ok(project) => {
                self.session.clear();
                self.project = project;
                self.path = Some(path);
                self.undo.clear();
                self.redo.clear();
                self.dirty = false;
                self.song_start = 0.0;
                self.selected_pattern = self.project.patterns[0].id;
                self.mode = PlayMode::Pattern(self.selected_pattern);
                self.validate_selection();
                self.refresh();
                let failures = self.session.load_errors.len();
                self.set_status(if failures == 0 {
                    "opened".to_string()
                } else {
                    format!("opened; {failures} plugins failed to load")
                });
            }
            Err(error) => self.set_status(format!("open failed: {error}")),
        }
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let events = event::listen_with(|event, status, _| match event {
            iced::Event::Keyboard(event) => Some(Message::Key(event, status == event::Status::Captured)),
            iced::Event::Window(window::Event::Resized(size)) => Some(Message::WindowResized(size)),
            iced::Event::Mouse(mouse::Event::CursorMoved { position }) => Some(Message::MouseMoved(position)),
            iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => Some(Message::MouseReleased),
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
        let mut subscriptions = vec![events, presses, tick, close, plugin_keys];
        if self.screenshot.is_some() {
            let delay = std::env::var("DAW_SCREENSHOT_DELAY").ok().and_then(|d| d.parse().ok()).unwrap_or(8);
            subscriptions.push(iced::time::every(Duration::from_secs(delay)).map(|_| Message::Screenshot));
        }
        Subscription::batch(subscriptions)
    }

    pub fn view(&self) -> Element<'_, Message> {
        let layout = self.layout();
        let tiles = if layout.zoomed { self.tile(layout.focused) } else { self.node(&layout.root, &mut Vec::new()) };
        column![self.transport(), tiles].into()
    }

    fn node<'a>(&'a self, node: &daw_model::layout::Node, path: &mut Vec<bool>) -> Element<'a, Message> {
        match node {
            daw_model::layout::Node::Leaf(id) => self.tile(*id),
            daw_model::layout::Node::Split { axis, ratio, first, second } => {
                let a = ((ratio * 1000.0).round() as u16).max(1);
                let b = (1000u16.saturating_sub(a)).max(1);
                path.push(false);
                let first = self.node(first, path);
                path.pop();
                path.push(true);
                let second = self.node(second, path);
                path.pop();
                let handle = path.clone();
                match axis {
                    Axis::Horizontal => {
                        let gutter = mouse_area(
                            container(rule::vertical(1).style(|_| rule_style()))
                                .width(GUTTER)
                                .height(Length::Fill)
                                .center_x(GUTTER),
                        )
                        .on_press(Message::SplitDrag(handle))
                        .interaction(mouse::Interaction::ResizingHorizontally);
                        row![
                            container(first).width(Length::FillPortion(a)),
                            gutter,
                            container(second).width(Length::FillPortion(b))
                        ]
                        .height(Length::Fill)
                        .into()
                    }
                    Axis::Vertical => {
                        let gutter = mouse_area(
                            container(rule::horizontal(1).style(|_| rule_style()))
                                .height(GUTTER)
                                .width(Length::Fill)
                                .center_y(GUTTER),
                        )
                        .on_press(Message::SplitDrag(handle))
                        .interaction(mouse::Interaction::ResizingVertically);
                        column![
                            container(first).height(Length::FillPortion(a)),
                            gutter,
                            container(second).height(Length::FillPortion(b))
                        ]
                        .width(Length::Fill)
                        .into()
                    }
                }
            }
        }
    }

    fn tile(&self, id: TileId) -> Element<'_, Message> {
        let layout = self.layout();
        let panel = layout.panel(id);
        let focused = layout.focused == id;
        let name_color = if focused { theme::BRIGHT } else { theme::TEXT_DIM };
        let picker = pick_list(Panel::ALL, Some(panel), move |p| Message::SetPanel(id, p))
            .text_size(theme::SMALL)
            .padding([2, 6])
            .style(move |theme, status| {
                let mut style = theme::pick(theme, status);
                style.text_color = name_color;
                style.background = iced::Background::Color(iced::Color::TRANSPARENT);
                style
            })
            .menu_style(theme::menu);
        let tools = match panel {
            Panel::Browser => browser::toolbar(self),
            Panel::ChannelRack => channel_rack::toolbar(self),
            Panel::PianoRoll => piano_roll::toolbar(self),
            Panel::Playlist => playlist::toolbar(self),
            Panel::Mixer => mixer::toolbar(self),
            Panel::Automation => automation::toolbar(self),
            Panel::Parameters => parameters::toolbar(self),
            Panel::Settings => settings::toolbar(self),
        };
        let marker = if layout.zoomed { text("focus").size(theme::SMALL).color(theme::TEXT_DIM) } else { text("") };
        let header = container(row![picker, tools, marker].spacing(6).align_y(iced::Alignment::Center))
            .height(theme::HEADER_HEIGHT)
            .width(Length::Fill)
            .padding([0, 4])
            .align_y(iced::Alignment::Center)
            .style(move |t| {
                let mut style = theme::header(t);
                if focused {
                    style.background = Some(iced::Background::Color(theme::CONTROL));
                }
                style
            });
        let body = match panel {
            Panel::Browser => browser::view(self),
            Panel::ChannelRack => channel_rack::view(self),
            Panel::PianoRoll => piano_roll::view(self, focused),
            Panel::Playlist => playlist::view(self, focused),
            Panel::Mixer => mixer::view(self),
            Panel::Automation => automation::view(self, focused),
            Panel::Parameters => parameters::view(self),
            Panel::Settings => settings::view(self),
        };
        container(column![header, container(body).width(Length::Fill).height(Length::Fill)])
            .width(Length::Fill)
            .height(Length::Fill)
            .style(theme::panel)
            .into()
    }

    fn transport(&self) -> Element<'_, Message> {
        let small = |label: &str| text(label.to_string()).size(theme::SMALL);
        let playing = self.playing;
        let mode_label = match self.mode {
            PlayMode::Song => "song",
            PlayMode::Pattern(_) => "pattern",
        };
        let beats = self.position / f64::from(daw_model::time::TICKS_PER_BEAT);
        let beats_per_bar = f64::from(self.project.signature.numerator);
        let position = format!(
            "{}.{}.{:02}",
            (beats / beats_per_bar).floor() as u32 + 1,
            (beats % beats_per_bar).floor() as u32 + 1,
            ((beats.fract()) * 100.0).floor() as u32
        );
        let pattern_names: Vec<PatternChoice> =
            self.project.patterns.iter().map(|p| PatternChoice { id: p.id, name: p.name.clone() }).collect();
        let selected = pattern_names.iter().find(|p| p.id == self.selected_pattern).cloned();
        let scan = match self.scan {
            Some(Progress { done, total }) if total > 0 => format!("scanning plugins {done}/{total}"),
            Some(_) => "scanning plugins".into(),
            None => String::new(),
        };
        let bar = row![
            button(small(if playing { "stop" } else { "play" }))
                .on_press(Message::Action(Action::PlayPause))
                .style(theme::toggle(playing))
                .padding([3, 8]),
            button(small(mode_label)).on_press(Message::Action(Action::ToggleMode)).style(theme::control).padding([3, 8]),
            text(position).size(theme::TEXT_SIZE).font(iced::Font::MONOSPACE).width(70),
            text_input("bpm", &format!("{}", self.project.bpm))
                .on_submit_maybe(None)
                .on_input(Message::SetBpm)
                .size(theme::SMALL)
                .width(48)
                .padding([3, 4])
                .style(theme::input),
            small("bpm").color(theme::TEXT_DIM),
            pick_list(pattern_names, selected, |p: PatternChoice| Message::SelectPattern(p.id))
                .text_size(theme::SMALL)
                .padding([3, 6])
                .style(theme::pick)
                .menu_style(theme::menu),
            button(small("+ pattern")).on_press(Message::NewPattern).style(theme::control).padding([3, 8]),
            button(small("bind")).on_press(Message::Action(Action::ToggleBind)).style(theme::toggle(self.bind_mode)).padding([3, 8]),
            button(small("rec")).on_press(Message::Action(Action::ToggleRecord)).style(theme::toggle(self.record)).padding([3, 8]),
            text(self.status.clone()).size(theme::SMALL).color(theme::TEXT_DIM).width(Length::Fill),
            small(&scan).color(theme::TEXT_DIM),
            button(small("export")).on_press(Message::Action(Action::Export)).style(theme::control).padding([3, 8]),
            button(small("open")).on_press(Message::Action(Action::Open)).style(theme::control).padding([3, 8]),
            button(small("save")).on_press(Message::Action(Action::Save)).style(theme::control).padding([3, 8]),
            button(small("settings")).on_press(Message::Action(Action::Settings)).style(theme::control).padding([3, 8]),
        ]
        .spacing(4)
        .align_y(iced::Alignment::Center);
        container(bar).height(TRANSPORT_HEIGHT).width(Length::Fill).padding([0, 4]).center_y(TRANSPORT_HEIGHT).style(theme::header).into()
    }
}

#[derive(Debug, Clone, PartialEq)]
struct PatternChoice {
    id: PatternId,
    name: String,
}

impl std::fmt::Display for PatternChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)
    }
}

fn rule_style() -> rule::Style {
    rule::Style { color: theme::LINE, radius: 0.0.into(), fill_mode: rule::FillMode::Full, snap: true }
}

/// Keys from plugin editor windows, waiting for the subscription to take them.
static PLUGIN_KEYS: Mutex<Option<UnboundedReceiver<keyboard::Event>>> = Mutex::new(None);

/// Send keys that plugin editors do not use to the app's key bindings, so
/// Space plays while a plugin window is in front. Call on the main thread.
fn forward_plugin_keys() {
    let (sender, receiver) = mpsc::unbounded();
    *PLUGIN_KEYS.lock().unwrap() = Some(receiver);
    daw_plugins::set_unhandled_keys(move |press| {
        let Some(event) = crate::keys::plugin_key_event(press) else { return false };
        sender.unbounded_send(event).is_ok()
    });
}

fn plugin_keys() -> impl Stream<Item = Message> {
    let receiver = PLUGIN_KEYS.lock().unwrap().take();
    stream::iter(receiver).flatten().map(|event| Message::Key(event, false))
}

pub fn gain_text(gain: f32) -> String {
    if gain <= 0.0001 { "-inf dB".into() } else { format!("{:.1} dB", 20.0 * gain.log10()) }
}

pub fn pan_text(pan: f32) -> String {
    match pan {
        p if p.abs() < 0.005 => "C".into(),
        p if p < 0.0 => format!("{:.0}L", -p * 100.0),
        p => format!("{:.0}R", p * 100.0),
    }
}

/// Headless render: `daw --export <project> <file.wav>`. Returns the exit code.
pub fn export_cli(project: &str, out: &str) -> i32 {
    let mut app = App::boot().0;
    app.open(PathBuf::from(project));
    for (instance, error) in &app.session.load_errors {
        eprintln!("plugin {instance:?} failed to load: {error}");
    }
    let mode = app.mode;
    match app.session.export(&app.project, std::path::Path::new(out), BitDepth::Int24, mode) {
        Ok(()) => {
            println!("exported {out}");
            0
        }
        Err(error) => {
            eprintln!("export failed: {error}");
            1
        }
    }
}

async fn save_dialog() -> Option<PathBuf> {
    rfd::AsyncFileDialog::new()
        .add_filter("project", &["dawproj"])
        .set_file_name("untitled.dawproj")
        .save_file()
        .await
        .map(|f| f.path().to_owned())
}

fn scan_task() -> Task<Message> {
    let (sender, receiver) = iced::futures::channel::mpsc::unbounded();
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
