//! The window: transport, background jobs, and the tiled panels.

use daw_engine::song::PlayMode;
use daw_model::PatternId;
use daw_model::layout::{Axis, Panel, Rect, Split, TileId};
use daw_plugins::scan::Progress;
use rune::{Cursor, Drag, El, Phase, Tx, col, el, icon, row, spacer, text};

use super::{App, JOBS_HEIGHT, Message, TRANSPORT_HEIGHT};
use crate::keys::Action;
use crate::menu;
use crate::panels::{self, automation, browser, channel_rack, mixer, parameters, piano_roll, playlist, settings};
use crate::theme::{self, C};
use crate::ui::{self, icons, widgets::{self, Btn, Choice, Look}};

/// Space between tiles, and around them.
pub(super) const GUTTER: f32 = 6.0;
/// Width of the invisible handle that drags a split line.
const HANDLE: f32 = 10.0;
/// Corner radius of a tile.
const TILE_RADIUS: f32 = 8.0;

impl App {
    /// Everything in the window.
    pub fn root(&self) -> El {
        let mut layers = el().w_full().h_full()
            .child(self.workspace());
        if let Some(toast) = self.toast() { layers = layers.child(toast); }
        if let Some(label) = browser::drag_label(self) { layers = layers.child(label); }
        if self.settings.open { layers = layers.child(self.settings_modal()); }
        if let Some(palette) = &self.palette { layers = layers.child(crate::palette::view(self, palette)); }
        if let Some(open) = &self.menu { layers = layers.child(menu::view(self, open)); }
        if let Some(prompt) = self.save_prompt() { layers = layers.child(prompt); }
        if let Some(dropdown) = widgets::dropdown_layer(self) { layers = layers.child(dropdown); }
        ui::root(self, layers)
    }

    /// Transport, jobs, and tiles, each at the place the layout math gives it.
    fn workspace(&self) -> El {
        let mut base = el().key("desk").absolute(0.0, 0.0).w(self.window.width).h(self.window.height).fill(C.bg)
            .child(self.transport());
        if self.showing_jobs() {
            base = base.child(self.jobs().absolute(0.0, TRANSPORT_HEIGHT));
        }
        if self.compact() {
            let area = self.tile_area();
            base = base.child(self.tile_tabs().absolute(0.0, area.y - GUTTER - theme::HEADER_HEIGHT));
        }
        for (tile, rect) in self.tile_rects() {
            base = base.child(self.tile(tile, rect));
        }
        for split in self.split_handles() {
            base = base.child(split_handle(split));
        }
        base
    }

    /// The last status message at the bottom right, fading out unless it is an error.
    fn toast(&self) -> Option<El> {
        let toast = self.toast.as_ref()?;
        let alpha = toast.opacity();
        let mut body = row().gap(10.0).center_y().pl(12.0).pr(4.0).py(4.0).min_h(36.0);
        if toast.error {
            body = body.child(el().size(8.0, 8.0).radius(4.0).fill(C.danger));
        }
        body = body.child(text(panels::sentence(&toast.text)).font(theme::BODY).color(C.text).wrap().max_w(380.0).shrink());
        if let Some((label, action)) = &toast.action {
            body = body.child(Btn::new(panels::sentence(label)).look(Look::On).build(action.clone()));
        }
        body = body.child(Btn::new("").icon(&icons::CLOSE).ghost().help("Dismiss").build(Message::DismissToast));
        let card = widgets::popover(body)
            .border(1.0, if toast.error { C.danger.alpha(0.6) } else { C.border })
            .opacity(alpha)
            .key("toast")
            .role("status");
        Some(el().absolute(0.0, 0.0).w(self.window.width).h(self.window.height).items_end().justify_end().padding(16.0).child(card))
    }

    /// One tab per tile, for narrow windows that show one tile at a time.
    fn tile_tabs(&self) -> El {
        let layout = self.layout();
        let items = layout.leaves().into_iter().map(|tile| {
            let panel = layout.panel(tile);
            (panels::sentence(&panel.to_string()), tile == layout.focused, Message::FocusTile(tile), String::new())
        }).collect();
        row().w(self.window.width).h(theme::HEADER_HEIGHT).px(GUTTER).center_y().child(widgets::segmented(items))
    }

