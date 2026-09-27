//! Settings: key bindings, saved to the config file on every change.

use iced::keyboard::Event;
use iced::widget::{column, container, row, text};
use iced::{Element, Length};

use super::{label, tool, toggle};
use crate::app::{App, Message as AppMessage};
use crate::keys::{self, BINDINGS, Chord};
use crate::theme;

#[derive(Debug, Default)]
pub struct State {
    /// Action waiting for its new key chord.
    pub capturing: Option<&'static str>,
}

#[derive(Debug, Clone)]
pub enum Message {
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
    ("cmd+scroll", "zoom time"),
    ("alt+scroll", "zoom height (keys, tracks, values)"),
    ("alt+shift+scroll over a note", "note velocity"),
];

pub fn toolbar(app: &App) -> Element<'_, AppMessage> {
    row![tool("reset all", Message::ResetAll.into()), label(app.config_path.display())]
        .spacing(6)
        .align_y(iced::Alignment::Center)
        .into()
}

pub fn view(app: &App) -> Element<'_, AppMessage> {
    let mut list = column![].spacing(1).padding([4, 8]);
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
    let note = "Keys match by position, so Alt chords work on any layout. Panel keys (Delete, arrows, \
                Cmd+A/C/V/D, 1-6 in automation, Q in the piano roll) apply when no binding matches.";
    list = list.push(container(label(note)).padding([8, 0]));
    super::scroll(list, true, false).height(Length::Fill).into()
}

