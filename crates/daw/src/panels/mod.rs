//! Panels shown in tiles. Each owns its view state, messages, and edit handling.

pub mod automation;
pub mod browser;
pub mod channel_rack;
pub mod mixer;
pub mod parameters;
pub mod piano_roll;
pub mod playlist;
pub mod settings;
pub mod timeline;

use rune::{El, ScrollSpec, row};

use crate::app::{App, Message};
use crate::input::ScrollDelta;
use crate::keys::{Action, BINDINGS};
use crate::ui::widgets::{self, Btn, Choice, Look};

/// Small button used in toolbars, with its tooltip.
pub fn tool(label: &str, message: Message) -> El {
    let hint = hint(&message).unwrap_or_default();
    Btn::new(label).help(hint).build(message)
}

/// A toolbar button without a fill until hovered.
pub fn ghost(label: &str, message: Message) -> El {
    let hint = hint(&message).unwrap_or_default();
    Btn::new(label).ghost().help(hint).build(message)
}

/// Small toggle button used in toolbars: the accent while on.
pub fn toggle(label: &str, on: bool, message: Message) -> El {
    let hint = hint(&message).unwrap_or_default();
    Btn::new(label).look(if on { Look::On } else { Look::Raised }).help(hint).build(message)
}

/// A tooltip on any element.
pub fn help(content: El, hint: impl Into<String>) -> El {
    let hint = hint.into();
    if hint.is_empty() { content } else { content.help(hint) }
}

/// Global actions use the user's current bindings in their tooltips.
pub fn action(app: &App, label: &str, action: Action, on: Option<bool>) -> El {
    Btn::new(label).on(on.unwrap_or(false)).help(action_hint(app, action)).build(Message::Action(action))
}

/// A record-arm toggle, red while armed.
pub fn arm(app: &App, label: &str, action: Action, armed: bool) -> El {
    Btn::new(label).ghost().armed(armed).help(action_hint(app, action)).build(Message::Action(action))
}

pub fn action_hint(app: &App, action: Action) -> String {
    let Some(binding) = BINDINGS.iter().find(|b| b.action == action) else { return String::new() };
    let label = match action {
        Action::PlayPause => "Play or stop; stop returns to the start marker",
        Action::Stop => "Stop and rewind to the loop or song start",
        Action::ToggleRecord => "Record parameter movements as automation",
        Action::ToggleComputerKeyboard => "Computer keyboard: A W S E D F T G Y H U J K play notes; Z/X change octave",
        Action::ToggleBind => "Move a parameter to bind it to an automation clip",
        _ => binding.label,
    };
    let chords: Vec<_> = app.keymap.chords(binding.id).iter().map(ToString::to_string).collect();
    let label = sentence(label);
    if chords.is_empty() { return label; }
    format!("{label} ({})", chords.join(", "))
}

/// Text with its first letter capitalized, for labels written in lowercase.
pub fn sentence(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

pub fn hint(message: &Message) -> Option<&'static str> {
    match message {
        Message::Playlist(playlist::Message::MakeUnique) => Some(if cfg!(target_os = "macos") {
            "Give selected clips independent sources (Cmd+U)"
        } else { "Give selected clips independent sources (Ctrl+U)" }),
        Message::Playlist(playlist::Message::Consolidate) => Some(if cfg!(target_os = "macos") {
            "Render selected pattern or audio clips to WAV (Cmd+Shift+C)"
        } else { "Render selected pattern or audio clips to WAV (Ctrl+Shift+C)" }),
        Message::Playlist(playlist::Message::Duplicate) => Some(if cfg!(target_os = "macos") {
            "Duplicate the selection after its last clip (Cmd+D)"
        } else { "Duplicate the selection after its last clip (Ctrl+D)" }),
        Message::Playlist(playlist::Message::MuteSelection) => Some("Mute or unmute selected clips (M)"),
        Message::Playlist(playlist::Message::DeleteSelection) => Some("Delete selected clips (Delete)"),
        Message::Playlist(playlist::Message::AddTrack) => Some("Add a playlist track"),
        Message::Playlist(playlist::Message::StretchMode) => Some("Stretch audio by dragging a clip edge; off trims the clip"),
        Message::Rack(channel_rack::Message::AddSynth) => Some("Add a built-in synth channel"),
        Message::Rack(channel_rack::Message::AddSampler) => Some("Load a WAV file into a sampler channel"),
        Message::Automation(automation::Message::Lfo) => Some("Generate an LFO using the selected shape and rate"),
        Message::Automation(automation::Message::Tool(automation::Tool::Edit)) => Some("Add and move automation points"),
        Message::Automation(automation::Message::Tool(automation::Tool::Draw)) => Some("Paint an automation curve"),
        Message::Automation(automation::Message::Tool(automation::Tool::Line)) => Some("Draw an automation ramp"),
        Message::Mixer(mixer::Message::AddInsert) => Some("Add a mixer insert"),
        Message::NewPattern => Some("Create an empty pattern"),
        Message::ClonePattern(_) => Some("Copy this pattern into a new pattern"),
        Message::Rescan => Some("Scan installed plugins again"),
        _ => None,
    }
}