    /// Settings over a dimmed workspace. Clicking outside or Escape closes it.
    fn settings_modal(&self) -> El {
        let close = Message::Action(Action::Settings);
        let title = row().h(48.0).pl(18.0).pr(10.0).center_y().gap(8.0)
            .child(text("Settings").font(theme::TITLE).color(C.text))
            .child(spacer())
            .child(panels::help(Btn::new("").icon(&icons::CLOSE).ghost().build(close.clone()), "Close settings (Escape)"));
        let width = (self.window.width - 80.0).clamp(280.0, 680.0);
        let height = (self.window.height - 120.0).max(240.0);
        let dialog = widgets::popover(col().w(width).h(height)
            .child(title)
            .child(widgets::rule())
            .child(settings::view(self)))
            .radius(theme::RADIUS_LG + 2.0)
            .key("settings-dialog")
            .role("dialog")
            .on_press(|_: &mut App| {});
        modal(self, dialog, close, 0.0)
    }

    fn jobs(&self) -> El {
        let mut jobs = row().gap(16.0).center_y().h(JOBS_HEIGHT).w(self.window.width).px(14.0).fill(C.header).border_sides([0.0, 0.0, 1.0, 0.0], C.line);
        if let Some(progress) = self.session.audio_progress() {
            jobs = jobs.child(job("Processing audio", progress, Message::CancelAudio));
        } else if self.session.preparation_error.is_some() {
            jobs = jobs.child(widgets::label("Audio processing stopped."))
                .child(Btn::new("Retry").build(Message::RetryAudio));
        }
        if let Some((label, progress)) = self.rendering.progress() {
            jobs = jobs.child(job(&panels::sentence(label), progress, Message::CancelRender));
        }
        jobs
    }

    fn tile(&self, id: TileId, rect: Rect) -> El {
        let layout = self.layout();
        let panel = layout.panel(id);
        let focused = layout.focused == id;
        let compact = self.compact();
        let highlight = focused && !layout.zoomed && !compact && layout.leaves().len() > 1;
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
        let border = if highlight { C.accent.alpha(0.55) } else { C.border };
        col().key(format!("tile-{}", id.0)).absolute(rect.x, rect.y).size(rect.width, rect.height)
            .fill(C.surface)
            .radius(TILE_RADIUS)
            .clip_rounded(TILE_RADIUS)
            .child(self.tile_header(id, panel, rect.width, highlight))
            .child(col().w_full().grow(1.0).min_h(0.0).clip_rounded(0.0).child(body))
            // The outline sits above the header and rows, which would otherwise cover it.
            .child(el().absolute(0.0, 0.0).size(rect.width, rect.height).radius(TILE_RADIUS).border(1.0, border).inert())
    }

    fn tile_header(&self, id: TileId, panel: Panel, width: f32, highlight: bool) -> El {
        let layout = self.layout();
        let compact = self.compact();
        let more = panels::help(
            Btn::new("").icon(&icons::MORE).ghost().build(menu::Message::Open(menu::Item::Tools(id))),
            format!("{} tools, split, focus, and close: {}\nDouble-click the header to fill the window", panels::sentence(&panel.to_string()), panels::action_hint(self, Action::PanelTools)),
        );
        let mut bar = row().h(theme::HEADER_HEIGHT).pl(6.0).pr(4.0).gap(6.0).center_y().w_full()
            .fill(if highlight { C.raised } else { C.header })
            .border_sides([0.0, 0.0, 1.0, 0.0], C.line);
        if width >= 140.0 {
            let choices = Panel::ALL.into_iter().map(|p| Choice::new(panels::sentence(&p.to_string()), Message::SetPanel(id, p), p == panel)).collect();
            let picker = panel_picker(&format!("panel-{}", id.0), &panels::sentence(&panel.to_string()), choices, highlight);
            bar = bar.child(panels::help(picker, "Change this tile's panel"));
            let extra = if layout.zoomed && !compact { 70.0 } else { 0.0 };
            let available = (width - 190.0 - extra).max(0.0);
            bar = bar.child(row().gap(4.0).center_y().grow(1.0).min_w(0.0).clip_rounded(0.0).child(self.primary_tools(panel, available)));
        } else {
            bar = bar.child(text(panels::sentence(&panel.to_string())).font(theme::LABEL).color(C.text).single_line().truncate().grow(1.0).shrink());
        }
        // While a tile fills the window, keep the way back in sight.
        if layout.zoomed && !compact && width >= 140.0 {
            bar = bar.child(Btn::new("Restore").icon(&icons::COLLAPSE).ghost().help(panels::action_hint(self, Action::Zoom)).build(Message::TileAction(id, Action::Zoom)));
        }
        bar = bar.child(more);
        let bar = bar.on_secondary_press(move |app: &mut App, _, tx: &mut Tx| ui::send(app, tx, menu::Message::Open(menu::Item::Tools(id)).into()));
        if compact { bar } else { bar.on_double_press_tx(ui::on(Message::TileAction(id, Action::Zoom))) }
    }

