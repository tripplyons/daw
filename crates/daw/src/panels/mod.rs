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

use iced::widget::{button, pick_list, row, scrollable, text, tooltip};
use iced::widget::scrollable::{Direction, Scrollbar};
use iced::{Element, mouse};

use crate::app::Message;
use crate::app::App;
use crate::keys::{Action, BINDINGS};
use crate::theme;

/// A mixer insert in a pick list.
#[derive(Debug, Clone, PartialEq)]
pub struct InsertChoice {
    pub id: daw_model::InsertId,
    pub name: String,
}

impl std::fmt::Display for InsertChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)
    }
}

/// Small flat button used in toolbars.
pub fn tool<'a>(label: &str, message: Message) -> Element<'a, Message> {
    let hint = hint(&message);
    let control = button(text(label.to_string()).size(theme::SMALL)).on_press(message).style(theme::control).padding([3, 6]);
    match hint {
        Some(hint) => help(control, hint),
        None => control.into(),
    }
}

/// Small toggle button used in toolbars.
pub fn toggle<'a>(label: &str, on: bool, message: Message) -> Element<'a, Message> {
    let hint = hint(&message);
    let control = button(text(label.to_string()).size(theme::SMALL)).on_press(message).style(theme::toggle(on)).padding([3, 6]);
    match hint {
        Some(hint) => help(control, hint),
        None => control.into(),
    }
}

pub fn help<'a>(content: impl Into<Element<'a, Message>>, hint: impl Into<String>) -> Element<'a, Message> {
    tooltip(content, text(hint.into()).size(theme::SMALL), tooltip::Position::Bottom)
        .delay(std::time::Duration::from_millis(450))
        .gap(4)
        .padding(6)
        .style(theme::popup)
        .into()
}

/// Global actions use the user's current bindings in their tooltips.
pub fn action<'a>(app: &App, label: &str, action: Action, on: Option<bool>) -> Element<'a, Message> {
    let hint = action_hint(app, action);
    let control = button(text(label.to_string()).size(theme::SMALL))
        .on_press(Message::Action(action))
        .style(theme::toggle(on.unwrap_or(false)))
        .padding([3, 8]);
    help(control, hint)
}

pub fn action_hint(app: &App, action: Action) -> String {
    let Some(binding) = BINDINGS.iter().find(|b| b.action == action) else { return String::new() };
    let label = match action {
        Action::PlayPause => "Play or pause; pause returns to the start marker",
        Action::Stop => "Stop and rewind to the loop or song start",
        Action::ToggleRecord => "Record parameter movements as automation",
        Action::ToggleBind => "Move a parameter to bind it to an automation clip",
        _ => binding.label,
    };
    let chords: Vec<_> = app.keymap.chords(binding.id).iter().map(ToString::to_string).collect();
    if chords.is_empty() { return label.into(); }
    format!("{label} ({})", chords.join(", "))
}

pub fn hint(message: &Message) -> Option<&'static str> {
    match message {
        Message::Playlist(playlist::Message::MakeUnique) => Some(if cfg!(target_os = "macos") {
            "Give selected clips independent sources (Cmd+U)"
        } else { "Give selected clips independent sources (Ctrl+U)" }),
        Message::Playlist(playlist::Message::Consolidate) => Some(if cfg!(target_os = "macos") {
            "Render selected pattern or audio clips to WAV (Cmd+Alt+C)"
        } else { "Render selected pattern or audio clips to WAV (Ctrl+Alt+C)" }),
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

pub fn label<'a>(label: impl ToString) -> Element<'a, Message> {
    text(label.to_string()).size(theme::SMALL).color(theme::TEXT_DIM).into()
}

/// Keep a toolbar label with its control when the row wraps.
pub fn labeled<'a>(name: &str, control: Element<'a, Message>) -> Element<'a, Message> {
    row![label(name), control].spacing(4).align_y(iced::Alignment::Center).into()
}

pub fn pick<'a, T>(options: Vec<T>, selected: Option<T>, on: impl Fn(T) -> Message + 'a) -> Element<'a, Message>
where
    T: ToString + PartialEq + Clone + 'a,
{
    pick_list(options, selected, on)
        .text_size(theme::SMALL)
        .padding([2, 6])
        .style(theme::pick)
        .menu_style(theme::menu)
        .into()
}

/// Wheel delta in lines, whatever the device reports.
pub fn wheel_lines(delta: mouse::ScrollDelta) -> (f32, f32) {
    match delta {
        mouse::ScrollDelta::Lines { x, y } => (x, y),
        mouse::ScrollDelta::Pixels { x, y } => (x / 20.0, y / 20.0),
    }
}

/// Scroll area with thin scrollbars. `vertical` and `horizontal` pick the axes.
pub fn scroll<'a>(content: impl Into<Element<'a, Message>>, vertical: bool, horizontal: bool) -> scrollable::Scrollable<'a, Message> {
    let bar = || Scrollbar::new().width(4).scroller_width(4).margin(0);
    let direction = match (vertical, horizontal) {
        (true, true) => Direction::Both { vertical: bar(), horizontal: bar() },
        (false, true) => Direction::Horizontal(bar()),
        _ => Direction::Vertical(bar()),
    };
    scrollable(content).direction(direction).style(theme::scrollable)
}

pub fn pick_or<'a, T>(options: Vec<T>, selected: Option<T>, placeholder: &str, on: impl Fn(T) -> Message + 'a) -> Element<'a, Message>
where
    T: ToString + PartialEq + Clone + 'a,
{
    pick_list(options, selected, on)
        .placeholder(placeholder.to_string())
        .text_size(theme::SMALL)
        .padding([2, 6])
        .style(theme::pick)
        .menu_style(theme::menu)
        .into()
}

pub fn toolbar(app: &App, panel: daw_model::layout::Panel) -> Element<'_, Message> {
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