/// A centered hint for an empty panel: a title, a few lines on how to start,
/// and buttons for the first steps.
pub fn empty_state(title: &str, lines: &[String], actions: Vec<El>) -> El {
    widgets::empty_state(title, lines, actions)
}

pub fn label(label: impl ToString) -> El {
    widgets::label(label.to_string())
}

/// Keep a toolbar label with its control when the row wraps.
pub fn labeled(name: &str, control: El) -> El {
    widgets::labeled(name, control)
}

/// A dropdown of `options`, showing `selected`. `key` names the control so
/// the list opens under it.
pub fn pick<T>(key: &str, options: Vec<T>, selected: Option<T>, on: impl Fn(T) -> Message) -> El
where
    T: ToString + PartialEq + Clone,
{
    pick_or(key, options, selected, "Choose", on)
}

pub fn pick_or<T>(key: &str, options: Vec<T>, selected: Option<T>, placeholder: &str, on: impl Fn(T) -> Message) -> El
where
    T: ToString + PartialEq + Clone,
{
    pick_sized(key, options, selected, placeholder, 0.0, on)
}

/// A dropdown with a set width; 0 sizes it to the longest choice.
pub fn pick_sized<T>(key: &str, options: Vec<T>, selected: Option<T>, placeholder: &str, width: f32, on: impl Fn(T) -> Message) -> El
where
    T: ToString + PartialEq + Clone,
{
    let longest = options.iter().map(|o| o.to_string().chars().count()).max().unwrap_or(0).max(placeholder.chars().count());
    let width = if width > 0.0 { width } else { (longest as f32 * 6.6 + 34.0).clamp(56.0, 220.0) };
    let current = selected.as_ref().map(ToString::to_string);
    let choices = options.into_iter().map(|o| {
        let chosen = selected.as_ref() == Some(&o);
        Choice::new(o.to_string(), on(o), chosen)
    }).collect();
    widgets::select(key, current, placeholder, choices, width)
}

/// Wheel delta in lines, whatever the device reports.
pub fn wheel_lines(delta: ScrollDelta) -> (f32, f32) {
    match delta {
        ScrollDelta::Lines { x, y } => (x, y),
        ScrollDelta::Pixels { x, y } => (x / 20.0, y / 20.0),
    }
}

/// Scroll area. `vertical` and `horizontal` pick the axes. It fills the
/// space its parent gives it.
pub fn scroll(content: El, vertical: bool, horizontal: bool) -> El {
    let area = rune::col().w_full().grow(1.0).min_h(0.0).min_w(0.0).child(content);
    let area = match (vertical, horizontal) {
        (true, true) => area.scroll_xy(),
        (false, true) => area.scroll_x(),
        _ => area.scroll(ScrollSpec { stick_bottom: false }),
    };
    // Content cut off at an edge fades out there instead of ending in a hard line.
    area.scroll_edge_fade(24.0)
}

/// A row of toolbar controls that wraps when narrow.
pub fn bar(children: Vec<El>) -> El {
    row().gap(4.0).center_y().flex_wrap().children(children)
}

pub fn toolbar(app: &App, panel: daw_model::layout::Panel) -> El {
    use daw_model::layout::Panel;
    match panel {
        Panel::Browser => browser::toolbar(app),
        Panel::ChannelRack => channel_rack::toolbar(app),
        Panel::PianoRoll => piano_roll::toolbar(app),
        Panel::Playlist => playlist::toolbar(app),
        Panel::Mixer => mixer::toolbar(app),
        Panel::Automation => automation::toolbar(app),
        Panel::Parameters => parameters::toolbar(app),
        Panel::Settings => settings::toolbar(app),
    }
}