    fn primary_tools(&self, panel: Panel, width: f32) -> El {
        let mut tools: Vec<El> = Vec::new();
        match panel {
            Panel::Browser if width >= 70.0 => tools.push(panels::ghost("Rescan", Message::Rescan)),
            Panel::Playlist => {
                if width >= 40.0 { tools.push(panels::help(Btn::new("Fit").ghost().icon(&icons::EXPAND).build(playlist::Message::FitSong), "Zoom to show the whole song")); }
                if width >= 180.0 { tools.push(playlist::brush_picker(self, 120.0)); }
                if width >= 270.0 { tools.push(Btn::new("Audio").icon(&icons::PLUS).ghost().help(panels::action_hint(self, Action::ImportAudio)).build(Message::Action(Action::ImportAudio))); }
                if width >= 350.0 { tools.push(Btn::new("Track").icon(&icons::PLUS).ghost().help("Add a playlist track").build(playlist::Message::AddTrack)); }
                if width >= 440.0 { tools.push(panels::help(Btn::new("Stretch").ghost().on(self.playlist.stretch_mode).build(playlist::Message::StretchMode), "Stretch audio by dragging a clip edge; off trims the clip")); }
            }
            Panel::ChannelRack => {
                if width >= 80.0 { tools.push(Btn::new("Synth").icon(&icons::PLUS).ghost().help("Add a built-in synth channel").build(channel_rack::Message::AddSynth)); }
                if width >= 170.0 { tools.push(Btn::new("Sample").icon(&icons::PLUS).ghost().help("Load a WAV file into a sampler channel").build(channel_rack::Message::AddSampler)); }
            }
            Panel::PianoRoll => {
                if width >= 110.0 { tools.push(piano_roll::channel_picker(self, 110.0)); }
                if width >= 230.0 {
                    tools.push(panels::labeled("Snap", panels::pick_sized("piano-snap", daw_model::time::Grid::CHOICES.to_vec(), Some(self.project.grid), "Snap", 70.0, |g| piano_roll::Message::Grid(g).into())));
                }
                if width >= 310.0 { tools.push(piano_roll::ghosts_toggle(self)); }
            }
            Panel::Mixer if width >= 90.0 => tools.push(Btn::new("Insert").icon(&icons::PLUS).ghost().help("Add a mixer insert").build(mixer::Message::AddInsert)),
            Panel::Automation if width >= 170.0 => {
                use automation::Tool;
                let items = [("Edit", Tool::Edit), ("Draw", Tool::Draw), ("Line", Tool::Line)].into_iter().map(|(label, tool)| {
                    let message: Message = automation::Message::Tool(tool).into();
                    let hint = panels::hint(&message).unwrap_or_default().to_string();
                    (label.to_string(), self.automation.tool == tool, message, hint)
                }).collect();
                tools.push(widgets::segmented(items));
            }
            _ => {}
        }
        row().gap(4.0).center_y().children(tools)
    }

