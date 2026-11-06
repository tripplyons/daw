//! A searchable list of every bound action and every panel, opened with
//! Cmd+K, so commands are easy to find without knowing where they live.

use daw_model::layout::Panel;
use iced::widget::{Space, button, column, container, mouse_area, opaque, operation, row, stack, text, text_input};
use iced::{Element, Length, Task};

use crate::app::{App, Message as AppMessage};
use crate::keys::{Action, BINDINGS};
use crate::theme;

const INPUT: &str = "palette-input";
/// Commands shown at once; typing narrows the list.
const ROWS: usize = 12;

#[derive(Debug, Default)]
pub struct Palette {
    pub query: String,
    pub selected: usize,
}

#[derive(Debug, Clone)]
pub enum Message {
    Query(String),
    Move(i32),
    /// Run the command at this position in the shown list.
    Run(usize),
    /// Run the selected command.
    Submit,
    Close,
}

impl From<Message> for AppMessage {
    fn from(message: Message) -> Self {
        AppMessage::Palette(message)
    }
}

pub struct Command {
    pub label: String,
    group: &'static str,
    keys: String,
    pub message: AppMessage,
}

fn commands(app: &App) -> Vec<Command> {
    let mut list: Vec<Command> = BINDINGS
        .iter()
        .filter(|b| b.action != Action::Palette)
        .map(|b| Command {
            label: b.label.to_string(),
            group: b.group,
            keys: app.keymap.chords(b.id).first().map(ToString::to_string).unwrap_or_default(),
            message: AppMessage::Action(b.action),
        })
        .collect();
    list.extend(Panel::ALL.iter().map(|&panel| Command {
        label: format!("show {panel}"),
        group: "panels",
        keys: String::new(),
        message: AppMessage::ShowPanel(panel),
    }));
    list
}

/// Commands whose label or group contains every word of the query: labels
/// that start with the query first, then by group.
pub fn matches(app: &App, query: &str) -> Vec<Command> {
    let query = query.trim().to_lowercase();
    let mut found: Vec<Command> = commands(app)
        .into_iter()
        .filter(|c| {
            let haystack = format!("{} {}", c.label, c.group).to_lowercase();
            query.split_whitespace().all(|word| haystack.contains(word))
        })
        .collect();
    // Everyday commands before layout ones; sorting is stable within a group.
    const GROUPS: [&str; 7] = ["file", "edit", "transport", "panels", "automation", "tiling", "workspaces"];
    let rank = |c: &Command| GROUPS.iter().position(|g| *g == c.group).unwrap_or(GROUPS.len());
    found.sort_by_key(|c| (!c.label.to_lowercase().starts_with(&query), rank(c), c.keys.is_empty()));
    found.truncate(ROWS);
    found
}

pub fn open(app: &mut App) -> Task<AppMessage> {
    if app.palette.take().is_some() { return Task::none(); }
    app.palette = Some(Palette::default());
    app.menu = None;
    app.settings.open = false;
    operation::focus(INPUT)
}

pub fn update(app: &mut App, message: Message) -> Task<AppMessage> {
    let Some(palette) = &mut app.palette else { return Task::none() };
    match message {
        Message::Query(query) => {
            palette.query = query;
            palette.selected = 0;
        }
        Message::Move(delta) => {
            let count = matches(app, &app.palette.as_ref().map(|p| p.query.clone()).unwrap_or_default()).len();
            if let Some(palette) = &mut app.palette
                && count > 0
            {
                palette.selected = (palette.selected as i32 + delta).rem_euclid(count as i32) as usize;
            }
        }
        Message::Submit => {
            let selected = palette.selected;
            return update(app, Message::Run(selected));
        }
        Message::Run(index) => {
            let query = palette.query.clone();
            app.palette = None;
            if let Some(command) = matches(app, &query).into_iter().nth(index) {
                return app.update(command.message);
            }
        }
        Message::Close => app.palette = None,
    }
    Task::none()
}

pub fn view<'a>(app: &'a App, palette: &'a Palette) -> Element<'a, AppMessage> {
    let input = text_input("type a command", &palette.query)
        .id(INPUT)
        .on_input(|q| Message::Query(q).into())
        .on_submit(Message::Submit.into())
        .size(theme::TEXT_SIZE)
        .padding([6, 8])
        .style(theme::input);
    let mut list = column![].spacing(1);
    let found = matches(app, &palette.query);
    if found.is_empty() {
        list = list.push(container(text("no matching command").size(theme::SMALL).color(theme::TEXT_DIM)).padding([4, 8]));
    }
    for (index, command) in found.into_iter().enumerate() {
        let selected = index == palette.selected;
        let line = row![
            text(command.label).size(theme::SMALL).width(Length::Fill),
            text(command.group).size(theme::SMALL).color(theme::TEXT_DIM),
            container(text(command.keys).size(theme::SMALL).color(theme::TEXT_DIM)).width(110).align_right(110),
        ]
        .spacing(12);
        list = list.push(
            button(line)
                .on_press(Message::Run(index).into())
                .padding([4, 8])
                .width(Length::Fill)
                .style(move |t, status| if selected { theme::toggle(true)(t, status) } else { theme::menu_entry(t, status) }),
        );
    }
    let width = (app.window.width - 40.0).clamp(200.0, 520.0);
    let dialog = container(column![input, list].spacing(6)).padding(6).width(width).style(theme::popup);
    let backdrop = mouse_area(container(Space::new()).width(Length::Fill).height(Length::Fill)
        .style(|_| container::Style { background: Some(iced::Color { a: 0.35, ..iced::Color::BLACK }.into()), ..container::Style::default() }))
        .on_press(Message::Close.into());
    let placed = container(opaque(dialog)).center_x(Length::Fill).padding(iced::Padding { top: 80.0, ..iced::Padding::ZERO });
    stack![backdrop, placed].into()
}
