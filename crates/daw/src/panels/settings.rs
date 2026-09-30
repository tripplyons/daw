//! Settings: MIDI input, autosave, render tails, and key bindings. Every
//! change except the tails saves to the config file; the tails belong to the
//! project.

use daw_model::RenderSettings;
use iced::keyboard::Event;
use iced::widget::{column, container, row, text, text_input};
use iced::{Element, Length};

use super::{label, pick, tool, toggle};
use crate::app::{App, Message as AppMessage};
use crate::keys::{self, BINDINGS, Chord};
use crate::theme;

#[derive(Debug, Default)]
pub struct State {
    /// Action waiting for its new key chord.
    pub capturing: Option<&'static str>,
    /// Tail length being typed, before it is checked.
    pub tail_text: Option<(Tail, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tail {
    Export,
    Consolidation,
}

impl Tail {
    fn name(self) -> &'static str {
        match self {
            Tail::Export => "export tail",
            Tail::Consolidation => "consolidation tail",
        }
    }

    fn seconds(self, settings: &mut RenderSettings) -> &mut f64 {
        match self {
            Tail::Export => &mut settings.export_tail_seconds,
            Tail::Consolidation => &mut settings.consolidation_tail_seconds,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    MidiPort(crate::midi::Port),
    MidiRescan,
    Autosave(u64),
    UiScale(u16),
    TailText(Tail, String),
    TailDone(Tail),
    Capture(&'static str),
    Cancel,
    Remove(&'static str, Chord),
    Reset(&'static str),
    ResetAll,
}

impl From<Message> for AppMessage {
    fn from(message: Message) -> Self {
        AppMessage::Settings(message)
    }
}

pub fn update(app: &mut App, message: Message) {
    match message {
        Message::MidiRescan => {
            if let Err(error) = app.midi.rescan() { app.set_status(format!("MIDI scan failed: {error}")); }
        }
        Message::MidiPort(port) => {
            app.poll_midi();
            app.finish_midi();
            let name = port.index.map(|_| port.name.clone());
            if app.connect_midi(port) {
                app.config.midi_input = name;
                app.save_config("MIDI input changed".into());
            }
        }
        Message::Autosave(minutes) => {
            app.config.autosave_minutes = minutes;
            app.save_config(if minutes == 0 { "autosave off".into() } else { format!("autosave every {minutes} minutes") });
        }
        Message::UiScale(scale) => {
            if !crate::config::UI_SCALES.contains(&scale) { return; }
            // Window and cursor coordinates are in the scaled UI's logical pixels.
            let ratio = f32::from(app.config.ui_scale) / f32::from(scale);
            app.window.width *= ratio;
            app.window.height *= ratio;
            app.cursor.x *= ratio;
            app.cursor.y *= ratio;
            app.menu = None;
            app.config.ui_scale = scale;
            app.save_config(format!("UI scale {scale}%"));
        }
        Message::TailText(tail, value) => app.settings.tail_text = Some((tail, value)),
        Message::TailDone(tail) => {
            let Some((typed, text)) = app.settings.tail_text.as_ref() else { return };
            if *typed != tail { return; }
            let Ok(value) = text.trim().parse::<f64>() else { app.set_status(format!("{} must be a number", tail.name())); return };
            let value = match RenderSettings::check_tail(tail.name(), value) {
                Ok(value) => value,
                Err(error) => { app.set_status(error); return; }
            };
            if *tail.seconds(&mut app.project.render) != value {
                app.checkpoint();
                *tail.seconds(&mut app.project.render) = value;
                app.mark_edited();
            }
            app.settings.tail_text = None;
        }
        Message::Capture(id) => app.settings.capturing = Some(id),
        Message::Cancel => app.settings.capturing = None,
        Message::Remove(id, chord) => {
            app.keymap.remove(id, chord);
            app.save_config(format!("removed {chord} from {}", keys::label(id)));
        }
        Message::Reset(id) => {
            app.keymap.reset(id);
            app.save_config(format!("reset {}", keys::label(id)));
        }
        Message::ResetAll => {
            app.keymap = keys::Keymap::default();
            app.settings.capturing = None;
            app.save_config("reset all key bindings".into());
        }
    }
}

/// Handle a key press while capturing. Escape alone cancels; modifier keys
/// on their own wait for the rest of the chord.
pub fn key(app: &mut App, event: &Event) {
    let Some(id) = app.settings.capturing else { return };
    let Some(chord) = Chord::from_event(event) else { return };
    if chord == Chord::parse("escape").expect("valid chord") {
        app.settings.capturing = None;
        return;
    }
    app.settings.capturing = None;
    let status = match app.keymap.assign(id, chord) {
        Some(from) => format!("{chord} moved from {} to {}", keys::label(from), keys::label(id)),
        None => format!("{chord} bound to {}", keys::label(id)),
    };
    app.save_config(status);
}

/// The shared wheel scheme in `timeline::Wheel`, for reference.
const SCROLLING: &[(&str, &str)] = &[
    ("scroll", "scroll up and down"),
    ("shift+scroll, or swipe sideways", "scroll sideways"),
    (if cfg!(target_os = "macos") { "cmd+scroll" } else { "ctrl+scroll" }, "zoom time"),
    ("alt+scroll", "zoom height (keys, tracks, values)"),
    ("alt+shift+scroll over a note", "note velocity"),
];

pub fn toolbar(app: &App) -> Element<'_, AppMessage> {
    row![tool("reset all", Message::ResetAll.into()), label(app.config_path.display())]
        .spacing(6)
        .align_y(iced::Alignment::Center)
        .wrap()
        .into()
}

pub fn view(app: &App) -> Element<'_, AppMessage> {
    let tail_row = |tail: Tail, value: f64| {
        let shown = app.settings.tail_text.as_ref().filter(|(typed, _)| *typed == tail).map_or_else(|| value.to_string(), |(_, text)| text.clone());
        row![label(tail.name()), text_input("seconds", &shown).on_input(move |s| Message::TailText(tail, s).into())
            .on_submit(Message::TailDone(tail).into()).width(80).size(theme::SMALL).padding([3, 6]).style(theme::input),
            label(format!("seconds (0-{})", RenderSettings::MAX_TAIL_SECONDS)), tool("set", Message::TailDone(tail).into())].spacing(6).align_y(iced::Alignment::Center)
    };
    let mut list = column![
        row![label("UI scale"), pick(crate::config::UI_SCALES.to_vec(), Some(app.config.ui_scale), |s| Message::UiScale(s).into()), label("percent")].spacing(6),
        label("MIDI input"),
        pick(app.midi.ports.clone(), app.midi.selected.clone(), |p| Message::MidiPort(p).into()),
        row![tool("rescan inputs", Message::MidiRescan.into())],
        row![label("autosave minutes (0 = off)"), pick(vec![0u64, 1, 2, 5, 10], Some(app.config.autosave_minutes), |m| Message::Autosave(m).into())].spacing(6),
        row![
            tool("recover backup", AppMessage::Action(crate::keys::Action::Recover)),
            tool("package project", AppMessage::Action(crate::keys::Action::Pack))].spacing(6),
        label("project render settings"),
        tail_row(Tail::Export, app.project.render.export_tail_seconds),
        tail_row(Tail::Consolidation, app.project.render.consolidation_tail_seconds),
    ].spacing(6).padding([4, 8]);
    if let Some(error) = &app.config_error {
        list = list.push(container(text(error.clone()).size(theme::SMALL).color(theme::BRIGHT)).padding([2, 0]));
    }
    let mut group = "";
    for binding in BINDINGS {
        if binding.group != group {
            group = binding.group;
            list = list.push(container(label(group)).padding(iced::Padding { top: 8.0, bottom: 2.0, ..iced::Padding::ZERO }));
        }
        let id = binding.id;
        let mut chords = row![].spacing(2);
        for &chord in app.keymap.chords(id) {
            chords = chords.push(tool(&format!("{chord}  x"), Message::Remove(id, chord).into()));
        }
        let capturing = app.settings.capturing == Some(id);
        let add: Element<'_, AppMessage> = if capturing {
            row![toggle("press keys", true, Message::Cancel.into()), label("escape cancels")].spacing(6).align_y(iced::Alignment::Center).into()
        } else {
            tool("+", Message::Capture(id).into())
        };
        let reset: Element<'_, AppMessage> =
            if app.keymap.is_default(id) { label("") } else { tool("reset", Message::Reset(id).into()) };
        list = list.push(
            row![
                text(binding.label).size(theme::SMALL).width(180),
                chords,
                add,
                container(reset).width(Length::Fill).align_x(iced::Alignment::End),
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center),
        );
    }
    // Scrolling is fixed; list it next to the editable keys.
    list = list.push(container(label("scrolling")).padding(iced::Padding { top: 8.0, bottom: 2.0, ..iced::Padding::ZERO }));
    for (combo, action) in SCROLLING {
        list = list.push(row![text(*action).size(theme::SMALL).width(180), label(*combo)].spacing(6));
    }
    let command = if cfg!(target_os = "macos") { "Cmd" } else { "Ctrl" };
    let note = format!("Keys match by position, so Alt chords work on any layout. Panel keys (Delete, arrows, \
                {command}+A/C/V/D, 1-6 in automation, Q in the piano roll) apply when no binding matches.");
    list = list.push(container(label(note)).padding([8, 0]));
    super::scroll(list, true, false).height(Length::Fill).into()
}