    fn pattern_picker(&self, width: f32) -> El {
        let choices = self.project.patterns.iter().map(|p| Choice::new(p.name.clone(), Message::SelectPattern(p.id), p.id == self.selected_pattern)).collect();
        let current = self.project.pattern(self.selected_pattern).map(|p| p.name.clone());
        let selected: PatternId = self.selected_pattern;
        panels::help(
            widgets::select("pattern", current, "Pattern", choices, width)
                .on_secondary_press(move |app: &mut App, _, tx: &mut Tx| ui::send(app, tx, menu::Message::Open(menu::Item::Pattern(selected)).into())),
            "Selected pattern; right-click to rename or clone",
        )
    }

    pub(crate) fn pattern_tools(&self) -> El {
        let bar = self.project.signature.ticks_per_bar();
        let bars = self.project.pattern(self.selected_pattern).map(|p| p.length.div_ceil(bar).max(1));
        let mut choices: Vec<u64> = (1..=16).collect();
        if let Some(bars) = bars && !choices.contains(&bars) { choices.push(bars); }
        col().gap(10.0).padding(12.0)
            .child(widgets::section("Pattern"))
            .child(self.pattern_picker(260.0))
            .child(row().gap(8.0).center_y()
                .child(widgets::label("Length"))
                .child(panels::pick_sized("pattern-bars", choices, bars, "Bars", 64.0, Message::PatternBars))
                .child(widgets::label("bars")))
            .child(row().gap(6.0)
                .child(Btn::new("New pattern").icon(&icons::PLUS).help("Create an empty pattern").build(Message::NewPattern))
                .child(Btn::new("Clone").help("Copy this pattern into a new pattern").build(Message::ClonePattern(self.selected_pattern))))
    }

