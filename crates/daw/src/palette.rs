//! A searchable list of every bound action and every panel, opened with
//! Cmd+K, so commands are easy to find without knowing where they live.

use daw_model::layout::Panel;
use rune::{Cursor, El, col, icon, row, text};

use crate::app::{App, Message as AppMessage};
use crate::keys::{Action, BINDINGS};
use crate::panels::sentence;
use crate::task::Task;
use crate::theme::{self, C};
use crate::ui::{self, icons, widgets};

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

pub fn open(app: &mut App) -> Task {
    if app.palette.take().is_some() { return Task::none(); }
    app.palette = Some(Palette::default());
    app.menu = None;
    app.settings.open = false;
    Task::focus(INPUT)
}

pub fn update(app: &mut App, message: Message) -> Task {
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

/// A key combination as small keycaps: "cmd+shift+z" → Cmd Shift Z.
fn keycaps(keys: &str) -> El {
    let mut caps = row().gap(3.0).center_y();
    if keys.is_empty() { return caps; }
    for part in keys.split('+') {
        let name = crate::keys::pretty(part);
        caps = caps.child(row().h(18.0).min_w(18.0).px(5.0).center().radius(4.0).fill(C.raised).border(1.0, C.border)
            .child(text(name).font(theme::SMALL_TEXT).color(C.text_dim).single_line()));
    }
    caps
}

pub fn view(app: &App, palette: &Palette) -> El {
    let input = widgets::field(app, INPUT, &palette.query, "Search commands and panels", |q| Message::Query(q).into(), Some(Message::Submit.into()));
    let search = row().h(48.0).px(14.0).gap(10.0).center_y()
        .child(icon(&icons::SEARCH, 16.0).color(C.text_faint))
        .child(input.grow(1.0).fill(rune::Color::TRANSPARENT).border(0.0, rune::Color::TRANSPARENT).px(0.0));
    let mut list = col().gap(1.0).padding(6.0);
    let found = matches(app, &palette.query);
    if found.is_empty() {
        list = list.child(row().h(40.0).px(10.0).center_y().child(text("No matching command").font(theme::BODY).color(C.text_dim)));
    }
    for (index, command) in found.into_iter().enumerate() {
        let selected = index == palette.selected;
        let mut line = row().w_full().h(34.0).px(10.0).gap(12.0).center_y().radius(theme::RADIUS)
            .child(text(sentence(&command.label)).font(theme::BODY).color(C.text).single_line().truncate().grow(1.0).shrink().min_w(0.0))
            .child(text(sentence(command.group)).font(theme::LABEL).color(C.text_faint).single_line())
            .child(row().w(120.0).justify_end().child(keycaps(&command.keys)))
            .hover(C.hover)
            .press(C.press)
            .cursor(Cursor::Pointer)
            .role("option")
            .on_press_tx(ui::on(Message::Run(index)));
        if selected { line = line.fill(C.accent_soft); }
        list = list.child(line);
    }
    let width = (app.window.width - 40.0).clamp(240.0, 580.0);
    let dialog = widgets::popover(col().w(width).child(search).child(widgets::rule()).child(list))
        .radius(theme::RADIUS_LG + 2.0)
        .role("dialog")
        .on_press(|_: &mut App| {});
    crate::app::modal(app, dialog, Message::Close.into(), 90.0)
}