    fn transport(&self) -> El {
        let wide = self.window.width;
        let beats = self.position / f64::from(daw_model::time::TICKS_PER_BEAT);
        let beats_per_bar = f64::from(self.project.signature.numerator);
        let position = format!("{}.{}.{:02}", (beats / beats_per_bar).floor() as u32 + 1,
            (beats % beats_per_bar).floor() as u32 + 1, ((beats.fract()) * 100.0).floor() as u32);
        let play = Btn::new("").icon(if self.playing { &icons::PAUSE } else { &icons::PLAY })
            .look(if self.playing { Look::Primary } else { Look::Raised })
            .width(40.0).height(32.0).key("play")
            .help(panels::action_hint(self, Action::PlayPause))
            .build(Message::Action(Action::PlayPause));
        let stop = Btn::new("").icon(&icons::STOP).width(34.0).height(32.0)
            .help(panels::action_hint(self, Action::Stop))
            .build(Message::Action(Action::Stop));
        let clock = panels::help(
            col().w(92.0).h(32.0).px(10.0).justify_center().radius(theme::RADIUS).fill(C.well).border(1.0, C.border)
                .child(text(position).font(theme::CLOCK).color(C.text).single_line())
                .key("clock"),
            "Position: bar.beat.fraction",
        );
        let bpm_text = self.bpm_text.clone().unwrap_or_else(|| format!("{}", self.project.bpm));
        let tempo = row().gap(6.0).center_y()
            .child(widgets::field(self, "tempo", &bpm_text, "120", Message::SetBpm, Some(Message::BpmDone)).w(64.0))
            .child(widgets::label("BPM"));
        let song = matches!(self.mode, PlayMode::Song);
        let mode_hint = panels::action_hint(self, Action::ToggleMode);
        let mode = widgets::segmented(vec![
            ("Song".into(), song, if song { Message::DismissToast } else { Message::Action(Action::ToggleMode) }, mode_hint.clone()),
            ("Pattern".into(), !song, if song { Message::Action(Action::ToggleMode) } else { Message::DismissToast }, mode_hint),
        ]);
        let mut left = row().gap(8.0).center_y().child(play).child(stop).child(clock).child(tempo).child(mode);
        left = left.child(widgets::divider());
        if wide >= 900.0 {
            left = left.child(self.pattern_picker(150.0))
                .child(panels::help(Btn::new("").icon(&icons::MORE).ghost().build(menu::Message::Open(menu::Item::PatternTools)), "Pattern length, new pattern, and clone"));
        } else {
            left = left.child(Btn::new("Pattern").ghost().build(menu::Message::Open(menu::Item::PatternTools)));
        }
        let mut record = row().gap(2.0).center_y()
            .child(icon(&icons::RECORD, 12.0).color(if self.record || self.midi.recording || self.session.recording_armed() { C.record } else { C.text_faint }).ml(2.0).mr(4.0))
            .child(panels::arm(self, if wide >= 1180.0 { "Automation" } else { "Auto" }, Action::ToggleRecord, self.record))
            .child(panels::arm(self, "MIDI", Action::ToggleMidiRecord, self.midi.recording))
            .child(panels::arm(self, "Audio", Action::ToggleAudioRecord, self.session.recording_armed()));
        record = record.help("Record arm: automation moves, MIDI notes, or audio input");
        let octave = i16::from(self.computer_keyboard.base) / 12 - 1;
        let tools = row().gap(4.0).center_y()
            .child(Btn::new("Bind").ghost().on(self.bind_mode).help(panels::action_hint(self, Action::ToggleBind)).build(Message::Action(Action::ToggleBind)))
            .child(Btn::new(format!("C{octave}")).icon(&icons::KEYBOARD).ghost().on(self.computer_keyboard.enabled)
                .help(panels::action_hint(self, Action::ToggleComputerKeyboard)).build(Message::Action(Action::ToggleComputerKeyboard)));
        // Lasting states only; results and errors show as toasts.
        let scan = match self.scan {
            Some(Progress { done, total }) if total > 0 => format!("Scanning plugins {done}/{total}"),
            Some(_) => "Scanning plugins".into(),
            None => String::new(),
        };
        let status = text(scan.clone()).font(theme::SMALL_TEXT).color(C.text_faint).single_line().truncate().shrink().help(scan);
        let mut files = row().gap(4.0).center_y();
        if wide >= 1100.0 { files = files.child(Btn::new("Save").ghost().help(panels::action_hint(self, Action::Save)).build(Message::Action(Action::Save))); }
        if wide >= 1260.0 { files = files.child(Btn::new("Export").ghost().help(panels::action_hint(self, Action::Export)).build(Message::Action(Action::Export))); }
        files = files
            .child(panels::help(Btn::new("").icon(&icons::FILE).ghost().build(menu::Message::Open(menu::Item::Files)), "New, open, save, import, and export"))
            .child(panels::help(Btn::new("").icon(&icons::PALETTE).ghost().build(Message::Action(Action::Palette)), panels::action_hint(self, Action::Palette)))
            .child(Btn::new("").icon(&icons::SETTINGS).ghost().on(self.settings.open).help(panels::action_hint(self, Action::Settings)).build(Message::Action(Action::Settings)));
        // Clear of the window's traffic lights, and dragging it moves the window.
        let lead = self.ui.chrome_inset.get();
        let mut bar = row().h(TRANSPORT_HEIGHT).w(self.window.width).pl(if lead > 0.0 { lead + 10.0 } else { 12.0 }).pr(12.0).gap(10.0).center_y()
            .fill(C.header).border_sides([0.0, 0.0, 1.0, 0.0], C.line)
            .window_drag()
            .child(left);
        if wide >= 1000.0 {
            bar = bar.child(widgets::divider()).child(record).child(widgets::divider()).child(tools);
        }
        bar.child(row().grow(1.0).min_w(0.0).justify_end().child(status)).child(files).key("transport")
    }

    /// The question about unsaved changes, while it waits for an answer.
    fn save_prompt(&self) -> Option<El> {
        let question = self.pending_question()?;
        let actions = row().gap(8.0).justify_end().w_full()
            .child(Btn::new("Cancel").build(Message::SaveChoice(super::SaveChoice::Cancel)))
            .child(Btn::new("Don't save").build(Message::SaveChoice(super::SaveChoice::Discard)))
            .child(Btn::new("Save").primary().key("save-default").build(Message::SaveChoice(super::SaveChoice::Save)).default_button());
        let dialog = widgets::popover(col().w(380.0).padding(20.0).gap(14.0)
            .child(text(question.0).font(theme::TITLE).color(C.text).wrap())
            .child(text(question.1).font(theme::BODY).color(C.text_dim).wrap())
            .child(actions))
            .radius(theme::RADIUS_LG + 2.0)
            .role("alertdialog")
            .on_press(|_: &mut App| {});
        Some(modal(self, dialog, Message::SaveChoice(super::SaveChoice::Cancel), 0.0))
    }
}

/// A dialog over a dimmed window; a click outside sends `close`.
pub(crate) fn modal(app: &App, dialog: El, close: Message, top: f32) -> El {
    let scrim = el().absolute(0.0, 0.0).w_full().h_full().fill(C.scrim).on_press_tx(ui::on(close));
    let mut place = col().absolute(0.0, 0.0).w(app.window.width).h(app.window.height).items_center();
    place = if top > 0.0 { place.pt(top) } else { place.justify_center() };
    el().absolute(0.0, 0.0).w(app.window.width).h(app.window.height).child(scrim).child(place.child(dialog))
}

/// The panel name at the start of a tile header, opening the list of panels.
fn panel_picker(key: &str, name: &str, choices: Vec<Choice>, focused: bool) -> El {
    let open = widgets::Dropdown { key: key.to_string(), choices, width: 170.0 };
    row().key(key.to_string()).h(26.0).pl(8.0).pr(4.0).gap(4.0).center_y().radius(theme::RADIUS)
        .child(text(name.to_string()).font(theme::LABEL).color(if focused { C.text } else { C.text_dim }).single_line())
        .child(icon(&icons::CHEVRON, 12.0).color(C.text_faint))
        .hover(C.hover)
        .press(C.press)
        .cursor(Cursor::Pointer)
        .role("combobox")
        .on_press_tx(ui::on(Message::Ui(ui::Msg::OpenDropdown(open))))
}

/// A running job's progress, with a button that cancels it.
fn job(label: &str, progress: f32, cancel: Message) -> El {
    row().gap(8.0).center_y()
        .child(widgets::label(label.to_string()))
        .child(widgets::progress(progress, 140.0))
        .child(text(format!("{:.0}%", progress * 100.0)).font(theme::NUMBER).color(C.text_dim).w(34.0))
        .child(Btn::new("Cancel").ghost().build(cancel))
}

/// The handle that drags a split line, wider than the gap it sits in.
fn split_handle(split: Split) -> El {
    let rect = handle_rect(&split);
    let (cursor, line) = match split.axis {
        Axis::Horizontal => (Cursor::ResizeColumn, el().absolute((HANDLE - 2.0) / 2.0, 12.0).size(2.0, (rect.height - 24.0).max(0.0))),
        Axis::Vertical => (Cursor::ResizeRow, el().absolute(12.0, (HANDLE - 2.0) / 2.0).size((rect.width - 24.0).max(0.0), 2.0)),
    };
    let path = split.path.clone();
    el().absolute(rect.x, rect.y).size(rect.width, rect.height).cursor(cursor)
        .child(line.radius(1.0).fill(Color::TRANSPARENT).hover(C.accent.alpha(0.5)))
        .hover_group()
        .on_drag(move |app: &mut App, d: &mut Drag, tx: &mut Tx| {
            let [x, y] = d.window_point();
            let scale = f32::from(app.config.ui_scale) / 100.0;
            match d.phase {
                Phase::Down => ui::send(app, tx, Message::SplitDrag(path.clone())),
                Phase::Move => ui::send(app, tx, Message::MouseMoved(crate::input::Point::new(x / scale, y / scale))),
                Phase::Up => ui::send(app, tx, Message::MouseReleased),
            }
        })
}

use rune::Color;

/// The draggable area around a split line, centered on it.
pub(super) fn handle_rect(split: &Split) -> Rect {
    let line = split.line;
    match split.axis {
        Axis::Horizontal => Rect { x: line.x + (line.width - HANDLE) / 2.0, width: HANDLE, ..line },
        Axis::Vertical => Rect { y: line.y + (line.height - HANDLE) / 2.0, height: HANDLE, ..line },
    }
}

